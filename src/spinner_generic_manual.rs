use ::std::{cell::UnsafeCell, mem::ManuallyDrop};
use std::io::{self, IsTerminal};

use crate::{
    shared::{
        CLEAR_LINE, DEFAULT_FINISH, FRAMES, format_finalize, format_finalize_plain, format_frame,
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
pub struct Spinner<State, Display, Frames, Finish, Writer>
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
    writer: Writer,
    is_tty: bool,
}

impl<State>
    Spinner<
        State,
        &'static &'static str,
        <&'static [&'static str] as IntoIterator>::IntoIter,
        (&'static str, AsciiColor),
        io::Stdout,
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
        Self {
            message: message.into(),
            frames: FRAMES.into_iter(),
            finish: (DEFAULT_FINISH, AsciiColor::Green),
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
        Self {
            message: message.into(),
            frames: FRAMES.into_iter(),
            finish: (DEFAULT_FINISH, AsciiColor::Green),
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
        Self {
            message: message.into(),
            frames: FRAMES.into_iter(),
            finish: (DEFAULT_FINISH, AsciiColor::Green),
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
    pub fn with_frames(
        self,
        frames: Frames,
        finish: impl Into<Finish>,
    ) -> Spinner<State, Display, Frames, Finish, Writer>
where {
        Self {
            frames,
            finish: finish.into(),
            message: self.message,
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
    /// Spawn the background animation thread and return a handle.
    ///
    /// When the output is not a TTY, no background thread is spawned and
    /// the animation is skipped entirely.
    #[must_use]
    pub fn start(self) -> SpinnerHandle<State, Display, Finish, Writer> {
        let message = UnsafeCell::new(self.message);
        let writer = UnsafeCell::new(self.writer);
        let last_frame = UnsafeCell::new(0);
        let is_tty = self.is_tty;

        let frames_iter = self.frames.into_iter();

        // Mark as already stopped so drop() is a no-op.

        SpinnerHandle {
            finish: self.finish,
            message,
            writer,
            is_tty,
            frames: frames_iter.collect(),
            last_frame,
        }
    }
}

macro_rules! into_inner {
    ($self:expr) => {{
        let md = ManuallyDrop::new($self);
        unsafe {
            let _finish = ::std::ptr::read(&md.finish);
            let _message = ::std::ptr::read(&md.message);
            let _frames = ::std::ptr::read(&md.frames);
            let _last_frame = ::std::ptr::read(&md.last_frame);
            drop((_finish, _message, _frames, _last_frame));
        }

        let writer = unsafe { ::std::ptr::read(&md.writer) };
        let writer = writer.into_inner();
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
pub struct SpinnerHandle<State, Display, Finish, Writer>
where
    Display: ::std::fmt::Display,
    Finish: Symbol,
    Writer: io::Write + 'static,
{
    finish: Finish,
    message: UnsafeCell<UpdateStrategy<State>>,
    writer: UnsafeCell<Writer>,
    is_tty: bool,
    frames: Vec<Display>,
    last_frame: UnsafeCell<usize>,
}

impl<State, Display, Finish, Writer> SpinnerHandle<State, Display, Finish, Writer>
where
    Display: ::std::fmt::Display,
    Finish: Symbol,
    Writer: io::Write + 'static,
{
    #[inline(always)]
    fn writer_mut(&self) -> &mut Writer {
        unsafe { &mut *self.writer.get() }
    }

    #[inline]
    pub unsafe fn writer(&self) -> &mut Writer {
        unsafe { &mut *self.writer.get() }
    }

    #[inline]
    pub fn with_writer<F>(&mut self, f: F)
    where
        F: FnOnce(&mut Writer),
    {
        f(self.writer_mut());
    }

    /// Update the spinner message while it's running.
    ///
    /// # Panics
    /// Panics if the internal mutex is poisoned.
    pub fn update(&self, message: impl Into<String>) {
        unsafe { *self.message.get() = UpdateStrategy::Message(message.into()) };
    }

    /// Stop the spinner and clear the line.
    ///
    /// # Panics
    /// Panics if the internal mutex is poisoned.
    pub fn stop(self) {
        self.shutdown();
    }

    fn shutdown(&self) {
        if self.is_tty {
            let writer = self.writer_mut();
            let _ = write!(writer, "\r{CLEAR_LINE}");
            let _ = writer.flush();
        }
    }

    /// Stop the spinner and print the symbol set at construction with the current message.
    ///
    /// # Panics
    /// Panics if the internal mutex is poisoned.
    pub fn finish(self) -> Writer {
        self.shutdown();
        let msg = unsafe { &mut *self.message.get() };
        let output = match msg {
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
        let w = self.writer_mut();
        write!(w, "{output}").unwrap();
        w.flush().unwrap();

        into_inner!(self)
    }

    /// Stop the spinner and print the symbol set at construction with a replacement message.
    ///
    /// # Panics
    /// Panics if the internal mutex is poisoned.
    pub fn finish_with(self, message: impl ::std::fmt::Display) -> Writer {
        self.shutdown();
        let output = if self.is_tty {
            format_finalize(&self.finish, message)
        } else {
            format_finalize_plain(self.finish.symbol(), message)
        };
        let w = self.writer_mut();
        write!(w, "{output}").unwrap();
        w.flush().unwrap();

        into_inner!(self)
    }

    /// Stop the spinner and print the given symbol with the current message.
    ///
    /// # Panics
    /// Panics if the internal mutex is poisoned.
    pub fn finish_with_symbol(self, symbol: impl Symbol) -> Writer {
        self.shutdown();
        let msg = unsafe { &mut *self.message.get() };
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
        let w = self.writer_mut();
        write!(w, "{output}").unwrap();
        w.flush().unwrap();

        into_inner!(self)
    }

    /// Stop the spinner and print the given symbol with a replacement message.
    ///
    /// # Panics
    /// Panics if the internal mutex is poisoned.
    pub fn finish_with_message(
        self,
        message: impl ::std::fmt::Display,
        symbol: impl Symbol,
    ) -> Writer {
        self.shutdown();

        let output = if self.is_tty {
            format_finalize(symbol, message)
        } else {
            format_finalize_plain(symbol.symbol(), message)
        };
        let w = self.writer_mut();
        write!(w, "{output}").unwrap();
        w.flush().unwrap();

        into_inner!(self)
    }

    /// Tick the spinner to update the frame immediately.
    pub fn tick(&self) {
        match unsafe { &mut *self.message.get() } {
            UpdateStrategy::Message(msg) => self.tick_with_unchecked(msg),
            UpdateStrategy::Callback { state, callback } => {
                self.tick_with_unchecked(&callback(state))
            }
        };
    }

    pub fn tick_with(&self, message: impl AsRef<str>) {
        self.tick_with_unchecked(message.as_ref());
    }

    fn tick_with_unchecked(&self, message: &str) {
        let idx = unsafe { &mut *self.last_frame.get() };
        let last_idx = *idx;
        *idx = (*idx + 1) % self.frames.len();

        let frame = &self.frames[last_idx];

        let output = format_frame(frame, message);
        let w = self.writer_mut();
        write!(w, "{output}").unwrap();
        w.flush().unwrap();
    }
}

impl<State, Display, Finish, Writer> Drop for SpinnerHandle<State, Display, Finish, Writer>
where
    Display: ::std::fmt::Display,
    Finish: Symbol,
    Writer: io::Write + 'static,
{
    fn drop(&mut self) {
        self.shutdown();
    }
}
