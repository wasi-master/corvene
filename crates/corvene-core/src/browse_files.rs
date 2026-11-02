//! Corvene `1117-browse-files` and `1118-quick-edit` (desktop/desktop#13831,
//! #20440): look at any commit's files read-only and make a small edit to a
//! working file. GitHub Desktop can only open files in an external editor.
//!
//! `1366-diff-line-edits` (#14243): [`Dispatcher::save_line_edit`] replaces
//! one line from the Changes diff.

use std::collections::HashSet;
use std::sync::Arc;

use corvene_git::file_browser::{self, EditableText, NotEditable};

use crate::dispatcher::Dispatcher;
use crate::host::{AsyncCtx, Host};
use crate::state::Popup;

/// Larger files show a message: lines this long list badly and highlight
/// slowly.
const MAX_SHOWN_BYTES: usize = 10 * 1024 * 1024;
/// Larger files open read-only (the text box slows down past this).
const MAX_EDIT_BYTES: usize = 512 * 1024;

#[derive(Clone, Debug)]
pub enum BrowseContent {
    Text {
        lines: Arc<Vec<String>>,
        /// `Ok`: can be edited, when browsing the working tree.
        edit: Result<Arc<EditableText>, String>,
    },
    Image {
        bytes: Arc<Vec<u8>>,
    },
    Binary,
    TooLarge,
}

#[derive(Clone, Debug, Default)]
pub struct BrowseFilesState {
    /// `None`: the working tree.
    pub rev: Option<String>,
    pub files: Arc<Vec<String>>,
    pub loading: bool,
    pub selected: Option<String>,
    /// Bumped per read, so a late one for another file is dropped.
    pub generation: u64,
    pub content: Option<BrowseContent>,
    pub collapsed: HashSet<String>,
    /// A quick edit of the selected file is open.
    pub editing: bool,
    /// The next file read opens in the editor (Edit File in Changes).
    pub edit_next: bool,
    /// Bumped with each saved edit, for the view to take the new text.
    pub saved: u64,
}

/// Image kinds the view can draw, by file name.
pub fn is_image(path: &str) -> bool {
    let name = path.to_ascii_lowercase();
    [".png", ".jpg", ".jpeg", ".gif", ".webp", ".bmp", ".svg"]
        .iter()
        .any(|ext| name.ends_with(ext))
}

fn classify(path: &str, bytes: Vec<u8>, worktree: bool) -> BrowseContent {
    if bytes.len() > MAX_SHOWN_BYTES {
        return BrowseContent::TooLarge;
    }
    if is_image(path) {
        return BrowseContent::Image {
            bytes: Arc::new(bytes),
        };
    }
    if bytes.iter().take(8000).any(|b| *b == 0) {
        return BrowseContent::Binary;
    }
    let text = String::from_utf8_lossy(&bytes);
    let lines: Vec<String> = text
        .split_inclusive('\n')
        .map(|l| {
            let l = l.strip_suffix('\n').unwrap_or(l);
            l.strip_suffix('\r').unwrap_or(l).to_string()
        })
        .collect();
    let edit = if !worktree {
        Err("Choose Working Tree to edit this file.".to_string())
    } else if bytes.len() > MAX_EDIT_BYTES {
        Err("This file is too large to edit here.".to_string())
    } else {
        match file_browser::editable_text(&bytes) {
            Ok(editable) => Ok(Arc::new(editable)),
            Err(NotEditable::NotUtf8) => Err("This file is not UTF-8 text.".to_string()),
            Err(NotEditable::MixedLineEndings) => {
                Err("This file mixes line endings, so saving could change them.".to_string())
            }
        }
    };
    BrowseContent::Text {
        lines: Arc::new(lines),
        edit,
    }
}

