//! Git LFS transfer progress - GHD `lib/progress/lfs.ts` and the
//! `trackLFSProgress` half of `lib/progress/from-process.ts`
//! (`executionOptionsWithProgress`, `createProgressProcessCallback`).
//!
//! A clone, fetch, pull or push runs with `GIT_LFS_PROGRESS` naming a fresh temp
//! file, which Git LFS appends one `<direction> <n>/<files> <done>/<size>
//! <name>` line to as it transfers. [`run_with_progress`] tails that file
//! while git runs and merges its lines with git's own `--progress` lines the
//! way GHD does: once LFS reported progress, git's lines that are not one
//! of the parser's steps (`Filtering content` among them) are dropped, so
//! the description does not flicker between the two.
//!
//! GHD reads git's stderr and tails the file on one event loop. Here git
//! runs on a scoped thread and the file is polled on another; both send
//! their lines to the calling thread, which parses them and calls the
//! (not `Send`) progress callback, so the callback sees the lines in the
//! order they arrived. The byte counts in the description use `,` and `.`
//! as separators (GHD's `formatBytes` follows the user's number format,
//! which the git crate does not see).

use std::collections::HashMap;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc;
use std::time::Duration;

use tracing::warn;

use crate::error::Result;
use crate::process::{GitCommand, GitOutput};
use crate::remote_ops::{GitProgressEvent, ProgressLine, ProgressParser};

/// How often the progress file is read while git runs.
const TAIL_INTERVAL: Duration = Duration::from_millis(100);

/// GHD `IFileProgress`: the last report for one file.
#[derive(Clone, Copy, Debug)]
struct FileProgress {
    transferred: u64,
    size: u64,
    done: bool,
}

/// GHD `GitLFSProgressParser`: turns `GIT_LFS_PROGRESS` lines into progress
/// over every file seen so far. One parser per git run.
#[derive(Clone, Debug, Default)]
pub struct GitLfsProgressParser {
    files: HashMap<String, FileProgress>,
}

impl GitLfsProgressParser {
    pub fn new() -> Self {
        Self::default()
    }

    /// GHD `parse`: a `<direction> <current>/<total files>
    /// <transferred>/<size> <name>` line is progress (overall percent 0, GHD's
    /// indeterminate LFS progress), anything else context.
    pub fn parse_event(&mut self, line: &str) -> GitProgressEvent {
        let Some(fields) = parse_lfs_line(line) else {
            return GitProgressEvent::Context {
                percent: 0.,
                text: line.to_string(),
            };
        };
        self.files.insert(
            fields.name.to_string(),
            FileProgress {
                transferred: fields.transferred,
                size: fields.size,
                done: fields.transferred == fields.size,
            },
        );
        // the estimate is exact for uploads but not for downloads: take
        // whichever is bigger, it or the files seen
        let file_count = fields.estimated_files.max(self.files.len() as u64);
        let (mut transferred, mut estimated, mut finished) = (0u64, 0u64, 0usize);
        for file in self.files.values() {
            transferred = transferred.saturating_add(file.transferred);
            estimated = estimated.saturating_add(file.size);
            finished += usize::from(file.done);
        }
        let verb = match fields.direction {
            "upload" => "Uploading",
            "checkout" => "Checking out",
            _ => "Downloading",
        };
        GitProgressEvent::Progress {
            percent: 0.,
            details: ProgressLine {
                title: format!("{verb} \"{}\"", fields.name),
                value: transferred,
                total: Some(estimated),
                percent: Some(0),
                done: false,
                text: format!(
                    "{verb} {} ({finished} out of an estimated {file_count} completed, {} / {})",
                    fields.name,
                    format_bytes(transferred),
                    format_bytes(estimated),
                ),
            },
        }
    }

    /// [`Self::parse_event`] as `Some((percent, text))` for progress and
    /// `None` for context, like [`ProgressParser::parse`].
    pub fn parse(&mut self, line: &str) -> Option<(f32, String)> {
        match self.parse_event(line) {
            GitProgressEvent::Progress { percent, details } => Some((percent as f32, details.text)),
            GitProgressEvent::Context { .. } => None,
        }
    }
}

/// The fields of one `GIT_LFS_PROGRESS` line.
struct LfsLine<'a> {
    direction: &'a str,
    estimated_files: u64,
    transferred: u64,
    size: u64,
    name: &'a str,
}

