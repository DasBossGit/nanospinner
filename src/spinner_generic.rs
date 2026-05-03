use ::std::{mem::ManuallyDrop, sync::atomic::AtomicUsize};
use std::io::{self, IsTerminal};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use crate::{
    shared::{
        format_finalize, format_finalize_plain, format_frame, CLEAR_LINE, DEFAULT_FINISH, FRAMES,
    },
    symbol::{AsciiColor, Symbol},
    update::UpdateStrategy,
};

/// A builder for configuring and starting a terminal spinner.
///
/// This is the same builder as the [`nanospinner::Spinner`] but rather than having heap allocated dynamic types,
/// its realies entierly on generic type parameters, thus allowing it to be used in `no_std` environments without the need for `alloc`,
/// and also better optimization opportunities for the compiler. The tradeoff is that it is less ergonomic to use, and requires more and strict type annotations.
///
/// Use [`Spinner::new`] for stdout, or [`Spinner::with_writer`] /
/// [`Spinner::with_writer_tty`] for custom output targets. Call
/// [`Spinner::start`] to begin the animation and get a [`SpinnerHandle`].
pub struct Spinner<State, Display, Frames, Finish, Writer = io::Stdout>
where
    State: Send + 'static,
    Display: ::std::fmt::Display,
    Frames: IntoIterator<Item = Display>,
    Frames::IntoIter: Clone + Send + 'static,
    Finish: Symbol + Send,
    Writer: io::Write + Send + 'static,
{
    message: UpdateStrategy<State>,
    frames: Frames,
    finish: Finish,
    interval: Option<Duration>,
    writer: Writer,
    is_tty: bool,
}

