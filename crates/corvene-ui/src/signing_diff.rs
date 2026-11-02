//! Corvene `1341-signing-material-diff`: certificates, PKCS#12 bundles,
//! provisioning profiles, keystores, OpenPGP and SSH keys read as their
//! public fields. A strip on top names the file kind, the item counts and
//! the added / removed / changed chips, with a "Show only changes" toggle;
//! a locked PKCS#12 bundle gets a password field there. Under it each item
//! is a card (children indented under their parent) with an icon by kind,
//! a status tint, an expiry badge and its fields, changed values shown old
//! over new (or side by side). Runs of unchanged items fold into a
//! "N unchanged" row.
//!
//! GitHub Desktop shows these files as binary or text diffs
//! (`app/src/ui/diff/index.tsx`); the diff header's Fields / Source switch
//! goes back to git's diff for text files.
//!
//! Deviation within the feature: field values are not selectable (GPUI
//! static text); fingerprint-like values get a copy button on hover and
//! every value a "Copy value" context menu item.

use std::collections::HashSet;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use corvene_core::Diff;
use corvene_models::SigningFile;
use corvene_models::signing::{FieldRow, ItemRow, PasswordState, RowStatus, SigningItemKind};
use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::prelude::*;
use gpui_kit::*;

use crate::icons::{Octicon, octicon};
use crate::keyboard_nav::ControlFocus;
use crate::scrollbar::{gutter, scrollbar};
use crate::theme::sizes::*;
use crate::theme::{ActiveGhdTheme, GhdTheme, mono_font};
use crate::widgets::{GhdTooltip, IconButtonA11y};

/// Items (both sides, nested included) above which rows are aligned on a
/// background task.
const BACKGROUND_ITEMS: usize = 300;
/// Indentation per nesting level (CSS px).
const INDENT: f32 = 16.;
/// The field label column's width (CSS px).
const LABEL_WIDTH: f32 = 160.;
/// Days before expiry that turn the badge orange.
const EXPIRY_WARNING_DAYS: i64 = 30;
/// How long a copied value shows its check mark.
const COPIED_FOR: Duration = Duration::from_secs(2);
const DAY: i64 = 86_400;

const PRIVATE_KEY_NOTE: &str = "Private key present. Corvene never reads private key material.";

/// A run of unchanged rows folded into one "N unchanged" row.
#[derive(Clone, Copy)]
struct Run {
    /// First row of the run.
    start: usize,
    /// One past the last row (children included).
    end: usize,
    /// Items at the run's own depth.
    count: usize,
    depth: usize,
}

/// One entry of the list.
#[derive(Clone, Copy)]
enum Entry {
    /// "No certificate or key fields changed."
    Empty,
    /// A fold row (index into `runs`) and whether it is open.
    Fold(usize, bool),
    /// An item (index into `rows`).
    Item(usize),
}

/// Changes counted for the header chips: an added or removed item counts
/// once with its children, a changed one when its own fields or title
/// changed.
#[derive(Clone, Copy, Default)]
struct Counts {
    added: usize,
    removed: usize,
    changed: usize,
}

impl Counts {
    fn any(&self) -> bool {
        self.added + self.removed + self.changed > 0
    }
}

/// Aligned rows with what the list needs from them.
#[derive(Default)]
struct Prepared {
    rows: Vec<ItemRow>,
    runs: Vec<Run>,
    counts: Counts,
}

fn prepare(previous: Option<&SigningFile>, current: Option<&SigningFile>) -> Prepared {
    let rows = corvene_models::signing::align(previous, current);
    let runs = unchanged_runs(&rows);
    let counts = count_changes(&rows);
    Prepared { rows, runs, counts }
}

/// Consecutive unchanged rows, each run starting at an unchanged item and
/// taking its children and the unchanged siblings after it.
fn unchanged_runs(rows: &[ItemRow]) -> Vec<Run> {
    let mut runs = Vec::new();
    let mut i = 0;
    while i < rows.len() {
        if rows[i].status != RowStatus::Unchanged {
            i += 1;
            continue;
        }
        let depth = rows[i].depth;
        let start = i;
        let mut count = 0;
        while i < rows.len()
            && (rows[i].depth > depth
                || (rows[i].depth == depth && rows[i].status == RowStatus::Unchanged))
        {
            if rows[i].depth == depth {
                count += 1;
            }
            i += 1;
        }
        runs.push(Run {
            start,
            end: i,
            count,
            depth,
        });
    }
    runs
}

fn count_changes(rows: &[ItemRow]) -> Counts {
    let mut counts = Counts::default();
    // the status of the latest row at each depth (the current ancestors)
    let mut stack: Vec<&RowStatus> = Vec::new();
    for (i, row) in rows.iter().enumerate() {
        stack.truncate(row.depth);
        let inside_one_sided = stack
            .iter()
            .any(|s| matches!(s, RowStatus::Added | RowStatus::Removed));
        match &row.status {
            RowStatus::Added if !inside_one_sided => counts.added += 1,
            RowStatus::Removed if !inside_one_sided => counts.removed += 1,
            RowStatus::Changed { fields } => {
                let own = !fields.is_empty()
                    || !rows[i + 1..]
                        .iter()
                        .take_while(|r| r.depth > row.depth)
                        .any(|r| r.status != RowStatus::Unchanged);
                if own {
                    counts.changed += 1;
                }
            }
            _ => {}
        }
        stack.push(&row.status);
    }
    counts
}

