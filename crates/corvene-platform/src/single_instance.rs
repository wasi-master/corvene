//! One Corvene per data folder on Linux and macOS - GHD's
//! `app.requestSingleInstanceLock()` and `second-instance` event
//! (`main-process/main.ts`). On macOS LaunchServices already hands a second
//! launch of the same app bundle to the running one; the socket covers
//! another copy of the app or the bare binary, which would otherwise find
//! the store locked and run on a temporary one.
//!
//! The first instance listens on a Unix socket in `$XDG_RUNTIME_DIR`
//! named after the data folder, so harness instances with their own
//! `CORVENE_DATA_DIR` stay separate. A later launch connects, sends the
//! `x-corvene://` / `x-corvene-auth://` URLs from its command line (the
//! `.desktop` file's `%U`, the command line tool) or, without any, asks
//! for the window (GHD: restore, show, focus), and exits. A socket left
//! behind by a crashed instance refuses the connection and is replaced.
//!
//! Wire format: one message per line, `url <url>` or `focus`.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};

use tracing::{debug, info, warn};

/// What a later launch asked the running instance for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Message {
    Url(String),
    Focus,
}

/// The URL schemes Corvene handles (`possibleProtocols`).
pub const SCHEMES: &[&str] = &["x-corvene", "x-corvene-auth"];

/// The command line arguments that are Corvene URLs (shiftkey/desktop's
/// Linux `handlePossibleProtocolLauncherArgs`, restricted to our schemes so
/// no argument is mistaken for one).
pub fn url_arguments(args: impl IntoIterator<Item = String>) -> Vec<String> {
    args.into_iter()
        .filter(|arg| {
            SCHEMES.iter().any(|scheme| {
                arg.strip_prefix(scheme)
                    .is_some_and(|r| r.starts_with("://"))
            })
        })
        .collect()
}

/// `$XDG_RUNTIME_DIR/corvene/instance-<hash of the data folder>.sock`
/// (a per-user folder in the temp dir without a runtime dir).
pub fn socket_path() -> PathBuf {
    let data = crate::paths::app_support_dir();
    let runtime = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .filter(|d| d.is_dir())
        .unwrap_or_else(|| {
            // SAFETY: getuid has no preconditions and cannot fail
            let uid = unsafe { libc::getuid() };
            std::env::temp_dir().join(format!("corvene-{uid}"))
        });
    runtime.join("corvene").join(format!(
        "instance-{:016x}.sock",
        fnv1a(data.as_os_str().as_encoded_bytes())
    ))
}

/// FNV-1a: stable across builds, unlike `DefaultHasher`.
fn fnv1a(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |hash, &b| {
        (hash ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3)
    })
}

/// The outcome of [`claim`].
pub enum Claim {
    /// This is the first instance; serve later launches with [`Listener::serve`].
    First(Listener),
    /// A running instance took the messages; this process should exit.
    Forwarded,
}

pub struct Listener {
    listener: Option<UnixListener>,
    path: PathBuf,
}

/// Hand `urls` (or a focus request) to a running instance, else become the
/// instance. Errors creating the socket are logged and leave this process
/// running on its own, as GHD does when the lock cannot be taken.
pub fn claim(urls: &[String]) -> Claim {
    claim_at(&socket_path(), urls)
}