/// GHD `LFSProgressLineRe`:
/// `/^(.+?)\s{1}(\d+)\/(\d+)\s{1}(\d+)\/(\d+)\s{1}(.+)$/`. The direction is
/// the shortest prefix after which the rest matches.
fn parse_lfs_line(line: &str) -> Option<LfsLine<'_>> {
    line.char_indices()
        .filter(|(at, c)| *at > 0 && c.is_whitespace())
        .find_map(|(at, c)| {
            let (_current, estimated_files, transferred, size, name) =
                parse_lfs_counts(&line[at + c.len_utf8()..])?;
            Some(LfsLine {
                direction: &line[..at],
                estimated_files,
                transferred,
                size,
                name,
            })
        })
}

/// `<current>/<files> <transferred>/<size> <name>` with one whitespace
/// character between the parts and a non-empty name.
fn parse_lfs_counts(s: &str) -> Option<(u64, u64, u64, u64, &str)> {
    fn number(s: &str) -> Option<(u64, &str)> {
        let digits = s.bytes().take_while(u8::is_ascii_digit).count();
        Some((s[..digits].parse().ok()?, &s[digits..]))
    }
    fn space(s: &str) -> Option<&str> {
        let c = s.chars().next().filter(|c| c.is_whitespace())?;
        Some(&s[c.len_utf8()..])
    }
    let (current, s) = number(s)?;
    let (files, s) = number(s.strip_prefix('/')?)?;
    let (transferred, s) = number(space(s)?)?;
    let (size, s) = number(s.strip_prefix('/')?)?;
    let name = space(s)?;
    (!name.is_empty() && !name.contains(['\n', '\r', '\u{2028}', '\u{2029}'])).then_some((
        current,
        files,
        transferred,
        size,
        name,
    ))
}

/// GHD `formatBytes(bytes, 1)` (`ui/lib/bytes.ts` › `formatCompactNumber`
/// with base 1024): `300 B`, `1.5 KiB`, `1,024 KiB`; one decimal at most,
/// none when it is zero.
fn format_bytes(bytes: u64) -> String {
    const UNITS: [&str; 9] = ["B", "KiB", "MiB", "GiB", "TiB", "PiB", "EiB", "ZiB", "YiB"];
    let value = bytes as f64;
    let unit = if value < 1024. {
        0
    } else {
        ((value.ln() / 1024f64.ln()).floor() as usize).min(UNITS.len() - 1)
    };
    let scaled = value / 1024f64.powi(unit as i32);
    // GHD `round(value, 1)`
    let rounded = ((scaled + f64::EPSILON) * 10.).round() / 10.;
    let tenths = (rounded * 10.).round() as u64;
    let (int, frac) = (tenths / 10, tenths % 10);
    let mut grouped = String::new();
    let digits = int.to_string();
    for (i, d) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            grouped.push(',');
        }
        grouped.push(d);
    }
    if frac > 0 {
        format!("{grouped}.{frac} {}", UNITS[unit])
    } else {
        format!("{grouped} {}", UNITS[unit])
    }
}

/// A line read while git runs.
enum Streamed {
    Stderr(String),
    Lfs(String),
}

/// GHD `createProgressProcessCallback`: the progress to report for one
/// line, or `None` for a line GHD drops while LFS progress is showing.
/// What [`run_with_progress`] turns git's stderr lines into: GHD's
/// `IGitProgressParser` (a [`ProgressParser`], or the submodule update's
/// own, `crate::op_progress::SubmoduleUpdateParser`).
pub(crate) trait LineParser {
    fn parse_event(&mut self, line: &str) -> GitProgressEvent;
}

impl LineParser for ProgressParser {
    fn parse_event(&mut self, line: &str) -> GitProgressEvent {
        ProgressParser::parse_event(self, line)
    }
}

struct ProgressMerger<'a> {
    parser: &'a mut dyn LineParser,
    lfs: GitLfsProgressParser,
    lfs_active: bool,
}

impl ProgressMerger<'_> {
    fn on_line(&mut self, line: Streamed) -> Option<GitProgressEvent> {
        match line {
            Streamed::Lfs(line) => {
                let progress = self.lfs.parse_event(&line);
                matches!(progress, GitProgressEvent::Progress { .. }).then(|| {
                    self.lfs_active = true;
                    progress
                })
            }
            Streamed::Stderr(line) => {
                let progress = self.parser.parse_event(&line);
                if self.lfs_active {
                    // no context in between LFS progress, which would flicker
                    let GitProgressEvent::Progress { details, .. } = &progress else {
                        return None;
                    };
                    // the LFS filter runs while git reports `Filtering content`
                    if details.title == "Filtering content" {
                        if details.done {
                            self.lfs_active = false;
                        }
                        return None;
                    }
                }
                Some(progress)
            }
        }
    }
}

