//! Corvene `1361-merge-editor`: the merge editor's sessions. GitHub Desktop
//! has no merge editor; conflicted files say they "must be resolved via the
//! command line" (`app/src/ui/diff/index.tsx`) and the conflicts dialog opens
//! them in an external editor (`app/src/ui/lib/conflicts/unmerged-file.tsx`).
//!
//! A session is opened per conflicted file. It holds the document
//! ([`corvene_merge::reconcile`] of the index stages and the working file)
//! and the text as the view last reported it. Edits reach the file after
//! [`MERGE_SAVE_DELAY`] without further typing, unresolved conflicts as
//! git-style markers. A write refuses when the file is not what the session
//! last read or wrote: a clean session then reloads, one with unsaved edits
//! asks ([`MergeSession::external_change`]). Sessions go when their file is
//! no longer conflicted.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use corvene_git::CancelToken;
use corvene_git::merge_file::{
    self, ChunkBlame, MergeInputs, PickReason, WriteOutcome, WriteStamp,
};
use corvene_merge::{ConflictStyle, Labels, MergeDocument, MergeInput, Side};
use corvene_models::{ManualConflictResolution, Section};

use crate::dispatcher::Dispatcher;
use crate::host::{AsyncCtx, Host};
use crate::remote::spawn_bg;
use crate::state::{AppState, RepositoryState};

/// How long typing must pause before the result is written.
pub const MERGE_SAVE_DELAY: Duration = Duration::from_millis(400);

/// Whether the editor replaces the conflicted file message.
pub fn enabled(state: &AppState) -> bool {
    state.flags.bool(crate::flags::ids::MERGE_EDITOR)
}

/// One chunk's blame on one side.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BlameLoad {
    Loading,
    Loaded(Arc<Vec<ChunkBlame>>),
    /// The side has no revision or path to blame (a stash without its
    /// commit, a rename on that side).
    Unavailable,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AfterWrite {
    MarkResolved,
}

/// A conflicted file open in the merge editor.
#[derive(Clone, Debug)]
pub struct MergeSession {
    pub path: String,
    pub loading: bool,
    /// Bumped by every (re)load: the view takes the text from here again.
    pub generation: u64,
    pub inputs: Option<Arc<MergeInputs>>,
    /// `None` until loaded, and for files the side picker handles.
    pub document: Option<Arc<MergeDocument>>,
    /// The file goes to the side picker.
    pub pick: Option<PickReason>,
    /// Why there is nothing to show.
    pub message: Option<String>,
    /// The result as the view last reported it.
    pub text: Arc<str>,
    /// What the file holds, as far as the session knows.
    saved: Arc<str>,
    stamp: Option<WriteStamp>,
    /// The file changed behind the session while it had unsaved edits.
    pub external_change: bool,
    pub blame: HashMap<(u32, Side), BlameLoad>,
    /// Bumped when the result should take the keyboard (the conflicts
    /// dialog's Resolve).
    pub focus_request: u64,
    /// Identifies the latest edit, for the save delay.
    nonce: u64,
    writing: bool,
    after_write: Option<AfterWrite>,
    cancel: CancelToken,
}

impl MergeSession {
    fn new(path: String) -> Self {
        Self {
            path,
            loading: true,
            generation: 0,
            inputs: None,
            document: None,
            pick: None,
            message: None,
            text: Arc::from(""),
            saved: Arc::from(""),
            stamp: None,
            external_change: false,
            blame: HashMap::new(),
            focus_request: 0,
            nonce: 0,
            writing: false,
            after_write: None,
            cancel: CancelToken::new(),
        }
    }

    pub fn dirty(&self) -> bool {
        self.text != self.saved
    }
}

/// The session of `path`, for the view.
pub fn session_of<'a>(rs: &'a RepositoryState, path: &str) -> Option<&'a MergeSession> {
    rs.merge_editors.get(path)
}

