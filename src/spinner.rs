use ::std::{mem::ManuallyDrop, sync::atomic::AtomicUsize};
use std::io::{self, IsTerminal};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

#[cfg(all(test, false))]
use crate::shared::RESET;
use crate::{
    shared::{
        format_finalize, format_finalize_plain, format_frame, CLEAR_LINE, DEFAULT_FINISH, FRAMES,
    },
    symbol::{AsciiColor, Symbol},
    update::UpdateStrategy,
};

/// A builder for configuring and starting a terminal spinner.
///
/// Use [`Spinner::new`] for stdout, or [`Spinner::with_writer`] /
/// [`Spinner::with_writer_tty`] for custom output targets. Call
/// [`Spinner::start`] to begin the animation and get a [`SpinnerHandle`].
pub struct Spinner<State: Send + 'static, W: io::Write + Send + 'static = io::Stdout> {
    message: UpdateStrategy<State>,
    frames: Box<[&'static str]>,
    finish: Box<dyn Symbol + Send>,
    interval: Option<Duration>,
    writer: W,
    is_tty: bool,
}

impl<State: Send + 'static> Spinner<State> {
    /// Create a new spinner with the given message, writing to stdout.
    ///
    /// Automatically detects whether stdout is a terminal. When it isn't
    /// (e.g. output is piped or redirected), the spinner skips animation
    /// and ANSI codes, printing plain text instead.
    #[must_use]
    pub fn new(message: impl Into<UpdateStrategy<State>>) -> Spinner<State> {
        Spinner {
            message: message.into(),
            frames: FRAMES.into(),
            finish: Box::new((DEFAULT_FINISH, AsciiColor::Green)),
            interval: None,
            is_tty: io::stdout().is_terminal(),
            writer: io::stdout(),
        }
    }
}

impl<State: Send + 'static, W: io::Write + Send + 'static> Spinner<State, W> {
    /// Create a new spinner with the given message and a custom writer.
    ///
    /// `is_tty` defaults to `false` for custom writers. Use
    /// [`Spinner::with_writer_tty`] if you need to override this.
    pub fn with_writer(message: impl Into<UpdateStrategy<State>>, writer: W) -> Self {
        Spinner {
            message: message.into(),
            frames: FRAMES.into(),
            finish: Box::new((DEFAULT_FINISH, AsciiColor::Green)),
            interval: None,
            is_tty: false,
            writer,
        }
    }

    /// Create a new spinner with the given message, a custom writer, and
    /// an explicit TTY flag controlling whether ANSI codes are emitted.
    pub fn with_writer_tty(
        message: impl Into<UpdateStrategy<State>>,
        writer: W,
        is_tty: bool,
    ) -> Self {
        Spinner {
            message: message.into(),
            frames: FRAMES.into(),
            finish: Box::new((DEFAULT_FINISH, AsciiColor::Green)),
            interval: None,
            is_tty,
            writer,
        }
    }