/// GHD `executionOptionsWithProgress` with `trackLFSProgress`: runs `cmd`
/// (a clone, fetch, pull or push with `--progress`) with `GIT_LFS_PROGRESS` set,
/// and calls `on_event` with what GHD's progress callback receives: every
/// stderr line through `parser`, and the Git LFS progress. When the
/// progress file cannot be created, only stderr is followed (GHD logs the
/// error and goes on the same way).
pub(crate) fn run_with_progress(
    cmd: GitCommand,
    parser: &mut dyn LineParser,
    on_event: &mut dyn FnMut(GitProgressEvent),
) -> Result<GitOutput> {
    let mut merger = ProgressMerger {
        parser,
        lfs: GitLfsProgressParser::new(),
        lfs_active: false,
    };
    let progress_file = match ProgressFile::create() {
        Ok(file) => file,
        Err(err) => {
            warn!(%err, "could not create the Git LFS progress file");
            return cmd.run_streaming(|line| {
                if let Some(event) = merger.on_line(Streamed::Stderr(line.to_string())) {
                    on_event(event);
                }
            });
        }
    };
    // git runs on a thread of its own: it takes this thread's cancel token
    // along (`process::with_cancel_token`)
    let cmd = cmd
        .env("GIT_LFS_PROGRESS", &progress_file.path)
        .with_scoped_cancel();
    // `with_env` variables belong to this thread's git commands, and its
    // tracing span to the logs of this run
    let scoped_env = crate::process::scoped_env();
    let span = tracing::Span::current();
    let finished = AtomicBool::new(false);
    let (tx, rx) = mpsc::channel::<Streamed>();
    let git_tx = tx.clone();
    std::thread::scope(|scope| {
        let path = progress_file.path.as_path();
        let finished = &finished;
        let tail = scope.spawn(move || {
            tail_by_line(path, finished, |line| {
                let _ = tx.send(Streamed::Lfs(line));
            })
        });
        let tail_thread = tail.thread().clone();
        let (cmd, scoped_env) = (&cmd, &scoped_env);
        let git = scope.spawn(move || {
            // also when git's thread panics, so the tail and the loop below end
            let _finished = Finished {
                flag: finished,
                tail: tail_thread,
            };
            let _span = span.enter();
            let env: Vec<(&str, &str)> = scoped_env
                .iter()
                .map(|(k, v)| (k.as_str(), v.as_str()))
                .collect();
            crate::process::with_env(&env, || {
                cmd.run_streaming(|line| {
                    let _ = git_tx.send(Streamed::Stderr(line.to_string()));
                })
            })
        });
        // ends once both threads are done and have dropped their senders
        for line in rx {
            if let Some(event) = merger.on_line(line) {
                on_event(event);
            }
        }
        match git.join() {
            Ok(result) => result,
            Err(panic) => std::panic::resume_unwind(panic),
        }
    })
}

/// Tells the tail thread that git exited, when dropped.
struct Finished<'a> {
    flag: &'a AtomicBool,
    tail: std::thread::Thread,
}

impl Drop for Finished<'_> {
    fn drop(&mut self) {
        self.flag.store(true, Ordering::SeqCst);
        // the last read need not wait out the poll interval
        self.tail.unpark();
    }
}

/// GHD `createLFSProgressFile`: an empty file of its own in the temp
/// directory, removed again when dropped.
struct ProgressFile {
    path: PathBuf,
}