/// What a status refresh does about the file's bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DiskChange {
    /// The session's own bytes.
    None,
    Reload,
    Ask,
}

fn disk_change(known: Option<WriteStamp>, now: Option<WriteStamp>, dirty: bool) -> DiskChange {
    if known == now {
        DiskChange::None
    } else if dirty {
        DiskChange::Ask
    } else {
        DiskChange::Reload
    }
}

/// Lays the stages over the working file.
pub fn build_document(inputs: &MergeInputs, ignore_whitespace: bool) -> MergeDocument {
    let labels = Labels {
        ours: inputs.labels.ours.clone(),
        base: inputs.labels.base.clone(),
        theirs: inputs.labels.theirs.clone(),
    };
    corvene_merge::reconcile(&MergeInput {
        base: inputs.base.as_deref(),
        ours: inputs.ours.as_deref().unwrap_or(""),
        theirs: inputs.theirs.as_deref().unwrap_or(""),
        working: inputs.working.as_deref().unwrap_or(""),
        style: inputs
            .conflict_style
            .as_deref()
            .map(ConflictStyle::from_config)
            .unwrap_or_default(),
        marker_size: inputs.attrs.marker_size.unwrap_or(0),
        labels,
        ignore_whitespace,
    })
}

type Loaded = Result<(MergeInputs, Option<MergeDocument>), String>;

impl Dispatcher {
    /// Opens `path` in the merge editor; an open session stays as it is.
    pub fn open_merge_editor(id: u64, path: String, cx: &mut dyn Host) {
        let opened = Self::state(cx).update(cx, |s, cx| {
            let rs = s.repo_state_mut(id);
            if rs.merge_editors.contains_key(&path) {
                return false;
            }
            rs.merge_editors
                .insert(path.clone(), MergeSession::new(path.clone()));
            cx.notify();
            true
        });
        if opened {
            Self::load_merge_session(id, path, cx);
        }
    }

    fn load_merge_session(id: u64, path: String, cx: &mut dyn Host) {
        let Some((git, workdir)) = Self::repo_context(id, cx) else {
            return;
        };
        let ignore_whitespace = Self::state(cx)
            .read(cx)
            .settings
            .merge_editor_ignore_whitespace;
        let load_path = path.clone();
        spawn_bg(
            cx,
            move || -> Loaded {
                let inputs = merge_file::load_merge_inputs(git, &workdir, &load_path)
                    .map_err(|err| err.to_string())?;
                let document = inputs
                    .pick
                    .is_none()
                    .then(|| build_document(&inputs, ignore_whitespace));
                Ok((inputs, document))
            },
            move |loaded, cx| Self::apply_merge_load(id, &path, loaded, cx),
        );
    }

    fn apply_merge_load(id: u64, path: &str, loaded: Loaded, cx: &mut dyn Host) {
        Self::state(cx).update(cx, |s, cx| {
            let Some(session) = s.repo_state_mut(id).merge_editors.get_mut(path) else {
                return;
            };
            session.loading = false;
            session.generation += 1;
            session.external_change = false;
            session.message = None;
            session.blame.clear();
            match loaded {
                Ok((inputs, document)) => {
                    if inputs.entries.is_empty() {
                        session.message = Some("This file is no longer in conflict.".to_string());
                    }
                    let text: Arc<str> = Arc::from(inputs.working.as_deref().unwrap_or(""));
                    session.text = text.clone();
                    session.saved = text;
                    session.stamp = inputs.working_stamp;
                    session.pick = inputs.pick;
                    session.document = document.map(Arc::new);
                    session.inputs = Some(Arc::new(inputs));
                }
                Err(message) => session.message = Some(message),
            }
            cx.notify();
        });
    }