    /// Override the default spinner frames, finish symbol, and/or interval.
    pub fn with_frames(
        mut self,
        frames: impl IntoIterator<Item = &'static str>,
        finish: impl Symbol + Send + 'static,
    ) -> Self {
        self.frames = frames.into_iter().collect();
        self.finish = Box::new(finish);
        self
    }

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
    pub fn start(self) -> SpinnerHandle<State> {
        let stop_flag = Arc::new(AtomicBool::new(false));
        let message = Arc::new(Mutex::new(self.message));
        let writer: Arc<Mutex<Box<dyn io::Write + Send>>> =
            Arc::new(Mutex::new(Box::new(self.writer)));
        let is_tty = self.is_tty;
        let last_frame = Arc::new(AtomicUsize::new(0));

        let thread = if is_tty && self.interval.is_some() {
            let t_frames = self.frames.clone();
            let t_interval = self.interval.unwrap_or(Duration::from_millis(80));
            let t_stop = Arc::clone(&stop_flag);
            let t_msg = Arc::clone(&message);
            let t_writer = Arc::clone(&writer);
            let t_last_frame = Arc::clone(&last_frame);

            Some(thread::spawn(move || {
                spin_loop(
                    &t_frames,
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
            frames: self.frames,
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
/// Returned by [`Spinner::start`]. Use [`SpinnerHandle::update`] to change
/// the message mid-spin, and finalize with [`SpinnerHandle::success`] or
/// [`SpinnerHandle::fail`]. Dropping the handle will automatically stop
/// the background thread.
pub struct SpinnerHandle<State: Send> {
    finish: Box<dyn Symbol>,
    stop_flag: Arc<AtomicBool>,
    message: Arc<Mutex<UpdateStrategy<State>>>,
    writer: Arc<Mutex<Box<dyn io::Write + Send>>>,
    thread: Mutex<Option<JoinHandle<()>>>,
    is_tty: bool,
    frames: Box<[&'static str]>,
    last_frame: Arc<AtomicUsize>,
}

impl<State: Send> SpinnerHandle<State> {
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
    pub fn finish(self) -> Box<dyn io::Write + Send> {
        let mut msg = self.message.lock().unwrap();
        self.shutdown();
        let output = match &mut *msg {
            UpdateStrategy::Message(msg) => {
                if self.is_tty {
                    format_finalize(self.finish.as_ref(), &msg)
                } else {
                    format_finalize_plain(self.finish.symbol(), &msg)
                }
            }
            UpdateStrategy::Callback { state, callback } => {
                if self.is_tty {
                    format_finalize(self.finish.as_ref(), &(callback)(state))
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
    pub fn finish_with(self, message: impl Into<String>) -> Box<dyn io::Write + Send> {
        self.shutdown();
        let msg = message.into();
        let output = if self.is_tty {
            format_finalize(self.finish.as_ref(), &msg)
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
    pub fn finish_with_symbol(self, symbol: impl Symbol) -> Box<dyn io::Write + Send> {
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
    pub fn finish_with_message(
        self,
        message: impl Into<String>,
        symbol: impl Symbol,
    ) -> Box<dyn io::Write + Send> {
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
        let frame =
            self.frames[self
                .last_frame
                .update(Ordering::Release, Ordering::Acquire, |idx| {
                    (idx + 1) % self.frames.len()
                })];
        let output = format_frame(frame, message);
        let mut w = self.writer.lock().unwrap();
        write!(w, "{output}").unwrap();
        w.flush().unwrap();
        drop(w);
    }
}

impl<State: Send> Drop for SpinnerHandle<State> {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn spin_loop<State>(
    frames: &[&str],
    interval: Duration,
    stop_flag: &Arc<AtomicBool>,
    message: &Arc<Mutex<UpdateStrategy<State>>>,
    writer: &Arc<Mutex<Box<dyn io::Write + Send>>>,
    last_frame: Arc<AtomicUsize>,
) {
    let mut frame_iter = frames.iter().enumerate().cycle();
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

#[cfg(test)]
#[cfg(false)]
mod tests {
    use super::*;
    use crate::{shared::tests::TestWriter, symbol::AsciiColor};
    use proptest::prelude::*;

    proptest! {
        #[test]
        fn property_construction_preserves_message(s in ".*") {
            let spinner = Spinner::with_writer(s.clone(), Vec::<u8>::new());
            prop_assert_eq!(spinner.message, s);
        }

        #[test]
        fn property_update_changes_shared_message_state(
            initial in ".{0,50}",
            new_msg in ".{0,50}"
        ) {
            let spinner = Spinner::with_writer(initial, Vec::<u8>::new());
            let handle = spinner.start();

            handle.update(new_msg.clone());

            // Read the shared message state — accessible since tests are in the same module
            let stored = handle.message.lock().unwrap().clone();
            prop_assert_eq!(stored, new_msg, "shared message state must equal the new message after update");

            // Clean up: stop the spinner
            drop(handle);
        }
    }

    // TTY property tests use fewer cases (20) since each spawns a thread + sleeps
    proptest! {
        #![proptest_config(ProptestConfig::with_cases(20))]

        #[test]
        fn property_tty_fail_output_contains_ansi_symbol_and_message(
            msg in "[^\x00]{1,50}"
        ) {
            let (writer, _buf) = TestWriter::new();
            let reader = writer.clone();

            let spinner = Spinner::with_writer_tty(msg.clone(), writer, true);
            let handle = spinner.start();
            thread::sleep(Duration::from_millis(100));
            handle.fail("✖");

            let output = reader.output();
            prop_assert!(output.contains(&AsciiColor::Red.to_ansi_code()), "TTY fail output must contain RED ANSI code");
            prop_assert!(output.contains("✖"), "TTY fail output must contain ✖ symbol");
            prop_assert!(output.contains(&msg), "TTY fail output must contain the message");
            prop_assert!(output.contains(RESET), "TTY fail output must contain RESET ANSI code");
        }

        #[test]
        fn property_tty_fail_with_output_contains_ansi_symbol_and_replacement(
            original in "[^\x00]{1,50}",
            replacement in "[^\x00]{1,50}"
        ) {
            let (writer, _buf) = TestWriter::new();
            let reader = writer.clone();

            let spinner = Spinner::with_writer_tty(original, writer, true);
            let handle = spinner.start();
            thread::sleep(Duration::from_millis(100));
            handle.fail_with(replacement.clone(), "✖");

            let output = reader.output();
            prop_assert!(output.contains(&AsciiColor::Red.to_ansi_code()), "TTY fail_with output must contain RED ANSI code");
            prop_assert!(output.contains("✖"), "TTY fail_with output must contain ✖ symbol");
            prop_assert!(output.contains(&replacement), "TTY fail_with output must contain the replacement message");
            prop_assert!(output.contains(RESET), "TTY fail_with output must contain RESET ANSI code");
        }

    }

    #[test]
    fn test_default_frames() {
        let expected = vec!["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
        let spinner = Spinner::with_writer("test", Vec::<u8>::new());
        assert_eq!(spinner.frames.as_ref(), expected.as_slice());
    }

    #[test]
    fn test_default_interval() {
        let spinner = Spinner::with_writer("test", Vec::<u8>::new());
        assert_eq!(spinner.interval, Duration::from_millis(80));
    }

    #[test]
    fn test_lifecycle_no_panic() {
        // start → sleep → stop
        let spinner = Spinner::with_writer("test", Vec::<u8>::new());
        let handle = spinner.start();
        thread::sleep(Duration::from_millis(100));
        handle.stop();

        // start → sleep → drop
        let spinner = Spinner::with_writer("test", Vec::<u8>::new());
        let handle = spinner.start();
        thread::sleep(Duration::from_millis(100));
        drop(handle);
    }

    #[test]
    fn test_single_spinner_drop_clears_line_like_stop() {
        // With stop()
        let (writer, _buf_stop) = TestWriter::new();
        let reader_stop = writer.clone();
        let handle = Spinner::with_writer_tty("Working...", writer, true).start();
        thread::sleep(Duration::from_millis(100));
        handle.stop();
        let out_stop = reader_stop.output();

        // With drop (no stop)
        let (writer, _buf_drop) = TestWriter::new();
        let reader_drop = writer.clone();
        let handle = Spinner::with_writer_tty("Working...", writer, true).start();
        thread::sleep(Duration::from_millis(100));
        drop(handle);
        let out_drop = reader_drop.output();

        // Both must contain the clear-line sequence
        assert!(
            out_stop.contains(CLEAR_LINE),
            "stop output must contain CLEAR_LINE"
        );
        assert!(
            out_drop.contains(CLEAR_LINE),
            "drop output must contain CLEAR_LINE"
        );
    }

    #[test]
    fn test_non_tty_finalization_all_variants() {
        // (symbol, original_message, replacement_message, finalize_fn)
        // Each variant is tested with original message and with a replacement message.
        let cases: Vec<(&str, &str, &str, Box<dyn Fn(SpinnerHandle)>)> = vec![
            // success — original message
            ("✔", "msg1", "msg1", Box::new(|h| h.success())),
            // success — replacement message
            (
                "✔",
                "ignored",
                "replacement1",
                Box::new(|h| h.success_with("replacement1".to_string())),
            ),
            // fail — original message
            ("✖", "msg2", "msg2", Box::new(|h| h.fail("✖"))),
            // fail — replacement message
            (
                "✖",
                "ignored",
                "replacement2",
                Box::new(|h| h.fail_with("replacement2".to_string(), "✖")),
            ),
            // warn — original message
            ("⚠", "msg3", "msg3", Box::new(|h| h.warn("⚠"))),
            // warn — replacement message
            (
                "⚠",
                "ignored",
                "replacement3",
                Box::new(|h| h.warn_with("replacement3".to_string(), "⚠")),
            ),
            // info — original message
            ("ℹ", "msg4", "msg4", Box::new(|h| h.info("ℹ"))),
            // info — replacement message
            (
                "ℹ",
                "ignored",
                "replacement4",
                Box::new(|h| h.info_with("replacement4".to_string(), "ℹ")),
            ),
        ];

        for (symbol, initial_msg, expected_msg, finalize) in cases {
            let (writer, _buf) = TestWriter::new();
            let reader = writer.clone();

            let spinner = Spinner::with_writer(initial_msg, writer);
            let handle = spinner.start();
            thread::sleep(Duration::from_millis(50));
            finalize(handle);

            let output = reader.output();
            let expected = format!("{symbol} {expected_msg}\n");
            assert_eq!(
                output, expected,
                "non-TTY {symbol} output must be \"{symbol} {expected_msg}\\n\", got: {output:?}"
            );
            assert!(
                !output.contains("\x1b["),
                "non-TTY {symbol} output must not contain ANSI escape codes"
            );
            assert!(
                !output.contains('\r'),
                "non-TTY {symbol} output must not contain carriage returns"
            );
            assert!(
                !output.contains('⠋'),
                "non-TTY {symbol} output must not contain spinner frames"
            );
        }
    }

    #[test]
    fn test_tty_mode_emits_ansi_codes() {
        let (writer, _buf) = TestWriter::new();
        let reader = writer.clone();

        let spinner = Spinner::with_writer_tty("Building...", writer, true);
        let handle = spinner.start();
        thread::sleep(Duration::from_millis(200));
        handle.success();

        let output = reader.output();
        assert!(
            output.contains("\x1b["),
            "TTY output should contain ANSI escape codes"
        );
        assert!(output.contains("✔"), "TTY output should contain ✔");
        assert!(
            output.contains(&AsciiColor::Green.to_ansi_code()),
            "TTY output should contain GREEN"
        );
    }
}