fn item_count(file: Option<&SigningFile>) -> usize {
    fn walk(items: &[corvene_models::signing::SigningItem]) -> usize {
        items.iter().map(|i| 1 + walk(&i.children)).sum()
    }
    file.map_or(0, |f| walk(&f.items))
}

fn kind_icon(kind: SigningItemKind) -> Octicon {
    use SigningItemKind as K;
    match kind {
        K::Certificate | K::SshCertificate => Octicon::Verified,
        K::CertificateRequest => Octicon::Unverified,
        K::RevocationList | K::RevokedCertificate => Octicon::ShieldX,
        K::PublicKey | K::PgpKey => Octicon::ShieldCheck,
        K::PrivateKey => Octicon::Lock,
        K::Profile | K::PgpSubkey => Octicon::Shield,
        K::Entitlements => Octicon::ListUnordered,
        K::Devices | K::Device => Octicon::DeviceDesktop,
        K::Certificates => Octicon::Stack,
        K::KeystoreEntry | K::Container => Octicon::Container,
        K::SshKey => Octicon::Terminal,
        K::PgpUserId | K::Signer => Octicon::Person,
        K::PgpUserAttribute => Octicon::Image,
        K::PgpSignature => Octicon::Pencil,
        K::PgpMessage => Octicon::Comment,
    }
}

/// The bar and title tint of a card.
fn status_colors(status: &RowStatus, t: &GhdTheme) -> (Hsla, Hsla) {
    match status {
        RowStatus::Added => (t.diff_add_border, t.diff_add_background),
        RowStatus::Removed => (t.diff_delete_border, t.diff_delete_background),
        RowStatus::Changed { .. } => (t.color_modified, t.color_modified.opacity(0.1)),
        RowStatus::Unchanged => (t.box_border, t.box_alt_background),
    }
}

fn status_label(status: &RowStatus) -> Option<&'static str> {
    match status {
        RowStatus::Added => Some("Added"),
        RowStatus::Removed => Some("Removed"),
        RowStatus::Changed { .. } => Some("Changed"),
        RowStatus::Unchanged => None,
    }
}

/// Fingerprints, serials, key IDs and similar: shown monospace with a copy
/// button.
fn is_digest(label: &str, value: &str) -> bool {
    let label = label.to_ascii_lowercase();
    const WORDS: [&str; 12] = [
        "fingerprint",
        "serial",
        "sha-",
        "sha1",
        "sha256",
        "key id",
        "keyid",
        "digest",
        "udid",
        "uuid",
        "identifier",
        "hash",
    ];
    if WORDS.iter().any(|w| label.contains(w)) {
        return true;
    }
    let hex = value
        .chars()
        .filter(|c| !matches!(c, ':' | ' ' | '-'))
        .count();
    hex >= 16
        && value
            .chars()
            .all(|c| c.is_ascii_hexdigit() || matches!(c, ':' | ' ' | '-'))
}

fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}

fn system_time(secs: i64) -> SystemTime {
    if secs >= 0 {
        UNIX_EPOCH + Duration::from_secs(secs as u64)
    } else {
        UNIX_EPOCH - Duration::from_secs(secs.unsigned_abs())
    }
}

fn days(n: i64) -> String {
    if n == 1 {
        "1 day".to_string()
    } else {
        format!("{} days", crate::format::format_count(n as u64))
    }
}

/// Expired, about to expire, or the plain expiry date.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Expiry {
    NotYetValid,
    Expired,
    Soon,
    Later,
}

fn expiry(row: &ItemRow, now: i64) -> Option<(Expiry, String)> {
    if let Some(start) = row.not_before
        && start > now
    {
        return Some((
            Expiry::NotYetValid,
            format!(
                "Valid from {}",
                crate::format::format_date(system_time(start))
            ),
        ));
    }
    let end = row.not_after?;
    Some(if end <= now {
        let ago = (now - end) / DAY;
        let text = if ago == 0 {
            "Expired today".to_string()
        } else if ago < 365 {
            format!("Expired {} ago", days(ago))
        } else {
            format!("Expired {}", crate::format::format_date(system_time(end)))
        };
        (Expiry::Expired, text)
    } else if end - now < EXPIRY_WARNING_DAYS * DAY {
        let left = (end - now) / DAY;
        let text = if left == 0 {
            "Expires today".to_string()
        } else {
            format!("Expires in {}", days(left))
        };
        (Expiry::Soon, text)
    } else {
        (
            Expiry::Later,
            format!("Expires {}", crate::format::format_date(system_time(end))),
        )
    })
}