    /// The toolbar's whitespace toggle: the chunks are worked out again.
    pub fn merge_editor_whitespace_changed(id: u64, cx: &mut dyn Host) {
        let ignore = Self::state(cx)
            .read(cx)
            .settings
            .merge_editor_ignore_whitespace;
        Self::state(cx).update(cx, |s, cx| {
            let rs = s.repo_state_mut(id);
            for session in rs.merge_editors.values_mut() {
                let Some(inputs) = session.inputs.as_ref().filter(|_| session.pick.is_none())
                else {
                    continue;
                };
                // the stages with the result as it is now
                let mut inputs = (**inputs).clone();
                inputs.working = Some(session.text.to_string());
                session.document = Some(Arc::new(build_document(&inputs, ignore)));
                session.generation += 1;
            }
            cx.notify();
        });
    }

    /// The result now holds `text`; written once typing pauses.
    pub fn merge_text_changed(id: u64, path: &str, text: String, cx: &mut dyn Host) {
        let state = Self::state(cx);
        let nonce = state.update(cx, |s, _| {
            let session = s.repo_state_mut(id).merge_editors.get_mut(path)?;
            if &*session.text == text.as_str() {
                return None;
            }
            session.text = Arc::from(text);
            session.nonce += 1;
            Some(session.nonce)
        });
        let Some(nonce) = nonce else { return };
        let path = path.to_string();
        cx.spawn(async move |cx: &mut AsyncCtx| {
            cx.background_executor().timer(MERGE_SAVE_DELAY).await;
            cx.update(|cx| {
                let current = Self::state(cx)
                    .read(cx)
                    .repo_states
                    .get(&id)
                    .and_then(|rs| rs.merge_editors.get(&path))
                    .is_some_and(|session| session.nonce == nonce);
                if current {
                    Self::write_merge_session(id, &path, cx);
                }
            });
        })
        .detach();
    }

    /// ⌘S, or leaving the file: write what is pending now.
    pub fn flush_merge_editor(id: u64, path: &str, cx: &mut dyn Host) {
        Self::write_merge_session(id, path, cx);
    }

    /// Writes the session's text unless a write is running (it writes again
    /// when it ends) or the file changed behind it.
    fn write_merge_session(id: u64, path: &str, cx: &mut dyn Host) {
        let Some((_, workdir)) = Self::repo_context(id, cx) else {
            return;
        };
        enum Next {
            Nothing,
            Write(Arc<str>, bool, Option<WriteStamp>),
            Finished(AfterWrite),
        }
        let next = Self::state(cx).update(cx, |s, _| {
            let Some(session) = s.repo_state_mut(id).merge_editors.get_mut(path) else {
                return Next::Nothing;
            };
            if session.writing || session.external_change {
                return Next::Nothing;
            }
            if !session.dirty() {
                return match session.after_write.take() {
                    Some(after) => Next::Finished(after),
                    None => Next::Nothing,
                };
            }
            session.writing = true;
            let crlf = session.inputs.as_ref().is_some_and(|i| i.crlf);
            Next::Write(session.text.clone(), crlf, session.stamp)
        });
        let (text, crlf, stamp) = match next {
            Next::Nothing => return,
            Next::Finished(AfterWrite::MarkResolved) => {
                Self::stage_merge_result(id, path.to_string(), cx);
                return;
            }
            Next::Write(text, crlf, stamp) => (text, crlf, stamp),
        };
        let write_path = path.to_string();
        let path = path.to_string();
        let written = text.clone();
        spawn_bg(
            cx,
            move || merge_file::write_merge_result(&workdir, &write_path, &written, crlf, stamp),
            move |outcome, cx| {
                let wrote = Self::state(cx).update(cx, |s, cx| {
                    let session = s.repo_state_mut(id).merge_editors.get_mut(&path)?;
                    session.writing = false;
                    let again = match outcome {
                        Ok(WriteOutcome::Written(stamp)) => {
                            session.stamp = Some(stamp);
                            session.saved = text;
                            true
                        }
                        Ok(WriteOutcome::ChangedOnDisk(_)) => {
                            session.external_change = true;
                            session.after_write = None;
                            cx.notify();
                            false
                        }
                        Err(err) => {
                            session.after_write = None;
                            session.message = Some(err.to_string());
                            cx.notify();
                            false
                        }
                    };
                    Some(again)
                });
                if wrote == Some(true) {
                    Self::write_merge_session(id, &path, cx);
                }
            },
        );
    }