impl<State>
    Spinner<
        State,
        &'static &'static str,
        <&'static [&'static str] as IntoIterator>::IntoIter,
        (&'static str, AsciiColor),
    >
where
    State: Send + 'static,
{
    /// Create a new spinner with the given message, writing to stdout.
    ///
    /// Automatically detects whether stdout is a terminal. When it isn't
    /// (e.g. output is piped or redirected), the spinner skips animation
    /// and ANSI codes, printing plain text instead.
    #[must_use]
    pub fn new(message: impl Into<UpdateStrategy<State>>) -> Self {
        Spinner {
            message: message.into(),
            frames: FRAMES.into_iter(),
            finish: (DEFAULT_FINISH, AsciiColor::Green),
            interval: None,
            is_tty: io::stdout().is_terminal(),
            writer: io::stdout(),
        }
    }
}

impl<State, Writer>
    Spinner<
        State,
        &'static &'static str,
        <&'static [&'static str] as IntoIterator>::IntoIter,
        (&'static str, AsciiColor),
        Writer,
    >
where
    State: Send + 'static,
    Writer: io::Write + Send + 'static,
{
    /// Create a new spinner with the given message and a custom writer.
    ///
    /// `is_tty` defaults to `false` for custom writers. Use
    /// [`Spinner::with_writer_tty`] if you need to override this.
    pub fn with_writer(message: impl Into<UpdateStrategy<State>>, writer: Writer) -> Self {
        Spinner {
            message: message.into(),
            frames: FRAMES.into_iter(),
            finish: (DEFAULT_FINISH, AsciiColor::Green),
            interval: None,
            is_tty: false,
            writer,
        }
    }

    /// Create a new spinner with the given message, a custom writer, and
    /// an explicit TTY flag controlling whether ANSI codes are emitted.
    pub fn with_writer_tty(
        message: impl Into<UpdateStrategy<State>>,
        writer: Writer,
        is_tty: bool,
    ) -> Self {
        Spinner {
            message: message.into(),
            frames: FRAMES.into_iter(),
            finish: (DEFAULT_FINISH, AsciiColor::Green),
            interval: None,
            is_tty,
            writer,
        }
    }
}
impl<State, Display, Frames, Finish, Writer> Spinner<State, Display, Frames, Finish, Writer>
where
    State: Send + 'static,
    Display: ::std::fmt::Display,
    Frames: IntoIterator<Item = Display>,
    Frames::IntoIter: Clone + Send + 'static,
    Finish: Symbol + Send,
    Writer: io::Write + Send + 'static,
{
    /// Override the default spinner frames, finish symbol, and/or interval.
    pub fn with_frames<D, I, S>(self, frames: I, finish: S) -> Spinner<State, D, I, S, Writer>
    where
        D: ::std::fmt::Display,
        I: IntoIterator<Item = D>,
        I::IntoIter: Clone + Send,
        S: Symbol + Send + 'static,
    {
        Spinner {
            frames,
            finish,
            message: self.message,
            interval: self.interval,
            writer: self.writer,
            is_tty: self.is_tty,
        }
    }
}

impl<State, Display, Frames, Finish, Writer> Spinner<State, Display, Frames, Finish, Writer>
where
    State: Send + 'static,
    Display: ::std::fmt::Display,
    Frames: IntoIterator<Item = Display>,
    Frames::IntoIter: Clone + Send + 'static,
    Finish: Symbol + Send,
    Writer: io::Write + Send + 'static,
{
    /// Set a update interval for the spinner animation. When `None` (the default), the spinner
    /// will not animate and will instead print the first frame as a static symbol - needs
    /// to be updated manually afterwards using `update()`.
    pub fn with_interval(mut self, interval: Duration) -> Self {
        self.interval = Some(interval);
        self
    }

    /// Spawn the background animation thread and return a handle.
    ///
    /// When the output is not a TTY, no background thread is spawned and
    /// the animation is skipped entirely.
    #[must_use]
    pub fn start(self) -> SpinnerHandle<State, Display, Frames::IntoIter, Finish, Writer> {
        let stop_flag = Arc::new(AtomicBool::new(false));
        let message = Arc::new(Mutex::new(self.message));
        let writer: Arc<Mutex<Writer>> = Arc::new(Mutex::new(self.writer));
        let is_tty = self.is_tty;
        let last_frame = Arc::new(AtomicUsize::new(0));

        let frames_iter = self.frames.into_iter();

        let thread = if is_tty && self.interval.is_some() {
            let t_frames = frames_iter.clone();
            let t_interval = self.interval.unwrap_or(Duration::from_millis(80));
            let t_stop = Arc::clone(&stop_flag);
            let t_msg = Arc::clone(&message);
            let t_writer = Arc::clone(&writer);
            let t_last_frame = Arc::clone(&last_frame);

            Some(thread::spawn(move || {
                spin_loop(
                    t_frames,
                    t_interval,
                    &t_stop,
                    &t_msg,
                    &t_writer,
                    t_last_frame,
                )
            }))
        } else {
            // Mark as already stopped so drop() is a no-op.
            stop_flag.store(true, Ordering::Release);
            None
        };

        SpinnerHandle {
            finish: self.finish,
            stop_flag,
            message,
            writer,
            thread: Mutex::new(thread),
            is_tty,
            frames: frames_iter,
            last_frame,
        }
    }
}

macro_rules! into_inner {
    ($self:expr) => {{
        let md = ManuallyDrop::new($self);
        unsafe {
            let _finish = std::ptr::read(&md.finish);
            let _stop_flag = std::ptr::read(&md.stop_flag);
            let _message = std::ptr::read(&md.message);
            let _thread = std::ptr::read(&md.thread);
            let _frames = std::ptr::read(&md.frames);
            let _last_frame = std::ptr::read(&md.last_frame);
            drop((_finish, _stop_flag, _message, _thread, _frames, _last_frame));
        }

        let writer = Arc::try_unwrap(unsafe { ::std::ptr::read(&md.writer) })
            .ok()
            .and_then(|mutex| mutex.into_inner().ok())
            .expect("Failed to unwrap writer Arc<Mutex>: multiple references exist");
        ::std::mem::forget(md);
        writer
    }};
}
/// Handle for controlling a running spinner.
///
/// This is the same handle as the [`nanospinner::SpinnerHandle`] but rather than having heap allocated dynamic types,
/// its realies entierly on generic type parameters, thus allowing it to be used in `no_std` environments without the need for `alloc`,
/// and also better optimization opportunities for the compiler. The tradeoff is that it is less ergonomic to use, and requires more and strict type annotations.
///
/// Returned by [`Spinner::start`]. Use [`SpinnerHandle::update`] to change
/// the message mid-spin, and finalize with [`SpinnerHandle::success`] or
/// [`SpinnerHandle::fail`]. Dropping the handle will automatically stop
/// the background thread.
pub struct SpinnerHandle<State, Display, Frames, Finish, Writer>
where
    Display: ::std::fmt::Display,
    Frames: Iterator<Item = Display> + Clone,
    Finish: Symbol,
    Writer: io::Write + 'static,
{
    finish: Finish,
    stop_flag: Arc<AtomicBool>,
    message: Arc<Mutex<UpdateStrategy<State>>>,
    writer: Arc<Mutex<Writer>>,
    thread: Mutex<Option<JoinHandle<()>>>,
    is_tty: bool,
    frames: Frames,
    last_frame: Arc<AtomicUsize>,
}

impl<State, Display, Frames, Finish, Writer> SpinnerHandle<State, Display, Frames, Finish, Writer>
where
    Display: ::std::fmt::Display,
    Frames: Iterator<Item = Display> + Clone,
    Finish: Symbol,
    Writer: io::Write + 'static,
{
    /// Update the spinner message while it's running.
    ///
    /// # Panics
    /// Panics if the internal mutex is poisoned.
    pub fn update(&self, message: impl Into<String>) {
        *self.message.lock().unwrap() = UpdateStrategy::Message(message.into());
    }

    /// Stop the spinner and clear the line.
    ///
    /// # Panics
    /// Panics if the internal mutex is poisoned.
    pub fn stop(self) {
        self.shutdown();
    }

    fn shutdown(&self) {
        self.stop_flag.store(true, Ordering::Release);
        let thread = self.thread.lock().unwrap().take();
        if let Some(thread) = thread {
            let _ = thread.join();
            if self.is_tty {
                if let Ok(mut w) = self.writer.lock() {
                    let _ = write!(w, "\r{CLEAR_LINE}");
                    let _ = w.flush();
                }
            }
        }
    }

    /// Stop the spinner and print the symbol set at construction with the current message.
    ///
    /// # Panics
    /// Panics if the internal mutex is poisoned.
    pub fn finish(self) -> Writer {
        let mut msg = self.message.lock().unwrap();
        self.shutdown();
        let output = match &mut *msg {
            UpdateStrategy::Message(msg) => {
                if self.is_tty {
                    format_finalize(&self.finish, &msg)
                } else {
                    format_finalize_plain(self.finish.symbol(), &msg)
                }
            }
            UpdateStrategy::Callback { state, callback } => {
                if self.is_tty {
                    format_finalize(&self.finish, &(callback)(state))
                } else {
                    format_finalize_plain(self.finish.symbol(), &(callback)(state))
                }
            }
        };
        drop(msg);
        let mut w = self.writer.lock().unwrap();
        write!(w, "{output}").unwrap();
        w.flush().unwrap();
        drop(w);

        into_inner!(self)
    }

    /// Stop the spinner and print the symbol set at construction with a replacement message.
    ///
    /// # Panics
    /// Panics if the internal mutex is poisoned.
    pub fn finish_with(self, message: impl Into<String>) -> Writer {
        self.shutdown();
        let msg = message.into();
        let output = if self.is_tty {
            format_finalize(&self.finish, &msg)
        } else {
            format_finalize_plain(self.finish.symbol(), &msg)
        };
        let mut w = self.writer.lock().unwrap();
        write!(w, "{output}").unwrap();
        w.flush().unwrap();
        drop(w);

        into_inner!(self)
    }

    /// Stop the spinner and print the given symbol with the current message.
    ///
    /// # Panics
    /// Panics if the internal mutex is poisoned.
    pub fn finish_with_symbol(self, symbol: impl Symbol) -> Writer {
        let mut msg = self.message.lock().unwrap();
        self.shutdown();
        let output = match &mut *msg {
            UpdateStrategy::Message(msg) => {
                if self.is_tty {
                    format_finalize(symbol, msg)
                } else {
                    format_finalize_plain(symbol.symbol(), msg)
                }
            }
            UpdateStrategy::Callback { state, callback } => {
                if self.is_tty {
                    format_finalize(symbol, &(callback)(state))
                } else {
                    format_finalize_plain(symbol.symbol(), &(callback)(state))
                }
            }
        };
        drop(msg);
        let mut w = self.writer.lock().unwrap();
        write!(w, "{output}").unwrap();
        w.flush().unwrap();
        drop(w);

        into_inner!(self)
    }

    /// Stop the spinner and print the given symbol with a replacement message.
    ///
    /// # Panics
    /// Panics if the internal mutex is poisoned.
    pub fn finish_with_message(self, message: impl Into<String>, symbol: impl Symbol) -> Writer {
        self.shutdown();
        let msg = message.into();
        let output = if self.is_tty {
            format_finalize(symbol, &msg)
        } else {
            format_finalize_plain(symbol.symbol(), &msg)
        };
        let mut w = self.writer.lock().unwrap();
        write!(w, "{output}").unwrap();
        w.flush().unwrap();
        drop(w);

        into_inner!(self)
    }

    /// Tick the spinner to update the frame immediately.
    pub fn tick(&self) {
        if self.thread.try_lock().map(|t| t.is_some()).unwrap_or(false) {
            // If the thread is still running, we can just return and let it update the frame on the next tick.
            return;
        }

        match &mut *self.message.lock().unwrap() {
            UpdateStrategy::Message(msg) => self.tick_with_unchecked(msg),
            UpdateStrategy::Callback { state, callback } => {
                self.tick_with_unchecked(&callback(state))
            }
        };
    }

    pub fn tick_with(&self, message: impl AsRef<str>) {
        if self.thread.try_lock().map(|t| t.is_some()).unwrap_or(false) {
            *self.message.lock().unwrap() = UpdateStrategy::Message(message.as_ref().to_string());
            return;
        }
        self.tick_with_unchecked(message.as_ref());
    }

    fn tick_with_unchecked(&self, message: &str) {
        let idx = self.last_frame.load(Ordering::Acquire);
        let (idx, frame) = self.frames.clone().enumerate().skip(idx).next().expect(
            "Last stored frame index is out of bounds for frames iterator - this is a logic bug",
        );
        self.last_frame.store(idx, Ordering::Release);
        let output = format_frame(frame, message);
        let mut w = self.writer.lock().unwrap();
        write!(w, "{output}").unwrap();
        w.flush().unwrap();
        drop(w);
    }
}

impl<State, Display, Frames, Finish, Writer> Drop
    for SpinnerHandle<State, Display, Frames, Finish, Writer>
where
    Display: ::std::fmt::Display,
    Frames: Iterator<Item = Display> + Clone,
    Finish: Symbol,
    Writer: io::Write + 'static,
{
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn spin_loop<State>(
    frames: impl Iterator<Item = impl ::std::fmt::Display> + Clone,
    interval: Duration,
    stop_flag: &Arc<AtomicBool>,
    message: &Arc<Mutex<UpdateStrategy<State>>>,
    writer: &Arc<Mutex<impl io::Write>>,
    last_frame: Arc<AtomicUsize>,
) {
    let mut frame_iter = frames.enumerate().cycle();
    while !stop_flag.load(Ordering::Acquire) {
        let mut msg = message.lock().unwrap();
        let (idx, frame) = frame_iter.next().unwrap();
        last_frame.store(idx, Ordering::Release);
        let output = match &mut *msg {
            UpdateStrategy::Message(msg) => format_frame(frame, msg),
            UpdateStrategy::Callback { state, callback } => format_frame(frame, &callback(state)),
        };
        let mut w = writer.lock().unwrap();
        write!(w, "{output}").unwrap();
        w.flush().unwrap();
        drop(w);
        thread::sleep(interval);
    }
}
