//! ssh's questions, answered in the app (desktop/desktop#16542, #22880,
//! #12216, #17284, #17885).
//!
//! GitHub Desktop runs a loopback server its askpass trampoline talks to,
//! with a per-process token (`lib/trampoline/trampoline-server.ts`,
//! `trampoline-askpass-handler.ts`) and shows `AddSSHHost`, `SSHKeyPassphrase`
//! and `SSHUserPassword` (`ui/ssh/`). So does this module: ssh runs this
//! binary as `SSH_ASKPASS`, [`crate::askpass::ask_app`] connects here and
//! blocks until the dialog answers.
//!
//! GHD keeps a secret once the git operation succeeds and deletes a stored
//! one when it fails to authenticate (`lib/ssh/ssh-secret-storage.ts`).
//! Corvene stores it when the dialog is confirmed and takes a second question
//! from the same ssh process as the answer having been wrong, on Windows
//! (no parent process id in std) a stored secret stays until replaced.
//!
//! Deviations: `1115-ssh-security-key-notice` shows a notice while ssh waits
//! for a touch (GHD's ssh just waits) and `1116-ssh-signing-prompt` gives a
//! signing `ssh-keygen` the same dialogs (GHD's commit has no askpass).

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use corvene_git::AddSshHostInfo;

use crate::Dispatcher;
use crate::askpass::{ReplyTo, SshPrompt, classify_ssh_prompt};
use crate::host::{AsyncCtx, Host};
use crate::state::Popup;

/// A question nobody answered is given up on after this long.
const ANSWER_TIMEOUT: Duration = Duration::from_secs(30 * 60);
/// Keychain "hosts" of the stored secrets, by key path or `user@host`.
const KEY_SERVICE: &str = "ssh-key";
const USER_SERVICE: &str = "ssh-user";

/// GHD accepts this host key without asking.
const GITHUB_RSA: &str = "SHA256:nThbg6kXUpJWGl7E1IGOCspRomTxdCARLviKw6E5SY8";

/// What the UI hears from the connection threads.
enum Event {
    Ask { popup: Box<Popup> },
    Presence { id: u64, key: String },
    Gone { id: u64 },
}

/// The dialog's answer: the text, and whether to remember it.
type Answer = Option<(String, bool)>;

fn pending() -> &'static Mutex<HashMap<u64, mpsc::Sender<Answer>>> {
    static PENDING: OnceLock<Mutex<HashMap<u64, mpsc::Sender<Answer>>>> = OnceLock::new();
    PENDING.get_or_init(Default::default)
}

/// The ssh process that last asked about a key or user.
fn last_asker() -> &'static Mutex<HashMap<String, u32>> {
    static LAST: OnceLock<Mutex<HashMap<String, u32>>> = OnceLock::new();
    LAST.get_or_init(Default::default)
}