    /// Mark as Resolved: the pending text is written, then the file staged.
    pub fn mark_merge_resolved(id: u64, path: &str, cx: &mut dyn Host) {
        Self::state(cx).update(cx, |s, _| {
            if let Some(session) = s.repo_state_mut(id).merge_editors.get_mut(path) {
                session.after_write = Some(AfterWrite::MarkResolved);
            }
        });
        Self::write_merge_session(id, path, cx);
    }

    fn stage_merge_result(id: u64, path: String, cx: &mut dyn Host) {
        let Some((git, workdir)) = Self::repo_context(id, cx) else {
            return;
        };
        let origin = merge_file::conflict_origin(&workdir);
        // `774-stash-conflict-flow`: a restore has no operation to continue,
        // and its files leave the index as changes of their own
        if origin == merge_file::ConflictOrigin::StashPop {
            Self::close_merge_editor(id, &path, cx);
            Self::mark_stash_conflicts_resolved(id, vec![path], cx);
            return;
        }
        let stage_path = path.clone();
        spawn_bg(
            cx,
            move || {
                merge_file::stage_resolved_path(git.clone(), &workdir, &stage_path)?;
                corvene_git::unmerged_paths(git, &workdir)
            },
            move |result, cx| Self::after_merge_marked(id, &path, result, cx),
        );
    }

    fn after_merge_marked(
        id: u64,
        path: &str,
        result: Result<Vec<String>, corvene_git::GitError>,
        cx: &mut dyn Host,
    ) {
        match result {
            Ok(left) => {
                Self::close_merge_editor(id, path, cx);
                Self::refresh_repository(id, cx);
                // the last file: the dialog's Continue is what comes next
                if left.is_empty() {
                    Self::show_conflicts(id, cx);
                }
            }
            Err(err) => Self::show_error("Could not mark the file as resolved", &err, cx),
        }
    }

    /// The side picker: the file as one side has it, deleted if that side
    /// deleted it.
    pub fn pick_merge_side(
        id: u64,
        path: String,
        side: ManualConflictResolution,
        cx: &mut dyn Host,
    ) {
        let Some((git, workdir)) = Self::repo_context(id, cx) else {
            return;
        };
        let file = Self::state(cx)
            .read(cx)
            .repo_states
            .get(&id)
            .and_then(|rs| rs.status.as_deref())
            .and_then(|st| st.files.iter().find(|f| f.path == path))
            .cloned();
        let Some(mut file) = file else { return };
        // `merge=binary` files have a marker count of 0, which the stage
        // takes as "resolved in an editor"
        file.status.conflict_markers = None;
        let origin = merge_file::conflict_origin(&workdir);
        let stage_path = path.clone();
        spawn_bg(
            cx,
            move || {
                corvene_git::rebase_ops::stage_manual_conflict_resolution(
                    git.clone(),
                    &workdir,
                    &file,
                    side,
                )?;
                corvene_git::unmerged_paths(git, &workdir)
            },
            move |result, cx| {
                if origin == merge_file::ConflictOrigin::StashPop && result.is_ok() {
                    Self::close_merge_editor(id, &stage_path, cx);
                    Self::mark_stash_conflicts_resolved(id, vec![stage_path], cx);
                } else {
                    Self::after_merge_marked(id, &stage_path, result, cx);
                }
            },
        );
    }