impl Dispatcher {
    /// Open the browser on `rev`, or the working tree.
    pub fn show_browse_files(id: u64, rev: Option<String>, cx: &mut dyn Host) {
        let Some((git, workdir)) = Self::repo_context(id, cx) else {
            return;
        };
        Self::state(cx).update(cx, |s, cx| {
            let rs = s.repo_state_mut(id);
            let keep = rs.browse.as_ref().and_then(|b| b.selected.clone());
            rs.browse = Some(BrowseFilesState {
                rev: rev.clone(),
                loading: true,
                selected: keep,
                ..Default::default()
            });
            cx.notify();
        });
        let listed_rev = rev.clone();
        let task = cx
            .background_executor()
            .spawn(async move { file_browser::list_files(git, &workdir, listed_rev.as_deref()) });
        cx.spawn(async move |cx: &mut AsyncCtx| {
            let result = task.await;
            cx.update(|cx| {
                let reread = Self::state(cx).update(cx, |s, cx| {
                    let rs = s.repo_state_mut(id);
                    let browse = rs.browse.as_mut()?;
                    if browse.rev != rev {
                        return None;
                    }
                    browse.loading = false;
                    let files = result.as_ref().ok()?.clone();
                    browse.files = Arc::new(files);
                    cx.notify();
                    let selected = browse.selected.clone()?;
                    Some(
                        browse
                            .files
                            .binary_search(&selected)
                            .is_ok()
                            .then_some(selected),
                    )
                });
                if let Err(err) = &result {
                    Self::show_error("Could not list the files", err, cx);
                }
                match reread {
                    Some(Some(path)) => Self::select_browse_file(id, path, cx),
                    Some(None) => {
                        Self::state(cx).update(cx, |s, cx| {
                            if let Some(b) = s.repo_state_mut(id).browse.as_mut() {
                                b.selected = None;
                                b.content = None;
                                cx.notify();
                            }
                        });
                    }
                    None => {}
                }
            });
        })
        .detach();
    }

    pub fn close_browse_files(id: u64, cx: &mut dyn Host) {
        Self::state(cx).update(cx, |s, cx| {
            if s.repo_state_mut(id).browse.take().is_some() {
                cx.notify();
            }
        });
    }

    /// Read `path` at the browser's revision.
    pub fn select_browse_file(id: u64, path: String, cx: &mut dyn Host) {
        let Some((git, workdir)) = Self::repo_context(id, cx) else {
            return;
        };
        let Some((rev, generation)) = Self::state(cx).update(cx, |s, cx| {
            let browse = s.repo_state_mut(id).browse.as_mut()?;
            browse.selected = Some(path.clone());
            browse.generation += 1;
            browse.editing = std::mem::take(&mut browse.edit_next);
            browse.content = None;
            cx.notify();
            Some((browse.rev.clone(), browse.generation))
        }) else {
            return;
        };
        let read_path = path.clone();
        let worktree = rev.is_none();
        let task = cx.background_executor().spawn(async move {
            let bytes = match &rev {
                Some(rev) => corvene_git::blob_bytes(git, &workdir, rev, &read_path)?,
                None => std::fs::read(workdir.join(&read_path))?,
            };
            Ok::<_, corvene_git::GitError>(classify(&read_path, bytes, worktree))
        });
        cx.spawn(async move |cx: &mut AsyncCtx| {
            let result = task.await;
            cx.update(|cx| {
                Self::state(cx).update(cx, |s, cx| {
                    let Some(browse) = s.repo_state_mut(id).browse.as_mut() else {
                        return;
                    };
                    if browse.generation != generation {
                        return;
                    }
                    browse.content = Some(result.unwrap_or(BrowseContent::Binary));
                    cx.notify();
                });
            });
        })
        .detach();
    }

    pub fn set_browse_folder_collapsed(
        id: u64,
        folder: String,
        collapsed: bool,
        cx: &mut dyn Host,
    ) {
        Self::state(cx).update(cx, |s, cx| {
            if let Some(browse) = s.repo_state_mut(id).browse.as_mut() {
                if collapsed {
                    browse.collapsed.insert(folder);
                } else {
                    browse.collapsed.remove(&folder);
                }
                cx.notify();
            }
        });
    }