/// The signing material diff view.
pub struct SigningDiffView {
    diff: Arc<Diff>,
    path: String,
    previous: Option<Arc<SigningFile>>,
    current: Option<Arc<SigningFile>>,
    prepared: Arc<Prepared>,
    /// Rows are being aligned on a background task.
    loading: bool,
    /// Bumped on every new alignment so an older task's result is dropped.
    generation: u64,
    entries: Rc<Vec<Entry>>,
    list: ListState,
    /// Runs (by their first row) the user opened.
    open_runs: HashSet<usize>,
    only_changes: bool,
    password: Option<Entity<InputState>>,
    unlocking: bool,
    /// The value last copied (row, field), while it shows its check mark.
    copied: Option<(usize, usize)>,
    copy_generation: u64,
}

impl SigningDiffView {
    /// `diff` is a `Diff::Signing`; any other diff shows as empty.
    pub fn new(
        diff: Arc<Diff>,
        context: crate::diff_view::InspectContext,
        cx: &mut Context<Self>,
    ) -> Self {
        let (previous, current) = match &*diff {
            Diff::Signing {
                previous, current, ..
            } => (previous.clone(), current.clone()),
            _ => (None, None),
        };
        let mut this = Self {
            diff,
            path: context.path,
            previous,
            current,
            prepared: Arc::new(Prepared::default()),
            loading: false,
            generation: 0,
            entries: Rc::new(Vec::new()),
            list: ListState::new(0, ListAlignment::Top, zpx(400.)),
            open_runs: HashSet::new(),
            only_changes: false,
            password: None,
            unlocking: false,
            copied: None,
            copy_generation: 0,
        };
        this.realign(cx);
        this
    }

    /// The diff shown.
    pub fn diff(&self) -> &Arc<Diff> {
        &self.diff
    }