    /// The conflicts dialog's Resolve: closes the dialog (the banner keeps
    /// the way back) and shows the file in the Changes tab.
    pub fn resolve_in_app(id: u64, path: String, cx: &mut dyn Host) {
        Self::hide_conflicts(id, cx);
        Self::show_section(id, Section::Changes, cx);
        Self::select_file(id, path.clone(), cx);
        Self::open_merge_editor(id, path.clone(), cx);
        Self::state(cx).update(cx, |s, cx| {
            if let Some(session) = s.repo_state_mut(id).merge_editors.get_mut(&path) {
                session.focus_request += 1;
                cx.notify();
            }
        });
    }

    /// Reads the file again; edits not yet saved are lost.
    pub fn reload_merge_from_disk(id: u64, path: String, cx: &mut dyn Host) {
        Self::state(cx).update(cx, |s, cx| {
            if let Some(session) = s.repo_state_mut(id).merge_editors.get_mut(&path) {
                session.loading = true;
                session.nonce += 1;
                session.after_write = None;
                cx.notify();
            }
        });
        Self::load_merge_session(id, path, cx);
    }

    /// The banner's Keep Mine: the session's text wins over the file's.
    pub fn keep_merge_edits(id: u64, path: String, cx: &mut dyn Host) {
        let now = Self::repo_context(id, cx)
            .and_then(|(_, workdir)| merge_file::file_stamp(&workdir, &path));
        Self::state(cx).update(cx, |s, cx| {
            if let Some(session) = s.repo_state_mut(id).merge_editors.get_mut(&path) {
                session.external_change = false;
                session.stamp = now;
                cx.notify();
            }
        });
        Self::write_merge_session(id, &path, cx);
    }

    pub fn close_merge_editor(id: u64, path: &str, cx: &mut dyn Host) {
        Self::state(cx).update(cx, |s, cx| {
            if let Some(session) = s.repo_state_mut(id).merge_editors.remove(path) {
                session.cancel.cancel();
                cx.notify();
            }
        });
    }

    /// Who changed a chunk on one side, for its hover card.
    pub fn load_chunk_blame(id: u64, path: &str, chunk: u32, side: Side, cx: &mut dyn Host) {
        let Some((git, workdir)) = Self::repo_context(id, cx) else {
            return;
        };
        let request = Self::state(cx).update(cx, |s, cx| {
            let session = s.repo_state_mut(id).merge_editors.get_mut(path)?;
            if session.blame.contains_key(&(chunk, side)) {
                return None;
            }
            let lines = session
                .document
                .as_ref()?
                .chunks
                .get(chunk as usize)?
                .lines(side);
            let revs = &session.inputs.as_ref()?.revs;
            let rev = match side {
                Side::Base => revs.base.clone(),
                Side::Ours => revs.ours.clone(),
                Side::Theirs => revs.theirs.clone(),
            };
            let Some(rev) = rev else {
                session.blame.insert((chunk, side), BlameLoad::Unavailable);
                cx.notify();
                return None;
            };
            session.blame.insert((chunk, side), BlameLoad::Loading);
            cx.notify();
            Some((rev, lines, session.cancel.clone()))
        });
        let Some((rev, lines, cancel)) = request else {
            return;
        };
        let blame_path = path.to_string();
        let path = path.to_string();
        spawn_bg(
            cx,
            move || merge_file::blame_lines(git, &workdir, &rev, &blame_path, lines, Some(cancel)),
            move |result, cx| {
                Self::state(cx).update(cx, |s, cx| {
                    let Some(session) = s.repo_state_mut(id).merge_editors.get_mut(&path) else {
                        return;
                    };
                    let load = match result {
                        Ok(blames) if blames.is_empty() => BlameLoad::Unavailable,
                        Ok(blames) => BlameLoad::Loaded(Arc::new(blames)),
                        Err(_) => BlameLoad::Unavailable,
                    };
                    session.blame.insert((chunk, side), load);
                    cx.notify();
                });
            },
        );
    }