    /// `1118-quick-edit`: Edit File from the Changes list opens the browser
    /// on the working tree with the file in the editor.
    pub fn edit_working_file(id: u64, path: String, cx: &mut dyn Host) {
        Self::show_browse_files(id, None, cx);
        Self::state(cx).update(cx, |s, _| {
            if let Some(browse) = s.repo_state_mut(id).browse.as_mut() {
                browse.selected = Some(path);
                browse.edit_next = true;
            }
        });
    }

    pub fn set_file_editing(id: u64, editing: bool, cx: &mut dyn Host) {
        Self::state(cx).update(cx, |s, cx| {
            if let Some(browse) = s.repo_state_mut(id).browse.as_mut() {
                browse.editing = editing;
                cx.notify();
            }
        });
    }

    /// Write the edit, then show the file as saved and refresh Changes.
    pub fn save_file_edit(id: u64, path: String, text: String, cx: &mut dyn Host) {
        let Some((_, workdir)) = Self::repo_context(id, cx) else {
            return;
        };
        let editable = Self::state(cx)
            .read(cx)
            .repo_states
            .get(&id)
            .and_then(|rs| match rs.browse.as_ref()?.content.as_ref()? {
                BrowseContent::Text { edit: Ok(e), .. } => Some(e.clone()),
                _ => None,
            });
        let Some(editable) = editable else {
            return;
        };
        let bytes = editable.encode(&text);
        let task = cx
            .background_executor()
            .spawn(async move { std::fs::write(workdir.join(&path), bytes).map(|()| path) });
        cx.spawn(async move |cx: &mut AsyncCtx| {
            let result = task.await;
            cx.update(|cx| match result {
                Ok(path) => {
                    Self::state(cx).update(cx, |s, _| {
                        if let Some(browse) = s.repo_state_mut(id).browse.as_mut() {
                            browse.saved += 1;
                        }
                    });
                    Self::select_browse_file(id, path, cx);
                    Self::refresh_repository(id, cx);
                }
                Err(err) => Self::show_error("Could not save the file", err.to_string(), cx),
            });
        })
        .detach();
    }

    /// `1366-diff-line-edits`: Edit Line in the Changes diff.
    pub fn save_line_edit(
        id: u64,
        path: String,
        line: u32,
        old: String,
        new: String,
        cx: &mut dyn Host,
    ) {
        Self::close_popups_where(|p| matches!(p, Popup::EditLine { .. }), cx);
        let Some((_, workdir)) = Self::repo_context(id, cx) else {
            return;
        };
        let task = cx.background_executor().spawn(async move {
            let full = workdir.join(&path);
            let bytes = std::fs::read(&full)?;
            let Some(edited) = file_browser::replace_line(&bytes, line, &old, &new) else {
                return Err(std::io::Error::other(
                    "The line changed on disk since the diff was read. Nothing was written.",
                ));
            };
            std::fs::write(&full, edited)
        });
        cx.spawn(async move |cx: &mut AsyncCtx| {
            let result = task.await;
            cx.update(|cx| {
                if let Err(err) = result {
                    Self::show_error("Could not edit the line", err.to_string(), cx);
                }
                Self::refresh_repository(id, cx);
            });
        })
        .detach();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_what_it_reads() {
        assert!(matches!(
            classify("a.png", vec![1, 2], true),
            BrowseContent::Image { .. }
        ));
        assert!(matches!(
            classify("a.bin", vec![1, 0, 2], true),
            BrowseContent::Binary
        ));
        let BrowseContent::Text { lines, edit } = classify("a.txt", b"x\r\ny\n".to_vec(), true)
        else {
            panic!("text expected");
        };
        assert_eq!(*lines, ["x", "y"]);
        assert!(edit.is_ok());
        let BrowseContent::Text { edit, .. } = classify("a.txt", b"x\n".to_vec(), false) else {
            panic!("text expected");
        };
        assert!(edit.is_err());
        let BrowseContent::Text { edit, .. } = classify("a.txt", b"x\r\ny\n\xff".to_vec(), true)
        else {
            panic!("text expected");
        };
        assert!(edit.is_err());
    }
}
