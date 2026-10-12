//! tracing setup: stderr in debug builds (Android: logcat, always), and GHD's
//! daily log file in ~/Library/Logs/Corvene
//! (`corvene_platform::desktop_file_transport`, GHD
//! `main-process/desktop-file-transport.ts`: `<UTC date>.desktop.<channel>.log`,
//! the newest 14 kept).

use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::{EnvFilter, fmt, prelude::*};

/// Log lines waiting for the writer thread; past this they are dropped
/// (a refresh logs a few dozen).
const LOG_QUEUE_LINES: usize = 4096;

pub fn init() -> Option<WorkerGuard> {
    let filter = EnvFilter::try_from_env("CORVENE_LOG").unwrap_or_else(|_| {
        // Android: every line is a write to logcat on the thread that
        // logs; a release keeps to `info` (a refresh alone logs a dozen
        // git commands at `debug`); the platform layer's frame timings,
        // one line per hundred frames, stay
        EnvFilter::new(if cfg!(target_os = "android") && !cfg!(debug_assertions) {
            "info,gpui_android=debug"
        } else {
            "info,corvene=debug"
        })
    });

    let logs_dir = corvene_platform::paths::logs_dir();
    let file_layer = std::fs::create_dir_all(&logs_dir).ok().map(|_| {
        let transport =
            corvene_platform::desktop_file_transport::DesktopFileTransport::new(&logs_dir);
        // the queue to the writer thread is allocated whole at launch:
        // tracing-appender's default of 128 000 lines held about 4 MB
        let (writer, guard) = tracing_appender::non_blocking::NonBlockingBuilder::default()
            .buffered_lines_limit(LOG_QUEUE_LINES)
            .finish(transport);
        (fmt::layer().with_ansi(false).with_writer(writer), guard)
    });

    let registry = tracing_subscriber::registry().with(filter);
    // Android has no stderr to read: logcat takes its place, in every build
    #[cfg(target_os = "android")]
    let registry = registry.with(
        fmt::layer()
            .with_ansi(false)
            .without_time()
            .with_writer(logcat::Writer::default),
    );
    match file_layer {
        Some((layer, guard)) => {
            registry
                .with(layer)
                .with(
                    (cfg!(debug_assertions) && !cfg!(target_os = "android"))
                        .then(|| fmt::layer().with_writer(std::io::stderr)),
                )
                .init();
            Some(guard)
        }
        None => {
            registry
                .with(
                    (!cfg!(target_os = "android"))
                        .then(|| fmt::layer().with_writer(std::io::stderr)),
                )
                .init();
            None
        }
    }
}

/// Lines for `adb logcat -s corvene`.
#[cfg(target_os = "android")]
mod logcat {
    use std::ffi::{CString, c_char, c_int};
    use std::io;

    const ANDROID_LOG_INFO: c_int = 4;

    #[link(name = "log")]
    unsafe extern "C" {
        fn __android_log_write(prio: c_int, tag: *const c_char, text: *const c_char) -> c_int;
    }

    /// Collects one formatted event and writes it when dropped.
    #[derive(Default)]
    pub struct Writer(Vec<u8>);

    impl io::Write for Writer {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.0.extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    impl Drop for Writer {
        fn drop(&mut self) {
            let mut text = std::mem::take(&mut self.0);
            text.retain(|byte| *byte != 0);
            let Ok(text) = CString::new(text) else {
                return;
            };
            // SAFETY: both strings are NUL-terminated and live for the call
            unsafe { __android_log_write(ANDROID_LOG_INFO, c"corvene".as_ptr(), text.as_ptr()) };
        }
    }
}