    /// After a status refresh: sessions of files no longer conflicted go,
    /// and files changed behind the others are read again or asked about.
    pub(crate) fn merge_editors_after_refresh(id: u64, cx: &mut dyn Host) {
        let open = Self::state(cx).update(cx, |s, cx| {
            let rs = s.repo_state_mut(id);
            if rs.merge_editors.is_empty() {
                return Vec::new();
            }
            let Some(status) = rs.status.clone() else {
                return Vec::new();
            };
            let before = rs.merge_editors.len();
            rs.merge_editors.retain(|path, session| {
                let keep = session.writing
                    || session.loading
                    || status
                        .files
                        .iter()
                        .any(|f| &f.path == path && f.status.is_conflicted());
                if !keep {
                    session.cancel.cancel();
                }
                keep
            });
            if rs.merge_editors.len() != before {
                cx.notify();
            }
            rs.merge_editors
                .iter()
                .filter(|(_, session)| !session.writing && !session.external_change)
                .map(|(path, session)| (path.clone(), session.generation, session.stamp))
                .collect::<Vec<_>>()
        });
        if open.is_empty() {
            return;
        }
        let Some((_, workdir)) = Self::repo_context(id, cx) else {
            return;
        };
        spawn_bg(
            cx,
            move || {
                open.into_iter()
                    .map(|(path, generation, stamp)| {
                        let now = merge_file::file_stamp(&workdir, &path);
                        (path, generation, stamp, now)
                    })
                    .collect::<Vec<_>>()
            },
            move |stamps, cx| {
                for (path, generation, known, now) in stamps {
                    let change = Self::state(cx).update(cx, |s, cx| {
                        let session = s.repo_state_mut(id).merge_editors.get_mut(&path)?;
                        // written or reloaded since the file was read
                        if session.generation != generation
                            || session.writing
                            || session.stamp != known
                        {
                            return None;
                        }
                        let change = disk_change(known, now, session.dirty());
                        if change == DiskChange::Ask {
                            session.external_change = true;
                            cx.notify();
                        }
                        Some(change)
                    });
                    if change == Some(DiskChange::Reload) {
                        Self::reload_merge_from_disk(id, path, cx);
                    }
                }
            },
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stamp(n: u64) -> Option<WriteStamp> {
        Some(WriteStamp { len: n, hash: n })
    }

    #[test]
    fn own_writes_are_not_changes() {
        assert_eq!(disk_change(stamp(1), stamp(1), false), DiskChange::None);
        assert_eq!(disk_change(stamp(1), stamp(1), true), DiskChange::None);
    }

    #[test]
    fn someone_elses_write_reloads_or_asks() {
        assert_eq!(disk_change(stamp(1), stamp(2), false), DiskChange::Reload);
        assert_eq!(disk_change(stamp(1), stamp(2), true), DiskChange::Ask);
        assert_eq!(disk_change(stamp(1), None, true), DiskChange::Ask);
    }

    #[test]
    fn a_session_is_dirty_until_written() {
        let mut session = MergeSession::new("a".into());
        assert!(!session.dirty());
        session.text = Arc::from("x");
        assert!(session.dirty());
        session.saved = session.text.clone();
        assert!(!session.dirty());
    }

    #[test]
    fn the_document_follows_the_inputs() {
        let inputs = MergeInputs {
            path: "a".into(),
            base: Some("a\nb\nc\n".into()),
            ours: Some("a\nours\nc\n".into()),
            theirs: Some("a\ntheirs\nc\n".into()),
            working: Some("a\n<<<<<<< HEAD\nours\n=======\ntheirs\n>>>>>>> t\nc\n".into()),
            conflict_style: Some("zdiff3".into()),
            ..MergeInputs::default()
        };
        let doc = build_document(&inputs, false);
        assert_eq!(doc.style, ConflictStyle::ZDiff3);
        assert_eq!(doc.conflicts().count(), 1);
        assert_eq!(doc.labels.theirs.as_deref(), Some("t"));
    }
}