    /// Aligns the two sides again: at once for small files, on a
    /// background task for big ones.
    fn realign(&mut self, cx: &mut Context<Self>) {
        self.generation += 1;
        let items = item_count(self.previous.as_deref()) + item_count(self.current.as_deref());
        if items <= BACKGROUND_ITEMS {
            self.set_prepared(prepare(self.previous.as_deref(), self.current.as_deref()));
            return;
        }
        self.loading = true;
        let (previous, current) = (self.previous.clone(), self.current.clone());
        let generation = self.generation;
        let task = cx
            .background_executor()
            .spawn(async move { prepare(previous.as_deref(), current.as_deref()) });
        cx.spawn(async move |this, cx| {
            let prepared = task.await;
            this.update(cx, |this, cx| {
                if this.generation == generation {
                    this.loading = false;
                    this.set_prepared(prepared);
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    fn set_prepared(&mut self, prepared: Prepared) {
        self.prepared = Arc::new(prepared);
        self.open_runs.clear();
        self.copied = None;
        self.build_entries();
        // open at the first change (a fold or row above kept in view)
        let rows = &self.prepared.rows;
        let first = self
            .entries
            .iter()
            .position(|e| matches!(e, Entry::Item(i) if rows[*i].status != RowStatus::Unchanged));
        if let Some(ix) = first {
            self.list.scroll_to(ListOffset {
                item_ix: ix.saturating_sub(1),
                offset_in_item: px(0.),
            });
        }
    }

    fn build_entries(&mut self) {
        let p = &self.prepared;
        let mut out = Vec::with_capacity(p.rows.len() + 1);
        if !p.counts.any() && !p.rows.is_empty() {
            out.push(Entry::Empty);
        }
        let mut runs = p.runs.iter().enumerate().peekable();
        let mut i = 0;
        while i < p.rows.len() {
            if let Some((ix, run)) = runs.next_if(|(_, r)| r.start == i) {
                let open = self.open_runs.contains(&run.start);
                out.push(Entry::Fold(ix, open));
                if open {
                    out.extend((run.start..run.end).map(Entry::Item));
                }
                i = run.end;
                continue;
            }
            out.push(Entry::Item(i));
            i += 1;
        }
        self.list.reset(out.len());
        self.entries = Rc::new(out);
    }

    fn toggle_run(&mut self, run: usize, cx: &mut Context<Self>) {
        let Some(start) = self.prepared.runs.get(run).map(|r| r.start) else {
            return;
        };
        if !self.open_runs.remove(&start) {
            self.open_runs.insert(start);
        }
        let top = self.list.logical_scroll_top();
        self.build_entries();
        self.list.scroll_to(top);
        cx.notify();
    }

    fn set_only_changes(&mut self, on: bool, cx: &mut Context<Self>) {
        self.only_changes = on;
        // rows change height
        self.list.reset(self.entries.len());
        cx.notify();
    }

    fn copy(&mut self, at: (usize, usize), value: String, cx: &mut Context<Self>) {
        cx.write_to_clipboard(ClipboardItem::new_string(value));
        self.copied = Some(at);
        self.copy_generation += 1;
        let generation = self.copy_generation;
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(COPIED_FOR).await;
            this.update(cx, |this, cx| {
                if this.copy_generation == generation {
                    this.copied = None;
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    fn locked(&self) -> impl Iterator<Item = &Arc<SigningFile>> {
        [&self.previous, &self.current]
            .into_iter()
            .flatten()
            .filter(|f| f.needs_password())
    }

    fn ensure_password(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<InputState> {
        if let Some(input) = &self.password {
            return input.clone();
        }
        let input = cx.new(|cx| {
            InputState::new(window, cx)
                .masked(true)
                .placeholder("Password")
        });
        cx.subscribe_in(&input, window, |this, _, ev: &InputEvent, window, cx| {
            if let InputEvent::PressEnter { .. } = ev {
                this.unlock(window, cx);
            }
        })
        .detach();
        self.password = Some(input.clone());
        input
    }

    /// Reads every locked side again with the typed password. The field is
    /// emptied at once and the password lives only in the background task.
    fn unlock(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.unlocking {
            return;
        }
        let Some(input) = self.password.clone() else {
            return;
        };
        let password = input.read(cx).value().to_string();
        input.update(cx, |input, cx| input.set_value("", window, cx));
        if password.is_empty() {
            return;
        }
        let locked = |f: &Option<Arc<SigningFile>>| f.clone().filter(|f| f.needs_password());
        let (previous, current) = (locked(&self.previous), locked(&self.current));
        if previous.is_none() && current.is_none() {
            return;
        }
        self.unlocking = true;
        let path = self.path.clone();
        let task = cx.background_executor().spawn(async move {
            let open = |file: Option<Arc<SigningFile>>| {
                file.map(|file| {
                    match corvene_core::inspect_diff::unlock_signing_file(&path, &file, &password) {
                        Some(read) => Arc::new(read),
                        None => {
                            let mut wrong = (*file).clone();
                            wrong.password = PasswordState::Wrong;
                            Arc::new(wrong)
                        }
                    }
                })
            };
            let result = (open(previous), open(current));
            // overwrite the bytes before they go back to the allocator
            let mut bytes = password.into_bytes();
            bytes.iter_mut().for_each(|b| *b = 0);
            std::hint::black_box(&bytes);
            drop(bytes);
            result
        });
        cx.spawn(async move |this, cx| {
            let (previous, current) = task.await;
            this.update(cx, |this, cx| {
                this.unlocking = false;
                if previous.is_some() {
                    this.previous = previous;
                }
                if current.is_some() {
                    this.current = current;
                }
                this.realign(cx);
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }
}

/// What a row draws from, shared with the list's row callback.
struct Rows {
    prepared: Arc<Prepared>,
    entries: Rc<Vec<Entry>>,
    only_changes: bool,
    side_by_side: bool,
    copied: Option<(usize, usize)>,
    now: i64,
    view: WeakEntity<SigningDiffView>,
}

/// A small bordered label ("Certificate", "Changed").
fn chip(text: impl Into<SharedString>, color: Hsla, border: Hsla) -> Div {
    div()
        .flex_none()
        .px(zpx(4.))
        .rounded(zpx(4.))
        .border_1()
        .border_color(border)
        .text_size(zpx(10.))
        .line_height(zpx(14.))
        .font_weight(FontWeight::MEDIUM)
        .text_color(color)
        .child(text.into())
}

impl Rows {
    fn row(&self, ix: usize, cx: &mut App) -> AnyElement {
        match self.entries.get(ix).copied() {
            Some(Entry::Empty) => self.empty_note(cx),
            Some(Entry::Fold(run, open)) => self.fold(run, open, cx),
            Some(Entry::Item(row)) => self.item(row, cx),
            None => div().into_any_element(),
        }
    }

    fn empty_note(&self, cx: &App) -> AnyElement {
        let t = cx.ghd();
        div()
            .w_full()
            .flex()
            .flex_row()
            .items_center()
            .gap(SPACING_HALF())
            .px(SPACING())
            .pt(SPACING())
            .text_color(t.text_secondary)
            .child(octicon(Octicon::Info, t.text_secondary))
            .child("No certificate or key fields changed.")
            .into_any_element()
    }

    fn fold(&self, run: usize, open: bool, cx: &App) -> AnyElement {
        let t = cx.ghd();
        let Some(r) = self.prepared.runs.get(run).copied() else {
            return div().into_any_element();
        };
        let count = crate::format::format_count(r.count as u64);
        let label = if open {
            format!("Hide {count} unchanged")
        } else {
            format!("{count} unchanged")
        };
        let view = self.view.clone();
        div()
            .w_full()
            .pl(SPACING() + zpx(INDENT * r.depth as f32))
            .pr(SPACING())
            .pt(SPACING_HALF())
            .child(
                div()
                    .id(("signing-fold", r.start))
                    .a11y_button(label.clone())
                    .h(zpx(24.))
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(SPACING_HALF())
                    .px(SPACING_HALF())
                    .rounded(BORDER_RADIUS())
                    .border_1()
                    .border_color(t.box_border)
                    .border_dashed()
                    .text_size(FONT_SIZE_SM())
                    .text_color(t.text_secondary)
                    .cursor_pointer()
                    .hover(|d| d.bg(t.box_hover_background))
                    .child(octicon(
                        if open { Octicon::Fold } else { Octicon::Unfold },
                        t.text_secondary,
                    ))
                    .child(label)
                    .control_focus(cx)
                    .on_click(move |_, _, cx| {
                        view.update(cx, |this, cx| this.toggle_run(run, cx)).ok();
                    }),
            )
            .into_any_element()
    }

    fn expiry_badge(&self, ix: usize, row: &ItemRow, t: &GhdTheme) -> Option<Stateful<Div>> {
        let (kind, text) = expiry(row, self.now)?;
        let color = match kind {
            Expiry::Expired => t.color_deleted,
            Expiry::Soon => t.file_warning,
            Expiry::NotYetValid => t.color_modified,
            Expiry::Later => t.text_secondary,
        };
        let border = if kind == Expiry::Later {
            t.box_border
        } else {
            color.opacity(0.5)
        };
        let mut tooltip = Vec::new();
        if let Some(start) = row.not_before {
            tooltip.push(format!(
                "Valid from {}",
                crate::format::format_date_time(system_time(start))
            ));
        }
        if let Some(end) = row.not_after {
            tooltip.push(format!(
                "Valid until {}",
                crate::format::format_date_time(system_time(end))
            ));
        }
        Some(
            chip(text, color, border)
                .id(("signing-expiry", ix))
                .ghd_tooltip(tooltip.join("\n")),
        )
    }

    fn item(&self, ix: usize, cx: &mut App) -> AnyElement {
        let t = cx.ghd().clone();
        let Some(row) = self.prepared.rows.get(ix) else {
            return div().into_any_element();
        };
        let (bar, tint) = status_colors(&row.status, &t);
        let changed = matches!(row.status, RowStatus::Changed { .. });
        let removed = row.status == RowStatus::Removed;

        let title = div()
            .flex()
            .flex_row()
            .items_center()
            .gap(SPACING_HALF())
            .min_h(zpx(26.))
            .px(SPACING_HALF())
            .bg(tint)
            .child(octicon(kind_icon(row.kind), t.text_secondary))
            .child(
                div()
                    .id(("signing-title", ix))
                    .flex_shrink(1.)
                    .min_w(zpx(40.))
                    .truncate()
                    .font_weight(FontWeight::SEMIBOLD)
                    .when(removed, |d| d.line_through().text_color(t.text_secondary))
                    .child(if row.title.is_empty() {
                        row.kind.display_name().to_string()
                    } else {
                        row.title.clone()
                    })
                    .ghd_tooltip(row.title.clone()),
            )
            .when(!row.title.is_empty(), |d| {
                d.child(chip(
                    row.kind.display_name(),
                    t.text_secondary,
                    t.box_border,
                ))
            })
            .children(self.expiry_badge(ix, row, &t))
            .child(div().flex_1())
            .children(status_label(&row.status).map(|label| {
                div()
                    .flex_none()
                    .text_size(FONT_SIZE_SM())
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(bar)
                    .child(label)
            }));

        let private = (row.kind == SigningItemKind::PrivateKey).then(|| {
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap(SPACING_HALF())
                .px(SPACING_HALF())
                .py(zpx(3.))
                .text_size(FONT_SIZE_SM())
                .text_color(t.text_secondary)
                .child(octicon(Octicon::Lock, t.text_secondary))
                .child(PRIVATE_KEY_NOTE)
        });

        let fields: Vec<AnyElement> = row
            .fields
            .iter()
            .enumerate()
            .filter(|(_, f)| !(changed && self.only_changes && !f.changed()))
            .map(|(fx, f)| self.field(ix, fx, f, changed, &t, cx))
            .collect();
        let has_fields = !fields.is_empty();

        div()
            .w_full()
            .pl(SPACING() + zpx(INDENT * row.depth as f32))
            .pr(SPACING())
            .pt(SPACING_HALF())
            .child(
                div()
                    .id(("signing-card", ix))
                    .w_full()
                    .flex()
                    .flex_row()
                    .rounded(BORDER_RADIUS())
                    .border_1()
                    .border_color(t.box_border)
                    .overflow_hidden()
                    .bg(t.background)
                    .child(div().flex_none().w(zpx(3.)).bg(bar))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .child(title)
                            .children(private)
                            .when(has_fields, |d| {
                                d.child(
                                    div()
                                        .flex()
                                        .flex_col()
                                        .px(SPACING_HALF())
                                        .py(zpx(2.))
                                        .border_t_1()
                                        .border_color(t.box_border.opacity(0.6))
                                        .children(fields),
                                )
                            }),
                    ),
            )
            .into_any_element()
    }

    fn field(
        &self,
        ix: usize,
        fx: usize,
        f: &FieldRow,
        in_changed: bool,
        t: &GhdTheme,
        cx: &App,
    ) -> AnyElement {
        let shown = f.new.as_deref().or(f.old.as_deref()).unwrap_or_default();
        let digest = is_digest(&f.label, shown);
        let changed = in_changed && f.changed();
        let value = |v: &str, color: Hsla, bg: Option<Hsla>, strike: bool| {
            div()
                .flex_1()
                .min_w_0()
                .px(zpx(4.))
                .py(zpx(1.))
                .rounded(zpx(3.))
                .when_some(bg, |d, bg| d.bg(bg))
                .when(strike, |d| d.line_through())
                .when(digest, |d| {
                    d.font_family(mono_font()).text_size(FONT_SIZE_SM())
                })
                .text_color(color)
                .child(v.to_string())
        };
        let values = if !changed {
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .child(value(shown, t.text, None, false))
        } else {
            let old = f
                .old
                .as_deref()
                .map(|v| value(v, t.diff_delete_text, Some(t.diff_delete_background), true));
            let new = f
                .new
                .as_deref()
                .map(|v| value(v, t.diff_add_text, Some(t.diff_add_background), false));
            let missing = || {
                div()
                    .flex_1()
                    .px(zpx(4.))
                    .py(zpx(1.))
                    .text_color(t.text_secondary)
                    .child("—")
            };
            if self.side_by_side {
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_row()
                    .gap(SPACING_HALF())
                    .child(old.unwrap_or_else(missing))
                    .child(new.unwrap_or_else(missing))
            } else {
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(zpx(1.))
                    .children(old)
                    .children(new)
            }
        };

        let copied = self.copied == Some((ix, fx));
        let group = SharedString::from(format!("signing-field-{ix}-{fx}"));
        let copy_value = shown.to_string();
        let copy = digest.then(|| {
            let view = self.view.clone();
            let value = copy_value.clone();
            div()
                .id(ElementId::Name(format!("signing-copy-{ix}-{fx}").into()))
                .a11y_button(if copied { "Copied!" } else { "Copy value" })
                .flex_none()
                .size(zpx(20.))
                .flex()
                .items_center()
                .justify_center()
                .rounded(BORDER_RADIUS())
                .cursor_pointer()
                .opacity(if copied { 1. } else { 0. })
                .group_hover(group.clone(), |s| s.opacity(1.))
                .hover(|d| d.bg(t.box_hover_background))
                .child(octicon(
                    if copied {
                        Octicon::Check
                    } else {
                        Octicon::Copy
                    },
                    t.text_secondary,
                ))
                .ghd_tooltip(if copied { "Copied!" } else { "Copy value" })
                .control_focus(cx)
                .on_click(move |_, _, cx| {
                    let value = value.clone();
                    view.update(cx, |this, cx| this.copy((ix, fx), value, cx))
                        .ok();
                })
        });

        let view = self.view.clone();
        div()
            .id(ElementId::Name(group.clone()))
            .group(group)
            .w_full()
            .flex()
            .flex_row()
            .items_start()
            .gap(SPACING_HALF())
            .py(zpx(2.))
            .text_size(FONT_SIZE())
            .child(
                div()
                    .w(zpx(LABEL_WIDTH))
                    .flex_none()
                    .py(zpx(1.))
                    .text_color(t.text_secondary)
                    .when(changed, |d| d.font_weight(FontWeight::SEMIBOLD))
                    .child(f.label.clone()),
            )
            .child(values)
            .child(div().w(zpx(20.)).flex_none().children(copy))
            .on_mouse_down(
                MouseButton::Right,
                move |ev: &MouseDownEvent, window, cx| {
                    cx.stop_propagation();
                    use crate::context_menu::MenuItem;
                    let view = view.clone();
                    let value = copy_value.clone();
                    let items = vec![MenuItem::new("Copy value", move |_, cx| {
                        let value = value.clone();
                        view.update(cx, |this, cx| this.copy((ix, fx), value, cx))
                            .ok();
                    })];
                    crate::native_menu::show_context_menu(items, ev.position, window, cx);
                },
            )
            .into_any_element()
    }
}

impl SigningDiffView {
    fn kind_name(&self) -> &'static str {
        self.current
            .as_ref()
            .or(self.previous.as_ref())
            .map_or("Signing material", |f| f.kind.display_name())
    }

    fn notes(&self) -> Vec<String> {
        let mut notes: Vec<String> = Vec::new();
        for file in [&self.previous, &self.current].into_iter().flatten() {
            for note in &file.notes {
                if !notes.contains(note) {
                    notes.push(note.clone());
                }
            }
        }
        notes
    }

    fn header_strip(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let t = cx.ghd().clone();
        let counts = self.prepared.counts;
        let top = |f: &Option<Arc<SigningFile>>| f.as_ref().map(|f| f.items.len());
        let items = match (top(&self.previous), top(&self.current)) {
            (Some(a), Some(b)) if a != b => format!(
                "{} → {} items",
                crate::format::format_count(a as u64),
                crate::format::format_count(b as u64)
            ),
            (_, Some(n)) | (Some(n), None) => {
                if n == 1 {
                    "1 item".to_string()
                } else {
                    format!("{} items", crate::format::format_count(n as u64))
                }
            }
            (None, None) => String::new(),
        };
        let pill = |icon: Octicon, n: usize, word: &str, color: Hsla| {
            div()
                .flex_none()
                .flex()
                .flex_row()
                .items_center()
                .gap(zpx(3.))
                .px(SPACING_HALF())
                .h(zpx(20.))
                .text_size(FONT_SIZE_SM())
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(color)
                .child(octicon(icon, color))
                .child(format!("{} {word}", crate::format::format_count(n as u64)))
        };
        let mut pills = Vec::new();
        if counts.added > 0 {
            pills.push(pill(Octicon::DiffAdded, counts.added, "added", t.color_new));
        }
        if counts.removed > 0 {
            pills.push(pill(
                Octicon::DiffRemoved,
                counts.removed,
                "removed",
                t.color_deleted,
            ));
        }
        if counts.changed > 0 {
            pills.push(pill(
                Octicon::DiffModified,
                counts.changed,
                "changed",
                t.color_modified,
            ));
        }

        let only = self.only_changes;
        let weak = cx.weak_entity();
        let toggle = (counts.changed > 0).then(|| {
            div()
                .id("signing-only-changes")
                .a11y_button("Show only changes")
                .flex_none()
                .h(zpx(22.))
                .px(SPACING_HALF())
                .flex()
                .items_center()
                .rounded(BORDER_RADIUS())
                .border_1()
                .text_size(FONT_SIZE_SM())
                .cursor_pointer()
                .when(only, |d| {
                    d.bg(t.box_selected_active_background)
                        .border_color(t.box_selected_active_background)
                        .text_color(t.box_selected_active_text)
                })
                .when(!only, |d| {
                    d.border_color(t.box_border)
                        .text_color(t.text)
                        .hover(|d| d.bg(t.box_hover_background))
                })
                .child("Show only changes")
                .ghd_tooltip(if only {
                    "Show every field of a changed item"
                } else {
                    "Hide the fields that did not change"
                })
                .control_focus(cx)
                .on_click(move |_, _, cx| {
                    weak.update(cx, |this, cx| this.set_only_changes(!only, cx))
                        .ok();
                })
        });

        let notes: Vec<AnyElement> = self
            .notes()
            .into_iter()
            .map(|note| {
                div()
                    .flex()
                    .flex_row()
                    .items_start()
                    .gap(SPACING_HALF())
                    .text_size(FONT_SIZE_SM())
                    .text_color(t.text_secondary)
                    .child(octicon(Octicon::Info, t.text_secondary))
                    .child(div().flex_1().min_w_0().child(note))
                    .into_any_element()
            })
            .collect();

        div()
            .flex_none()
            .flex()
            .flex_col()
            .gap(zpx(3.))
            .px(SPACING())
            .py(zpx(4.))
            .bg(t.box_alt_background)
            .border_b_1()
            .border_color(t.diff_border)
            .child(
                div()
                    .flex()
                    .flex_row()
                    .flex_wrap()
                    .items_center()
                    .gap(SPACING_HALF())
                    .min_h(zpx(22.))
                    .child(octicon(Octicon::ShieldCheck, t.text_secondary))
                    .child(
                        div()
                            .flex_none()
                            .text_size(FONT_SIZE_SM())
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(t.text)
                            .child(self.kind_name()),
                    )
                    .when(!items.is_empty(), |d| {
                        d.child(
                            div()
                                .flex_none()
                                .text_size(FONT_SIZE_SM())
                                .text_color(t.text_secondary)
                                .child(items),
                        )
                    })
                    .children(pills)
                    .child(div().flex_1())
                    .children(toggle),
            )
            .children(notes)
            .into_any_element()
    }

    /// The inline password field for locked sides.
    fn password_panel(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let locked: Vec<PasswordState> = self.locked().map(|f| f.password).collect();
        if locked.is_empty() {
            return None;
        }
        let wrong = locked.contains(&PasswordState::Wrong);
        let t = cx.ghd().clone();
        let input = self.ensure_password(window, cx);
        let unlocking = self.unlocking;
        let weak = cx.weak_entity();
        Some(
            div()
                .flex_none()
                .flex()
                .flex_col()
                .gap(SPACING_HALF())
                .mx(SPACING())
                .mt(SPACING())
                .p(SPACING())
                .rounded(BORDER_RADIUS())
                .border_1()
                .border_color(if wrong {
                    t.color_deleted.opacity(0.6)
                } else {
                    t.box_border
                })
                .bg(t.box_alt_background)
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(SPACING_HALF())
                        .child(octicon(Octicon::Lock, t.text_secondary))
                        .child(
                            "This file is protected with a password. Enter it to see its \
                             certificates.",
                        ),
                )
                .when(wrong, |d| {
                    d.child(
                        div()
                            .text_size(FONT_SIZE_SM())
                            .text_color(t.input_error_text)
                            .child("That password is not correct."),
                    )
                })
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(SPACING_HALF())
                        .child(div().w(zpx(240.)).child(crate::widgets::password_text_box(
                            "signing-password",
                            &input,
                            window,
                            cx,
                        )))
                        .child(
                            crate::widgets::primary_button(
                                "signing-unlock",
                                if unlocking { "Unlocking…" } else { "Unlock" },
                                unlocking,
                                cx,
                            )
                            .on_click(move |_, window, cx| {
                                weak.update(cx, |this, cx| this.unlock(window, cx)).ok();
                            }),
                        ),
                )
                .into_any_element(),
        )
    }

    fn placeholder(&self, text: &'static str, cx: &App) -> AnyElement {
        let t = cx.ghd();
        div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap(SPACING_HALF())
            .p(SPACING_DOUBLE())
            .text_color(t.text_secondary)
            .child(octicon(Octicon::ShieldCheck, t.text_secondary).size(zpx(24.)))
            .child(text)
            .into_any_element()
    }
}

impl Render for SigningDiffView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.ghd().clone();
        let side_by_side = corvene_core::AppState::try_global(cx)
            .is_some_and(|s| s.read(cx).settings.show_side_by_side_diff);
        let header = self.header_strip(cx);
        let password = self.password_panel(window, cx);
        let body = if self.loading {
            self.placeholder("Reading certificates and keys…", cx)
        } else if self.entries.is_empty() {
            if password.is_some() {
                div().flex_1().into_any_element()
            } else {
                self.placeholder("No certificate or key fields changed.", cx)
            }
        } else {
            let rows = Rc::new(Rows {
                prepared: self.prepared.clone(),
                entries: self.entries.clone(),
                only_changes: self.only_changes,
                side_by_side,
                copied: self.copied,
                now: now_secs(),
                view: cx.weak_entity(),
            });
            div()
                .relative()
                .flex_1()
                .min_h_0()
                .flex()
                .flex_col()
                .child(
                    list(self.list.clone(), move |ix, _window, cx| rows.row(ix, cx))
                        .flex_1()
                        .min_h_0()
                        .w_full()
                        .pb(SPACING())
                        .pr(gutter(&self.list)),
                )
                .child(scrollbar("signing-scrollbar", self.list.clone()))
                .into_any_element()
        };
        div()
            .id("signing-diff")
            .flex_1()
            .min_h_0()
            .min_w_0()
            .flex()
            .flex_col()
            .bg(t.background)
            .text_color(t.text)
            .text_size(FONT_SIZE())
            .child(header)
            .children(password)
            .child(body)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    // gpui's prelude has a `test` macro of its own
    use core::prelude::v1::test;
    use corvene_models::signing::SigningItem;

    fn item(key: &str, serial: &str) -> SigningItem {
        let mut i = SigningItem::new(SigningItemKind::Certificate, key, key);
        i.field("Serial", serial);
        i
    }

    fn file(items: Vec<SigningItem>) -> SigningFile {
        SigningFile {
            items,
            ..SigningFile::default()
        }
    }

    #[test]
    fn runs_and_counts() {
        let mut parent = item("p", "1");
        parent.children.push(item("c", "1"));
        let mut added = item("n", "1");
        added.children.push(item("nc", "1"));
        let old = file(vec![
            item("a", "1"),
            item("b", "1"),
            parent.clone(),
            item("x", "1"),
        ]);
        let new = file(vec![
            item("a", "1"),
            item("b", "2"),
            parent,
            item("x", "1"),
            added,
        ]);
        let p = prepare(Some(&old), Some(&new));
        // a | b changed | p, c, x unchanged | n, nc added
        assert_eq!(p.runs.len(), 2);
        assert_eq!((p.runs[0].start, p.runs[0].end, p.runs[0].count), (0, 1, 1));
        assert_eq!((p.runs[1].start, p.runs[1].end, p.runs[1].count), (2, 5, 2));
        assert_eq!(p.counts.added, 1);
        assert_eq!(p.counts.changed, 1);
        assert_eq!(p.counts.removed, 0);
    }

    #[test]
    fn expiry_wording() {
        let mut row = prepare(None, Some(&file(vec![item("a", "1")])))
            .rows
            .remove(0);
        let now = 1_000_000_000;
        row.not_after = Some(now - 3 * DAY - 10);
        assert_eq!(
            expiry(&row, now).map(|e| e.1),
            Some("Expired 3 days ago".into())
        );
        row.not_after = Some(now + 12 * DAY + 10);
        assert_eq!(
            expiry(&row, now).map(|e| e.1),
            Some("Expires in 12 days".into())
        );
        row.not_after = None;
        assert!(expiry(&row, now).is_none());
    }

    #[test]
    fn digests() {
        assert!(is_digest("SHA-256 fingerprint", "x"));
        assert!(is_digest("Thing", "AB:CD:EF:01:23:45:67:89"));
        assert!(!is_digest("Subject", "CN=example.com"));
    }
}