impl ProgressFile {
    fn create() -> std::io::Result<Self> {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let path = std::env::temp_dir().join(format!(
            "Corvene-lfs-progress-{}-{nanos:x}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        // `wx`: never reuse a file that is already there
        std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)?;
        Ok(Self { path })
    }
}

impl Drop for ProgressFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

/// GHD `tailByLine`: calls `on_line` with every line appended to `path`
/// until `finished` is set, then once more with what is left. Polls every
/// [`TAIL_INTERVAL`]; unpark the thread to make it look sooner.
fn tail_by_line(path: &Path, finished: &AtomicBool, mut on_line: impl FnMut(String)) {
    let mut file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(err) => {
            warn!(%err, path = %path.display(), "unable to tail the Git LFS progress file");
            return;
        }
    };
    let mut position = 0u64;
    let mut pending = Vec::new();
    loop {
        // read `finished` first so the last appended lines are not missed
        let done = finished.load(Ordering::SeqCst);
        let mut chunk = Vec::new();
        if file.seek(SeekFrom::Start(position)).is_ok()
            && let Ok(n) = file.read_to_end(&mut chunk)
        {
            position += n as u64;
            pending.extend_from_slice(&chunk);
            // byline: `\n`, `\r\n` and `\r` end a line
            while let Some(end) = pending.iter().position(|b| *b == b'\n' || *b == b'\r') {
                let line = String::from_utf8_lossy(&pending[..end]).into_owned();
                let skip = if pending[end] == b'\r' && pending.get(end + 1) == Some(&b'\n') {
                    2
                } else {
                    1
                };
                pending.drain(..end + skip);
                if !line.is_empty() {
                    on_line(line);
                }
            }
        }
        if done {
            if !pending.is_empty() {
                on_line(String::from_utf8_lossy(&pending).into_owned());
            }
            return;
        }
        // woken early once git exits
        std::thread::park_timeout(TAIL_INTERVAL);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reports_every_file_seen() {
        let mut parser = GitLfsProgressParser::new();
        let GitProgressEvent::Progress { percent, details } =
            parser.parse_event("download 1/2 5/300 my cool image.jpg")
        else {
            panic!("expected progress");
        };
        assert_eq!(percent, 0.);
        assert_eq!(details.title, "Downloading \"my cool image.jpg\"");
        assert_eq!(
            details.text,
            "Downloading my cool image.jpg (0 out of an estimated 2 completed, 5 B / 300 B)"
        );
        let (_, text) = parser.parse("upload 2/2 2048/2048 b.psd").unwrap();
        assert_eq!(
            text,
            "Uploading b.psd (1 out of an estimated 2 completed, 2 KiB / 2.3 KiB)"
        );
        // a third file beyond the estimate raises the count
        let (_, text) = parser.parse("checkout 3/2 0/1 c").unwrap();
        assert!(text.contains("out of an estimated 3 completed"), "{text}");
        assert!(
            parser
                .parse("Git LFS: (1 of 2 files) 5 B / 300 B")
                .is_none()
        );
        assert!(parser.parse("download 1/2 5/300").is_none());
    }

    #[test]
    fn formats_bytes_like_ghd() {
        assert_eq!(format_bytes(0), "0 B");
        assert_eq!(format_bytes(1023), "1,023 B");
        assert_eq!(format_bytes(1024), "1 KiB");
        assert_eq!(format_bytes(1536), "1.5 KiB");
        assert_eq!(format_bytes(1_048_575), "1,024 KiB");
        assert_eq!(format_bytes(5 * 1024 * 1024 * 1024), "5 GiB");
    }

    #[test]
    fn lfs_progress_hides_git_context() {
        let mut parser = ProgressParser::push();
        let mut merger = ProgressMerger {
            parser: &mut parser,
            lfs: GitLfsProgressParser::new(),
            lfs_active: false,
        };
        let line = |s: &str| Streamed::Stderr(s.to_string());
        assert!(merger.on_line(line("To /tmp/remote.git")).is_some());
        assert!(
            merger
                .on_line(Streamed::Lfs("upload 1/1 1/2 a.bin".into()))
                .is_some()
        );
        assert!(merger.on_line(line("To /tmp/remote.git")).is_none());
        // not a step of any parser, so context, as in GHD
        assert!(
            merger
                .on_line(line("Filtering content:  50% (1/2)"))
                .is_none()
        );
        // git's own steps still show
        assert!(
            merger
                .on_line(line("Writing objects:  50% (1/2)"))
                .is_some()
        );
        assert!(merger.on_line(line("To /tmp/remote.git")).is_none());
    }

    #[test]
    fn tails_appended_lines() {
        let file = ProgressFile::create().unwrap();
        std::fs::write(&file.path, "download 1/1 0/4 a\ndownload 1/1 4/4").unwrap();
        let finished = AtomicBool::new(true);
        let mut lines = Vec::new();
        tail_by_line(&file.path, &finished, |l| lines.push(l));
        assert_eq!(lines, ["download 1/1 0/4 a", "download 1/1 4/4"]);
        let path = file.path.clone();
        drop(file);
        assert!(!path.exists());
    }
}