fn claim_at(path: &Path, urls: &[String]) -> Claim {
    if let Ok(stream) = UnixStream::connect(path) {
        match send(stream, urls) {
            Ok(()) => {
                info!(urls = urls.len(), "handed over to the running instance");
                return Claim::Forwarded;
            }
            Err(err) => warn!(%err, "the running instance did not take the launch"),
        }
    }
    // nobody answers: a stale socket from a crash, or none yet
    let _ = std::fs::remove_file(path);
    if let Some(dir) = path.parent()
        && let Err(err) = create_private_dir(dir)
    {
        warn!(%err, dir = %dir.display(), "no single-instance socket");
        return Claim::First(Listener {
            listener: None,
            path: path.to_path_buf(),
        });
    }
    match UnixListener::bind(path) {
        Ok(listener) => Claim::First(Listener {
            listener: Some(listener),
            path: path.to_path_buf(),
        }),
        // another instance started in between: hand over to it
        Err(err) if err.kind() == std::io::ErrorKind::AddrInUse => {
            match UnixStream::connect(path).and_then(|s| send(s, urls)) {
                Ok(()) => Claim::Forwarded,
                Err(_) => Claim::First(Listener {
                    listener: None,
                    path: path.to_path_buf(),
                }),
            }
        }
        Err(err) => {
            warn!(%err, "no single-instance socket");
            Claim::First(Listener {
                listener: None,
                path: path.to_path_buf(),
            })
        }
    }
}

fn create_private_dir(dir: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::DirBuilderExt;
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)
}

fn send(mut stream: UnixStream, urls: &[String]) -> std::io::Result<()> {
    let mut text = String::new();
    if urls.is_empty() {
        text.push_str("focus\n");
    }
    for url in urls {
        // a URL is one line; anything else would split the message
        if !url.contains('\n') {
            text.push_str(&format!("url {url}\n"));
        }
    }
    stream.write_all(text.as_bytes())?;
    stream.flush()
}

fn parse(line: &str) -> Option<Message> {
    match line.trim_end() {
        "focus" => Some(Message::Focus),
        other => {
            let url = other.strip_prefix("url ")?;
            url_arguments([url.to_string()]).pop().map(Message::Url)
        }
    }
}

impl Listener {
    /// Accept later launches on a background thread; each message goes to
    /// `on_message` (on that thread).
    pub fn serve(self, on_message: impl Fn(Message) + Send + 'static) {
        let Some(listener) = self.listener else {
            return;
        };
        debug!(path = %self.path.display(), "single-instance socket");
        let spawned = std::thread::Builder::new()
            .name("single-instance".into())
            .spawn(move || {
                for stream in listener.incoming().flatten() {
                    for line in BufReader::new(stream).lines().map_while(Result::ok) {
                        match parse(&line) {
                            Some(message) => on_message(message),
                            None => warn!("ignored an unknown single-instance message"),
                        }
                    }
                }
            });
        if let Err(err) = spawned {
            warn!(%err, "could not start the single-instance listener");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;
    use std::time::Duration;

    #[test]
    fn picks_corvene_urls_only() {
        let args = [
            "--hidden",
            "x-corvene://openLocalRepo/tmp",
            "https://example.com",
            "x-corvene-auth://oauth?code=1",
            "x-corveneevil://x",
        ]
        .map(String::from);
        assert_eq!(
            url_arguments(args),
            [
                "x-corvene://openLocalRepo/tmp",
                "x-corvene-auth://oauth?code=1"
            ]
        );
    }

    #[test]
    fn a_second_launch_hands_over_and_a_stale_socket_is_replaced() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("instance.sock");
        // a stale socket file nobody listens on
        drop(UnixListener::bind(&path).unwrap());
        let Claim::First(listener) = claim_at(&path, &[]) else {
            panic!("the first launch must become the instance");
        };
        let (tx, rx) = mpsc::channel();
        listener.serve(move |m| {
            let _ = tx.send(m);
        });
        let url = "x-corvene://openLocalRepo/tmp".to_string();
        assert!(matches!(
            claim_at(&path, std::slice::from_ref(&url)),
            Claim::Forwarded
        ));
        assert_eq!(
            rx.recv_timeout(Duration::from_secs(5)).unwrap(),
            Message::Url(url)
        );
        assert!(matches!(claim_at(&path, &[]), Claim::Forwarded));
        assert_eq!(
            rx.recv_timeout(Duration::from_secs(5)).unwrap(),
            Message::Focus
        );
    }

    #[test]
    fn sockets_differ_per_data_folder() {
        assert_ne!(fnv1a(b"/a"), fnv1a(b"/b"));
    }
}