fn next_id() -> u64 {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

/// A new token for this run: the operating system's randomness, as hex.
fn new_token() -> Option<String> {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).ok()?;
    Some(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

/// The token check, in the same time wherever the first difference is.
fn token_matches(given: &str, token: &str) -> bool {
    given.len() == token.len()
        && given
            .bytes()
            .zip(token.bytes())
            .fold(0u8, |diff, (a, b)| diff | (a ^ b))
            == 0
}

/// Whether asker `parent` already asked about `subject`: its answer was
/// wrong. An unknown parent (0) never counts.
fn asked_before(subject: &str, parent: u32) -> bool {
    let Ok(mut last) = last_asker().lock() else {
        return false;
    };
    let before = last.insert(subject.to_string(), parent);
    parent != 0 && before == Some(parent)
}

impl Dispatcher {
    /// Start listening and tell `corvene_git` where remote commands reach it.
    pub fn serve_ssh_prompts(cx: &mut dyn Host) {
        let Some(token) = new_token() else {
            tracing::warn!("ssh prompts: no random token");
            return;
        };
        let listener = match TcpListener::bind(("127.0.0.1", 0)) {
            Ok(listener) => listener,
            Err(err) => {
                tracing::warn!("ssh prompts: {err}");
                return;
            }
        };
        let Ok(port) = listener.local_addr().map(|addr| addr.port()) else {
            return;
        };
        corvene_git::set_ssh_prompt_server(port, token.clone());
        let (events, inbox) = async_channel::unbounded::<Event>();
        let spawned = std::thread::Builder::new()
            .name("ssh-prompts".into())
            .spawn(move || {
                for stream in listener.incoming().flatten() {
                    let events = events.clone();
                    let token = token.clone();
                    let _ = std::thread::Builder::new()
                        .name("ssh-prompt".into())
                        .spawn(move || serve_connection(stream, &token, &events));
                }
            });
        if let Err(err) = spawned {
            tracing::warn!("ssh prompts: {err}");
            return;
        }
        cx.spawn(async move |cx: &mut AsyncCtx| {
            while let Ok(event) = inbox.recv().await {
                cx.update(|cx| Self::on_ssh_prompt_event(event, cx));
            }
        })
        .detach();
    }

    fn on_ssh_prompt_event(event: Event, cx: &mut dyn Host) {
        match event {
            Event::Ask { popup } => Self::show_popup(*popup, cx),
            Event::Presence { id, key } => {
                let on = Self::state(cx)
                    .read(cx)
                    .flags
                    .bool(crate::flags::ids::SSH_SECURITY_KEY_NOTICE);
                if on {
                    Self::set_banner(crate::mco::Banner::SecurityKeyTouch { id, key }, cx);
                }
            }
            Event::Gone { id } => {
                Self::close_ssh_question(id, cx);
                let touching = matches!(
                    Self::state(cx).read(cx).banner.as_ref(),
                    Some(crate::mco::Banner::SecurityKeyTouch { id: shown, .. }) if *shown == id
                );
                if touching {
                    Self::clear_banner(cx);
                }
            }
        }
    }

    fn close_ssh_question(id: u64, cx: &mut dyn Host) {
        Self::close_popups_where(
            |popup| {
                matches!(
                    popup,
                    Popup::AddSshHost { request, .. } | Popup::SshSecret { request, .. }
                        if *request == id
                )
            },
            cx,
        );
    }

    /// A dialog's answer to question `request` (`None`: cancelled).
    pub fn answer_ssh_prompt(request: u64, answer: Option<(String, bool)>, cx: &mut dyn Host) {
        Self::close_ssh_question(request, cx);
        if let Ok(mut pending) = pending().lock()
            && let Some(sender) = pending.remove(&request)
        {
            let _ = sender.send(answer);
        }
    }
}

/// One helper process: `<token>\n<parent pid>\n<no ui>\n<prompt>\0`, answered
/// with `y<text>` or `n` and the connection closed.
fn serve_connection(stream: TcpStream, token: &str, events: &async_channel::Sender<Event>) {
    let Ok(reader_stream) = stream.try_clone() else {
        return;
    };
    let mut reader = BufReader::new(reader_stream);
    let line = |reader: &mut BufReader<TcpStream>| {
        let mut text = String::new();
        reader.read_line(&mut text).ok()?;
        Some(text.trim_end().to_string())
    };
    let Some(given) = line(&mut reader) else {
        return;
    };
    if !token_matches(&given, token) {
        tracing::warn!("ssh prompts: a connection without the token");
        return;
    }
    let parent = line(&mut reader)
        .and_then(|parent| parent.parse::<u32>().ok())
        .unwrap_or(0);
    let no_ui = line(&mut reader).is_some_and(|no_ui| no_ui == "1");
    let mut prompt = Vec::new();
    if reader.read_until(0, &mut prompt).is_err() {
        return;
    }
    let prompt = String::from_utf8_lossy(&prompt);
    let prompt = prompt.trim_end_matches('\0');
    let Some(question) = classify_ssh_prompt(prompt) else {
        return reply(stream, None);
    };
    let answer = answer_question(question, parent, no_ui, &stream, reader, events);
    reply(stream, answer);
}

fn reply(mut stream: TcpStream, answer: Option<String>) {
    let bytes = match answer {
        Some(text) => format!("y{text}"),
        None => "n".to_string(),
    };
    let _ = stream.write_all(bytes.as_bytes());
}

fn answer_question(
    question: SshPrompt,
    parent: u32,
    no_ui: bool,
    stream: &TcpStream,
    mut reader: BufReader<TcpStream>,
    events: &async_channel::Sender<Event>,
) -> Option<String> {
    let id = next_id();
    match question {
        SshPrompt::Host(info) => {
            if is_known_github_key(&info) {
                return Some("yes".into());
            }
            if no_ui {
                return None;
            }
            let popup = Popup::AddSshHost { request: id, info };
            let answer = ask_ui(id, popup, stream, events)?;
            Some(answer.0)
        }
        SshPrompt::KeyPassphrase { key } => secret(
            id,
            KEY_SERVICE,
            key,
            ReplyTo::Key,
            parent,
            no_ui,
            stream,
            events,
        ),
        SshPrompt::UserPassword { user } => secret(
            id,
            USER_SERVICE,
            user,
            ReplyTo::User,
            parent,
            no_ui,
            stream,
            events,
        ),
        SshPrompt::UserPresence { key } => {
            if !no_ui {
                let _ = events.try_send(Event::Presence { id, key });
                // ssh ends the helper when the touch came
                let mut rest = Vec::new();
                let _ = reader.read_to_end(&mut rest);
                let _ = events.try_send(Event::Gone { id });
            }
            None
        }
    }
}

fn is_known_github_key(info: &AddSshHostInfo) -> bool {
    info.host == "github.com" && info.key_type == "RSA" && info.fingerprint == GITHUB_RSA
}

/// A passphrase or password: the stored one unless it just failed, else the
/// dialog (`handleSSHKeyPassphrase`, `handleSSHUserPassword`).
#[allow(clippy::too_many_arguments)]
fn secret(
    id: u64,
    service: &str,
    subject: String,
    kind: ReplyTo,
    parent: u32,
    no_ui: bool,
    stream: &TcpStream,
    events: &async_channel::Sender<Event>,
) -> Option<String> {
    let wrong = asked_before(&subject, parent);
    let stored = corvene_platform::keychain::generic_password(service, &subject)
        .ok()
        .flatten();
    match stored {
        Some(stored) if !wrong => return Some(stored),
        Some(_) => {
            let _ = corvene_platform::keychain::delete_generic_password(service, &subject);
        }
        None => {}
    }
    if no_ui {
        return None;
    }
    let popup = Popup::SshSecret {
        request: id,
        kind,
        subject: subject.clone(),
        wrong,
    };
    let (text, remember) = ask_ui(id, popup, stream, events)?;
    if remember
        && let Err(err) =
            corvene_platform::keychain::store_generic_password(service, &subject, &text)
    {
        tracing::warn!("ssh prompts: could not store the secret: {err}");
    }
    Some(text)
}

/// Show `popup` and wait for its answer, giving up when the helper is gone
/// (the operation was cancelled) or nobody answers.
fn ask_ui(
    id: u64,
    popup: Popup,
    stream: &TcpStream,
    events: &async_channel::Sender<Event>,
) -> Answer {
    let (sender, answers) = mpsc::channel();
    pending().lock().ok()?.insert(id, sender);
    events
        .try_send(Event::Ask {
            popup: Box::new(popup),
        })
        .ok()?;
    let started = Instant::now();
    let _ = stream.set_read_timeout(Some(Duration::from_millis(100)));
    let answer = loop {
        match answers.recv_timeout(Duration::from_millis(250)) {
            Ok(answer) => break answer,
            Err(RecvTimeoutError::Disconnected) => break None,
            Err(RecvTimeoutError::Timeout) => {}
        }
        // nothing to read times out; a closed connection reads 0
        let mut probe = [0u8; 1];
        let gone = match stream.peek(&mut probe) {
            Ok(read) => read == 0,
            Err(err) => !matches!(
                err.kind(),
                std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
            ),
        };
        if gone || started.elapsed() > ANSWER_TIMEOUT {
            let _ = events.try_send(Event::Gone { id });
            break None;
        }
    };
    if let Ok(mut pending) = pending().lock() {
        pending.remove(&id);
    }
    answer
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A connection to [`serve_connection`] with `request` as what the helper
    /// writes; returns what it reads.
    fn exchange(request: String) -> Vec<u8> {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        let (events, _inbox) = async_channel::unbounded();
        let server = std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            serve_connection(stream, "secret", &events);
        });
        let mut client = TcpStream::connect(("127.0.0.1", port)).unwrap();
        client.write_all(request.as_bytes()).unwrap();
        let mut reply = Vec::new();
        client.read_to_end(&mut reply).unwrap();
        server.join().unwrap();
        reply
    }

    fn asked_id(inbox: &async_channel::Receiver<Event>) -> u64 {
        let Event::Ask { popup } = smol::block_on(inbox.recv()).unwrap() else {
            panic!("a question expected");
        };
        let Popup::AddSshHost { request, .. } = *popup else {
            panic!("the host dialog expected");
        };
        request
    }

    #[test]
    fn the_server_answers_only_with_the_token() {
        let github = format!(
            "The authenticity of host 'github.com (140.82.121.4)' can't be established.\nRSA key fingerprint is {GITHUB_RSA}.\nAre you sure?"
        );
        assert_eq!(exchange(format!("wrong\n1\n0\n{github}\0")), b"");
        assert_eq!(exchange(format!("secret\n1\n0\n{github}\0")), b"yyes");
        // not one of ssh's questions
        assert_eq!(
            exchange("secret\n1\n0\nWho goes there?\0".to_string()),
            b"n"
        );
        // a background task gets no dialog for a host it does not know
        let unknown = "The authenticity of host 'example.com (192.0.2.1)' can't be established.\nED25519 key fingerprint is SHA256:abc.\nAre you sure?";
        assert_eq!(exchange(format!("secret\n1\n1\n{unknown}\0")), b"n");
    }

    #[test]
    fn a_question_goes_to_the_ui_and_returns_its_answer() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        let (events, inbox) = async_channel::unbounded();
        let server = std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            serve_connection(stream, "secret", &events);
        });
        let mut client = TcpStream::connect(("127.0.0.1", port)).unwrap();
        let unknown = "The authenticity of host 'example.com (192.0.2.1)' can't be established.\nED25519 key fingerprint is SHA256:abc.\nAre you sure?";
        client
            .write_all(format!("secret\n1\n0\n{unknown}\0").as_bytes())
            .unwrap();
        let id = asked_id(&inbox);
        let sender = pending().lock().unwrap().remove(&id).unwrap();
        sender.send(Some(("yes".into(), false))).unwrap();
        let mut reply = Vec::new();
        client.read_to_end(&mut reply).unwrap();
        server.join().unwrap();
        assert_eq!(reply, b"yyes");
    }

    #[test]
    fn a_question_ends_when_the_helper_goes_away() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        let (events, inbox) = async_channel::unbounded();
        let server = std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            serve_connection(stream, "secret", &events);
        });
        let mut client = TcpStream::connect(("127.0.0.1", port)).unwrap();
        let unknown = "The authenticity of host 'example.com (192.0.2.1)' can't be established.\nED25519 key fingerprint is SHA256:abc.\nAre you sure?";
        client
            .write_all(format!("secret\n1\n0\n{unknown}\0").as_bytes())
            .unwrap();
        let id = asked_id(&inbox);
        drop(client);
        let Event::Gone { id: gone } = smol::block_on(inbox.recv()).unwrap() else {
            panic!("the question should end");
        };
        assert_eq!(gone, id);
        server.join().unwrap();
    }

    #[test]
    fn the_token_has_to_match() {
        assert!(token_matches("abc", "abc"));
        assert!(!token_matches("abd", "abc"));
        assert!(!token_matches("ab", "abc"));
        assert!(!token_matches("", "abc"));
        assert_eq!(new_token().map(|token| token.len()), Some(64));
    }

    #[test]
    fn a_second_question_from_one_process_means_a_wrong_answer() {
        assert!(!asked_before("test-subject-a", 10));
        assert!(asked_before("test-subject-a", 10));
        assert!(!asked_before("test-subject-a", 11));
        assert!(!asked_before("test-subject-b", 0));
        assert!(!asked_before("test-subject-b", 0));
    }

    #[test]
    fn github_s_published_key_needs_no_question() {
        let info = |key_type: &str, fingerprint: &str| AddSshHostInfo {
            host: "github.com".into(),
            ip: "140.82.121.4".into(),
            key_type: key_type.into(),
            fingerprint: fingerprint.into(),
        };
        assert!(is_known_github_key(&info("RSA", GITHUB_RSA)));
        assert!(!is_known_github_key(&info("RSA", "SHA256:other")));
        assert!(!is_known_github_key(&info("ED25519", GITHUB_RSA)));
    }
}
