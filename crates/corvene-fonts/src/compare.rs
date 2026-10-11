//! Two font files compared face by face, glyph by glyph.
//!
//! [`compare`] unpacks both sides and pairs their faces cheaply (by
//! PostScript name, then title, then position) with a hash of each face's
//! tables, so a collection's face list shows at once. [`FontDiff::face`]
//! then reads one pair in full; the UI calls it off the main thread for the
//! face on screen only.
//!
//! Glyphs pair by name, then by their first code point (a rename), then by
//! glyph id when neither font names its glyphs.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::sync::Arc;

use crate::container::{Decoded, decode};
use crate::layout::{Feature, LayoutReport, LayoutTable, RuleKind};
use crate::report::{FaceReport, FaceSummary, summary};
use crate::{FontError, blocks};

/// What happened to a face, glyph, table, feature, name record or field.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Status {
    Added,
    Removed,
    Modified,
    Unchanged,
}

impl Status {
    /// From the presence and equality of the two sides.
    pub fn of<T: PartialEq>(old: Option<&T>, new: Option<&T>) -> Status {
        match (old, new) {
            (None, Some(_)) => Status::Added,
            (Some(_), None) => Status::Removed,
            (Some(a), Some(b)) if a != b => Status::Modified,
            _ => Status::Unchanged,
        }
    }

    pub fn is_change(self) -> bool {
        self != Status::Unchanged
    }
}

/// A labelled value on each side (`None` where that side lacks it).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Field {
    pub label: String,
    pub old: Option<String>,
    pub new: Option<String>,
}

impl Field {
    pub fn new(label: impl Into<String>, old: Option<String>, new: Option<String>) -> Self {
        Self {
            label: label.into(),
            old,
            new,
        }
    }

    pub fn status(&self) -> Status {
        Status::of(self.old.as_ref(), self.new.as_ref())
    }
}

/// Labelled values of each side merged into fields: the old side's order,
/// then labels only the new side has. Repeated labels pair up in order.
pub fn merge_fields(
    old: Option<&[(String, String)]>,
    new: Option<&[(String, String)]>,
) -> Vec<Field> {
    let mut out: Vec<Field> = Vec::new();
    // label → indexes into `out` not yet given a new value
    let mut open: HashMap<&str, std::collections::VecDeque<usize>> = HashMap::new();
    for (label, value) in old.unwrap_or_default() {
        open.entry(label.as_str()).or_default().push_back(out.len());
        out.push(Field::new(label.clone(), Some(value.clone()), None));
    }
    for (label, value) in new.unwrap_or_default() {
        match open.get_mut(label.as_str()).and_then(|q| q.pop_front()) {
            Some(i) => out[i].new = Some(value.clone()),
            None => out.push(Field::new(label.clone(), None, Some(value.clone()))),
        }
    }
    out
}

/// A face of the old file paired with one of the new.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FacePair {
    pub old: Option<FaceSummary>,
    pub new: Option<FaceSummary>,
    pub status: Status,
    /// The new side's title (the old one's for a removed face).
    pub title: String,
}

/// Two font files, unpacked, with their faces paired.
#[derive(Clone, Debug)]
pub struct FontDiff {
    pub old: Option<Decoded>,
    pub new: Option<Decoded>,
    /// Why a side present in git could not be read.
    pub old_error: Option<FontError>,
    pub new_error: Option<FontError>,
    pub faces: Vec<FacePair>,
}

/// Unpacks both sides (`None` for a missing side: a new or deleted file)
/// and pairs their faces. Fails when no side can be read as a font.
pub fn compare(old: Option<&[u8]>, new: Option<&[u8]>) -> Result<FontDiff, FontError> {
    let read = |bytes: Option<&[u8]>| match bytes.map(decode) {
        None => (None, None),
        Some(Ok(d)) => (Some(d), None),
        Some(Err(e)) => (None, Some(e)),
    };
    let (old, old_error) = read(old);
    let (new, new_error) = read(new);
    if old.is_none() && new.is_none() {
        return Err(new_error.or(old_error).unwrap_or(FontError::NotAFont));
    }
    let summaries = |d: &Option<Decoded>| -> Vec<FaceSummary> {
        let Some(d) = d else { return Vec::new() };
        (0..d.face_count)
            .filter_map(|i| d.face(i).ok().map(|f| summary(&f, i)))
            .collect()
    };
    let faces = pair_faces(summaries(&old), summaries(&new));
    Ok(FontDiff {
        old,
        new,
        old_error,
        new_error,
        faces,
    })
}

fn pair_faces(old: Vec<FaceSummary>, new: Vec<FaceSummary>) -> Vec<FacePair> {
    let mut old: Vec<Option<FaceSummary>> = old.into_iter().map(Some).collect();
    let mut out: Vec<FacePair> = Vec::new();
    let mut unmatched_new = Vec::new();
    for n in new {
        let by =
            |f: &dyn Fn(&FaceSummary) -> bool| old.iter().position(|o| o.as_ref().is_some_and(f));
        let found = n
            .ps_name
            .as_ref()
            .and_then(|ps| by(&|o: &FaceSummary| o.ps_name.as_ref() == Some(ps)))
            .or_else(|| by(&|o: &FaceSummary| o.title == n.title));
        match found.and_then(|i| old[i].take()) {
            Some(o) => out.push(FacePair {
                status: if o.hash == n.hash {
                    Status::Unchanged
                } else {
                    Status::Modified
                },
                title: n.title.clone(),
                old: Some(o),
                new: Some(n),
            }),
            None => unmatched_new.push(n),
        }
    }
    // what is left pairs by position (a single renamed font stays modified)
    let mut rest_old = old.into_iter().flatten().collect::<Vec<_>>().into_iter();
    for n in unmatched_new {
        match rest_old.next() {
            Some(o) if out.is_empty() || n.index == o.index => out.push(FacePair {
                status: if o.hash == n.hash {
                    Status::Unchanged
                } else {
                    Status::Modified
                },
                title: n.title.clone(),
                old: Some(o),
                new: Some(n),
            }),
            Some(o) => {
                out.push(FacePair {
                    status: Status::Removed,
                    title: o.title.clone(),
                    old: Some(o),
                    new: None,
                });
                out.push(added(n));
            }
            None => out.push(added(n)),
        }
    }
    for o in rest_old {
        out.push(FacePair {
            status: Status::Removed,
            title: o.title.clone(),
            old: Some(o),
            new: None,
        });
    }
    out.sort_by_key(|p| {
        (
            p.new.as_ref().map(|n| n.index).unwrap_or(u32::MAX),
            p.old.as_ref().map(|o| o.index).unwrap_or(u32::MAX),
        )
    });
    out
}

fn added(n: FaceSummary) -> FacePair {
    FacePair {
        status: Status::Added,
        title: n.title.clone(),
        old: None,
        new: Some(n),
    }
}

impl FontDiff {
    /// Reads face pair `i` in full (slow for large fonts: call it off the
    /// main thread).
    pub fn face(&self, i: usize) -> Option<FaceDiff> {
        let pair = self.faces.get(i)?;
        let read = |d: &Option<Decoded>, s: &Option<FaceSummary>| {
            let (d, s) = (d.as_ref()?, s.as_ref()?);
            let font = d.face(s.index).ok()?;
            Some(Arc::new(FaceReport::read(&font, s.index)))
        };
        Some(FaceDiff::new(
            read(&self.old, &pair.old),
            read(&self.new, &pair.new),
        ))
    }
}

/// What changed about a glyph present on both sides.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct GlyphChanges(pub u16);

impl GlyphChanges {
    pub const OUTLINE: u16 = 1;
    /// Only a component's base glyph changed.
    pub const COMPONENT_BASE: u16 = 1 << 1;
    pub const COMPONENTS: u16 = 1 << 2;
    pub const ADVANCE: u16 = 1 << 3;
    pub const SIDEBEARING: u16 = 1 << 4;
    pub const VARIATIONS: u16 = 1 << 5;
    pub const NAME: u16 = 1 << 6;
    pub const CODEPOINTS: u16 = 1 << 7;
    pub const COLOR: u16 = 1 << 8;

    pub fn has(self, bit: u16) -> bool {
        self.0 & bit != 0
    }

    pub fn is_empty(self) -> bool {
        self.0 == 0
    }

    /// Short labels for a tooltip ("outline, advance").
    pub fn labels(self) -> Vec<&'static str> {
        [
            (Self::OUTLINE, "outline"),
            (Self::COMPONENT_BASE, "component base"),
            (Self::COMPONENTS, "components"),
            (Self::ADVANCE, "advance width"),
            (Self::SIDEBEARING, "side bearing"),
            (Self::VARIATIONS, "variations"),
            (Self::NAME, "name"),
            (Self::CODEPOINTS, "code points"),
            (Self::COLOR, "colour"),
        ]
        .into_iter()
        .filter(|(bit, _)| self.has(*bit))
        .map(|(_, label)| label)
        .collect()
    }
}

/// One glyph of either side.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GlyphRow {
    pub old: Option<u32>,
    pub new: Option<u32>,
    /// The new side's name (the old one's for a removed glyph).
    pub name: String,
    /// The old name of a renamed glyph.
    pub old_name: Option<String>,
    /// The new side's code points (the old one's for a removed glyph).
    pub codepoints: Vec<u32>,
    pub status: Status,
    pub changes: GlyphChanges,
}

/// One table of either side's directory.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TableRow {
    pub tag: String,
    pub status: Status,
    pub old_len: Option<u32>,
    pub new_len: Option<u32>,
}

/// One `name` record of either side.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NameRow {
    pub name_id: u16,
    pub platform: u16,
    pub encoding: u16,
    pub language_tag: String,
    pub old: Option<String>,
    pub new: Option<String>,
}

impl NameRow {
    pub fn status(&self) -> Status {
        Status::of(self.old.as_ref(), self.new.as_ref())
    }
}

/// Code points of one Unicode block on each side.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BlockRow {
    pub name: &'static str,
    pub start: u32,
    pub end: u32,
    pub old_count: u32,
    pub new_count: u32,
    pub added: Vec<u32>,
    pub removed: Vec<u32>,
    /// Mapped on both sides, to differently named glyphs.
    pub remapped: Vec<u32>,
}

impl BlockRow {
    pub fn status(&self) -> Status {
        match (self.old_count, self.new_count) {
            (0, _) => Status::Added,
            (_, 0) => Status::Removed,
            _ if self.added.is_empty() && self.removed.is_empty() && self.remapped.is_empty() => {
                Status::Unchanged
            }
            _ => Status::Modified,
        }
    }

    /// Code points the block holds (for "x of y" coverage).
    pub fn size(&self) -> u32 {
        self.end - self.start + 1
    }
}

/// A substitution with glyph names.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct NamedRule {
    pub kind: RuleKind,
    pub input: Vec<String>,
    pub output: Vec<String>,
}

/// One feature tag of one layout table, on either side.
#[derive(Clone, Debug, PartialEq)]
pub struct FeatureRow {
    pub table: LayoutTable,
    pub tag: String,
    pub ui_name: Option<String>,
    pub status: Status,
    pub old: Option<Feature>,
    pub new: Option<Feature>,
    pub added_rules: Vec<NamedRule>,
    pub removed_rules: Vec<NamedRule>,
    pub unchanged_rules: usize,
    /// Whether the feature's lookups changed at all (including what this
    /// crate does not list: positioning other than kerning, contextual
    /// rules), compared with glyphs by name.
    pub lookups_changed: Option<bool>,
}

/// Feature tags a script / language turns on, per side.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScriptRow {
    pub table: LayoutTable,
    pub script: String,
    pub language: String,
    pub old: Option<Vec<String>>,
    pub new: Option<Vec<String>>,
}

impl ScriptRow {
    pub fn status(&self) -> Status {
        Status::of(self.old.as_ref(), self.new.as_ref())
    }
}

/// A kerning pair whose value differs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KernRow {
    pub left: String,
    pub right: String,
    pub old: Option<i32>,
    pub new: Option<i32>,
}

impl KernRow {
    pub fn delta(&self) -> i32 {
        self.new.unwrap_or(0) - self.old.unwrap_or(0)
    }
}

/// One axis or named instance, as fields.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ItemRow {
    pub name: String,
    pub status: Status,
    pub fields: Vec<Field>,
}

/// Counts for the header.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Summary {
    pub glyphs_added: u32,
    pub glyphs_removed: u32,
    pub glyphs_modified: u32,
    pub glyphs_unchanged: u32,
    pub codepoints_added: u32,
    pub codepoints_removed: u32,
    pub features_changed: u32,
    pub kerning_changed: u32,
    pub tables_changed: u32,
    pub names_changed: u32,
    pub axes_changed: u32,
    pub instances_changed: u32,
}

/// Two faces compared in full.
#[derive(Clone, Debug)]
pub struct FaceDiff {
    pub old: Option<Arc<FaceReport>>,
    pub new: Option<Arc<FaceReport>>,
    pub overview: Vec<Field>,
    pub tables: Vec<TableRow>,
    /// Per table both sides have fields for (or one side, for an added or
    /// removed table).
    pub table_fields: BTreeMap<String, Vec<Field>>,
    pub table_fields_truncated: BTreeSet<String>,
    pub names: Vec<NameRow>,
    pub coverage: Vec<BlockRow>,
    pub glyphs: Vec<GlyphRow>,
    pub features: Vec<FeatureRow>,
    pub scripts: Vec<ScriptRow>,
    /// Changed pairs, largest change first.
    pub kerning: Vec<KernRow>,
    pub kerning_counts: (usize, usize),
    pub kerning_truncated: bool,
    pub axes: Vec<ItemRow>,
    pub instances: Vec<ItemRow>,
    pub summary: Summary,
}

impl FaceDiff {
    /// Compares two reads of a face (`None` for a missing side).
    pub fn new(old: Option<Arc<FaceReport>>, new: Option<Arc<FaceReport>>) -> FaceDiff {
        let (o, n) = (old.as_deref(), new.as_deref());
        let overview = merge_fields(
            o.map(|r| r.overview.as_slice()),
            n.map(|r| r.overview.as_slice()),
        );
        let tables = table_rows(o, n);
        let (table_fields, table_fields_truncated) = table_field_diffs(o, n, &tables);
        let names = name_rows(o, n);
        let glyphs = glyph_rows(o, n);
        let coverage = coverage_rows(o, n);
        let mut features = feature_rows(o, n, LayoutTable::Gsub);
        features.extend(feature_rows(o, n, LayoutTable::Gpos));
        let scripts = script_rows(o, n);
        let (kerning, kerning_counts) = kern_rows(o, n);
        let axes = axis_rows(o, n);
        let instances = instance_rows(o, n);
        let mut summary = Summary::default();
        for g in &glyphs {
            match g.status {
                Status::Added => summary.glyphs_added += 1,
                Status::Removed => summary.glyphs_removed += 1,
                Status::Modified => summary.glyphs_modified += 1,
                Status::Unchanged => summary.glyphs_unchanged += 1,
            }
        }
        for b in &coverage {
            summary.codepoints_added += b.added.len() as u32;
            summary.codepoints_removed += b.removed.len() as u32;
        }
        summary.features_changed = features.iter().filter(|f| f.status.is_change()).count() as u32;
        summary.kerning_changed = kerning.len() as u32;
        summary.tables_changed = tables.iter().filter(|t| t.status.is_change()).count() as u32;
        summary.names_changed = names.iter().filter(|r| r.status().is_change()).count() as u32;
        summary.axes_changed = axes.iter().filter(|r| r.status.is_change()).count() as u32;
        summary.instances_changed =
            instances.iter().filter(|r| r.status.is_change()).count() as u32;
        FaceDiff {
            kerning_truncated: o.is_some_and(|r| r.kerning.truncated)
                || n.is_some_and(|r| r.kerning.truncated),
            old,
            new,
            overview,
            tables,
            table_fields,
            table_fields_truncated,
            names,
            coverage,
            glyphs,
            features,
            scripts,
            kerning,
            kerning_counts,
            axes,
            instances,
            summary,
        }
    }

    /// The glyph id of `name` on the old / new side.
    pub fn gid(&self, new_side: bool, name: &str) -> Option<u32> {
        let side = if new_side { &self.new } else { &self.old };
        side.as_ref()?.glyph_by_name(name)
    }
}

fn table_rows(o: Option<&FaceReport>, n: Option<&FaceReport>) -> Vec<TableRow> {
    let mut rows: BTreeMap<&str, TableRow> = BTreeMap::new();
    for t in o.map(|r| r.tables.as_slice()).unwrap_or_default() {
        rows.insert(
            &t.tag,
            TableRow {
                tag: t.tag.clone(),
                status: Status::Removed,
                old_len: Some(t.len),
                new_len: None,
            },
        );
    }
    let old_hash: HashMap<&str, u64> = o
        .map(|r| r.tables.iter().map(|t| (t.tag.as_str(), t.hash)).collect())
        .unwrap_or_default();
    for t in n.map(|r| r.tables.as_slice()).unwrap_or_default() {
        let row = rows.entry(&t.tag).or_insert_with(|| TableRow {
            tag: t.tag.clone(),
            status: Status::Added,
            old_len: None,
            new_len: None,
        });
        row.new_len = Some(t.len);
        if let Some(h) = old_hash.get(t.tag.as_str()) {
            row.status = if *h == t.hash {
                Status::Unchanged
            } else {
                Status::Modified
            };
        }
    }
    rows.into_values().collect()
}

fn table_field_diffs(
    o: Option<&FaceReport>,
    n: Option<&FaceReport>,
    tables: &[TableRow],
) -> (BTreeMap<String, Vec<Field>>, BTreeSet<String>) {
    let mut out = BTreeMap::new();
    let mut truncated = BTreeSet::new();
    for t in tables {
        let of = o.and_then(|r| r.table_fields.get(&t.tag));
        let nf = n.and_then(|r| r.table_fields.get(&t.tag));
        if of.is_none() && nf.is_none() {
            continue;
        }
        if of.is_some_and(|f| f.truncated) || nf.is_some_and(|f| f.truncated) {
            truncated.insert(t.tag.clone());
        }
        // an unchanged table's fields are the same on both sides
        let (of, nf) = if t.status == Status::Unchanged {
            (of.or(nf), of.or(nf))
        } else {
            (of, nf)
        };
        out.insert(
            t.tag.clone(),
            merge_fields(
                of.map(|f| f.fields.as_slice()),
                nf.map(|f| f.fields.as_slice()),
            ),
        );
    }
    (out, truncated)
}

fn name_rows(o: Option<&FaceReport>, n: Option<&FaceReport>) -> Vec<NameRow> {
    type Key = (u16, u16, u16, u16);
    let mut rows: BTreeMap<Key, NameRow> = BTreeMap::new();
    for e in o.map(|r| r.names.as_slice()).unwrap_or_default() {
        rows.insert(
            (e.name_id, e.platform, e.encoding, e.language),
            NameRow {
                name_id: e.name_id,
                platform: e.platform,
                encoding: e.encoding,
                language_tag: e.language_tag.clone(),
                old: Some(e.value.clone()),
                new: None,
            },
        );
    }
    for e in n.map(|r| r.names.as_slice()).unwrap_or_default() {
        rows.entry((e.name_id, e.platform, e.encoding, e.language))
            .or_insert_with(|| NameRow {
                name_id: e.name_id,
                platform: e.platform,
                encoding: e.encoding,
                language_tag: e.language_tag.clone(),
                old: None,
                new: None,
            })
            .new = Some(e.value.clone());
    }
    rows.into_values().collect()
}

/// Pairs glyph ids of the two sides: by name, then by first code point,
/// then (unnamed fonts) by id.
fn pair_glyphs(o: &FaceReport, n: &FaceReport) -> Vec<(Option<u32>, Option<u32>)> {
    let mut new_for_old: Vec<Option<u32>> = vec![None; o.glyphs.len()];
    let mut old_taken = vec![false; o.glyphs.len()];
    let mut new_taken = vec![false; n.glyphs.len()];
    let named = |r: &FaceReport, g: usize| !r.glyphs[g].synthesized_name;
    for (ni, g) in n.glyphs.iter().enumerate() {
        if !named(n, ni) {
            continue;
        }
        if let Some(&oi) = o.by_name.get(&g.name)
            && named(o, oi as usize)
            && !old_taken[oi as usize]
        {
            new_for_old[oi as usize] = Some(ni as u32);
            old_taken[oi as usize] = true;
            new_taken[ni] = true;
        }
    }
    let first_cp = |r: &FaceReport, i: usize| r.glyphs[i].codepoints.first().copied();
    let old_by_cp: HashMap<u32, usize> = (0..o.glyphs.len())
        .filter(|&i| !old_taken[i])
        .filter_map(|i| first_cp(o, i).map(|cp| (cp, i)))
        .collect();
    for (ni, taken) in new_taken.iter_mut().enumerate() {
        if *taken {
            continue;
        }
        if let Some(&oi) = first_cp(n, ni).and_then(|cp| old_by_cp.get(&cp))
            && !old_taken[oi]
        {
            new_for_old[oi] = Some(ni as u32);
            old_taken[oi] = true;
            *taken = true;
        }
    }
    for ni in 0..n.glyphs.len().min(o.glyphs.len()) {
        if !new_taken[ni] && !old_taken[ni] && !named(n, ni) && !named(o, ni) {
            new_for_old[ni] = Some(ni as u32);
            old_taken[ni] = true;
            new_taken[ni] = true;
        }
    }
    let mut out: Vec<(Option<u32>, Option<u32>)> = Vec::new();
    for (oi, ni) in new_for_old.iter().enumerate() {
        out.push((Some(oi as u32), *ni));
    }
    for (ni, taken) in new_taken.iter().enumerate() {
        if !taken {
            out.push((None, Some(ni as u32)));
        }
    }
    out
}

fn glyph_rows(o: Option<&FaceReport>, n: Option<&FaceReport>) -> Vec<GlyphRow> {
    let pairs: Vec<(Option<u32>, Option<u32>)> = match (o, n) {
        (Some(o), Some(n)) => pair_glyphs(o, n),
        (Some(o), None) => (0..o.glyphs.len() as u32)
            .map(|g| (Some(g), None))
            .collect(),
        (None, Some(n)) => (0..n.glyphs.len() as u32)
            .map(|g| (None, Some(g)))
            .collect(),
        (None, None) => Vec::new(),
    };
    let mut rows: Vec<GlyphRow> = pairs
        .into_iter()
        .filter_map(|(og, ng)| {
            let old = og.and_then(|g| o?.glyphs.get(g as usize));
            let new = ng.and_then(|g| n?.glyphs.get(g as usize));
            let (status, changes, name, old_name, codepoints) = match (old, new) {
                (Some(a), Some(b)) => {
                    let mut c = 0u16;
                    if a.own_hash != b.own_hash {
                        c |= if a.components.is_empty() && b.components.is_empty() {
                            GlyphChanges::OUTLINE
                        } else {
                            GlyphChanges::COMPONENTS
                        };
                    } else if a.flat_hash != b.flat_hash {
                        c |= GlyphChanges::COMPONENT_BASE;
                    }
                    if (a.advance - b.advance).abs() > 0.01 {
                        c |= GlyphChanges::ADVANCE;
                    }
                    if (a.lsb - b.lsb).abs() > 0.01
                        && c & (GlyphChanges::OUTLINE | GlyphChanges::COMPONENTS) == 0
                    {
                        c |= GlyphChanges::SIDEBEARING;
                    }
                    if a.var_hash != b.var_hash {
                        c |= GlyphChanges::VARIATIONS;
                    }
                    if a.name != b.name {
                        c |= GlyphChanges::NAME;
                    }
                    if a.codepoints != b.codepoints {
                        c |= GlyphChanges::CODEPOINTS;
                    }
                    if a.color != b.color {
                        c |= GlyphChanges::COLOR;
                    }
                    let changes = GlyphChanges(c);
                    (
                        if changes.is_empty() {
                            Status::Unchanged
                        } else {
                            Status::Modified
                        },
                        changes,
                        b.name.clone(),
                        (a.name != b.name).then(|| a.name.clone()),
                        b.codepoints.clone(),
                    )
                }
                (Some(a), None) => (
                    Status::Removed,
                    GlyphChanges::default(),
                    a.name.clone(),
                    None,
                    a.codepoints.clone(),
                ),
                (None, Some(b)) => (
                    Status::Added,
                    GlyphChanges::default(),
                    b.name.clone(),
                    None,
                    b.codepoints.clone(),
                ),
                (None, None) => return None,
            };
            Some(GlyphRow {
                old: og.filter(|_| old.is_some()),
                new: ng.filter(|_| new.is_some()),
                name,
                old_name,
                codepoints,
                status,
                changes,
            })
        })
        .collect();
    // new glyph order; a removed glyph goes after the glyph that preceded
    // it in the old font
    let mut prev_new: Vec<u32> = Vec::new();
    if let Some(o) = o {
        let mut last = 0u32;
        let mut new_of_old = vec![None; o.glyphs.len()];
        for r in &rows {
            if let (Some(og), Some(ng)) = (r.old, r.new) {
                new_of_old[og as usize] = Some(ng);
            }
        }
        for v in new_of_old {
            if let Some(ng) = v {
                last = ng;
            }
            prev_new.push(last);
        }
    }
    rows.sort_by_key(|r| match (r.new, r.old) {
        (Some(ng), _) => (u64::from(ng) << 1, 0),
        (None, Some(og)) => (
            (u64::from(prev_new.get(og as usize).copied().unwrap_or(0)) << 1) | 1,
            og,
        ),
        (None, None) => (u64::MAX, 0),
    });
    rows
}

fn coverage_rows(o: Option<&FaceReport>, n: Option<&FaceReport>) -> Vec<BlockRow> {
    let empty = BTreeMap::new();
    let oc = o.map(|r| &r.cmap).unwrap_or(&empty);
    let nc = n.map(|r| &r.cmap).unwrap_or(&empty);
    let mut rows: BTreeMap<u32, BlockRow> = BTreeMap::new();
    let all: BTreeSet<u32> = oc.keys().chain(nc.keys()).copied().collect();
    for cp in all {
        let (start, end, name) = blocks::block_of(cp).unwrap_or((cp, cp, "Unassigned"));
        let row = rows.entry(start).or_insert_with(|| BlockRow {
            name,
            start,
            end,
            old_count: 0,
            new_count: 0,
            added: Vec::new(),
            removed: Vec::new(),
            remapped: Vec::new(),
        });
        match (oc.get(&cp), nc.get(&cp)) {
            (Some(og), Some(ng)) => {
                row.old_count += 1;
                row.new_count += 1;
                let name_of = |r: Option<&FaceReport>, g: u32| r.map(|r| r.glyph_name(g));
                if name_of(o, *og) != name_of(n, *ng) {
                    row.remapped.push(cp);
                }
            }
            (Some(_), None) => {
                row.old_count += 1;
                row.removed.push(cp);
            }
            (None, Some(_)) => {
                row.new_count += 1;
                row.added.push(cp);
            }
            (None, None) => {}
        }
    }
    // a new or deleted font lists what it covers, not every code point as
    // added or removed
    if o.is_none() || n.is_none() {
        for row in rows.values_mut() {
            row.added.clear();
            row.removed.clear();
        }
    }
    rows.into_values().collect()
}

fn named_rules(r: &FaceReport, f: &Feature) -> HashSet<NamedRule> {
    let names = |gids: &[u32]| gids.iter().map(|&g| r.glyph_name(g)).collect();
    f.rules
        .iter()
        .map(|rule| NamedRule {
            kind: rule.kind,
            input: names(&rule.input),
            output: names(&rule.output),
        })
        .collect()
}

fn feature_rows(
    o: Option<&FaceReport>,
    n: Option<&FaceReport>,
    table: LayoutTable,
) -> Vec<FeatureRow> {
    fn layout(r: Option<&FaceReport>, table: LayoutTable) -> Option<&LayoutReport> {
        let r = r?;
        match table {
            LayoutTable::Gsub => r.gsub.as_ref(),
            LayoutTable::Gpos => r.gpos.as_ref(),
        }
    }
    let (ol, nl) = (layout(o, table), layout(n, table));
    let tags: BTreeSet<&String> = ol
        .iter()
        .flat_map(|l| l.features.keys())
        .chain(nl.iter().flat_map(|l| l.features.keys()))
        .collect();
    let mut rows = Vec::new();
    for tag in tags {
        let of = ol.and_then(|l| l.features.get(tag));
        let nf = nl.and_then(|l| l.features.get(tag));
        let (orules, nrules) = (
            of.zip(o)
                .map(|(f, r)| named_rules(r, f))
                .unwrap_or_default(),
            nf.zip(n)
                .map(|(f, r)| named_rules(r, f))
                .unwrap_or_default(),
        );
        let mut added_rules: Vec<NamedRule> = nrules.difference(&orules).cloned().collect();
        let mut removed_rules: Vec<NamedRule> = orules.difference(&nrules).cloned().collect();
        added_rules.sort();
        removed_rules.sort();
        let unchanged_rules = orules.intersection(&nrules).count();
        let lookups_changed = match (of, nf) {
            (Some(a), Some(b)) => Some(a.data_hash != b.data_hash),
            _ => Some(false),
        };
        // a kern feature's own changes are its pairs (counted elsewhere);
        // GSUB rules speak for themselves
        let listed_change = !added_rules.is_empty() || !removed_rules.is_empty();
        let kern_change = table == LayoutTable::Gpos
            && tag == "kern"
            && match (o, n) {
                (Some(o), Some(n)) => kerning_differs(o, n),
                _ => false,
            };
        let status = match (of, nf) {
            (None, Some(_)) => Status::Added,
            (Some(_), None) => Status::Removed,
            (Some(a), Some(b)) => {
                let structural = a.lookup_types != b.lookup_types || a.contextual != b.contextual;
                let unlisted = match table {
                    // GSUB lists its context-free rules; contextual ones
                    // only show through the bytes
                    LayoutTable::Gsub => a.contextual > 0 && lookups_changed == Some(true),
                    LayoutTable::Gpos => tag != "kern" && lookups_changed == Some(true),
                };
                if listed_change || kern_change || structural || unlisted {
                    Status::Modified
                } else {
                    Status::Unchanged
                }
            }
            (None, None) => continue,
        };
        rows.push(FeatureRow {
            table,
            tag: tag.clone(),
            ui_name: nf
                .and_then(|f| f.ui_name.clone())
                .or_else(|| of.and_then(|f| f.ui_name.clone())),
            status,
            old: of.cloned(),
            new: nf.cloned(),
            added_rules,
            removed_rules,
            unchanged_rules,
            lookups_changed,
        });
    }
    rows
}

fn kerning_by_name(r: &FaceReport) -> HashMap<(String, String), i32> {
    r.kerning
        .pairs
        .iter()
        .map(|(&(a, b), &v)| ((r.glyph_name(a), r.glyph_name(b)), v))
        .collect()
}

fn kerning_differs(o: &FaceReport, n: &FaceReport) -> bool {
    kerning_by_name(o) != kerning_by_name(n)
}

fn kern_rows(o: Option<&FaceReport>, n: Option<&FaceReport>) -> (Vec<KernRow>, (usize, usize)) {
    let ok = o.map(kerning_by_name).unwrap_or_default();
    let nk = n.map(kerning_by_name).unwrap_or_default();
    let counts = (ok.len(), nk.len());
    if o.is_none() || n.is_none() {
        return (Vec::new(), counts);
    }
    let mut rows: Vec<KernRow> = Vec::new();
    for (pair, &v) in &ok {
        match nk.get(pair) {
            Some(&w) if w == v => {}
            other => rows.push(KernRow {
                left: pair.0.clone(),
                right: pair.1.clone(),
                old: Some(v),
                new: other.copied(),
            }),
        }
    }
    for (pair, &w) in &nk {
        if !ok.contains_key(pair) {
            rows.push(KernRow {
                left: pair.0.clone(),
                right: pair.1.clone(),
                old: None,
                new: Some(w),
            });
        }
    }
    rows.sort_by(|a, b| {
        b.delta()
            .abs()
            .cmp(&a.delta().abs())
            .then_with(|| (&a.left, &a.right).cmp(&(&b.left, &b.right)))
    });
    (rows, counts)
}

fn script_rows(o: Option<&FaceReport>, n: Option<&FaceReport>) -> Vec<ScriptRow> {
    let mut rows: BTreeMap<(LayoutTable, String, String), ScriptRow> = BTreeMap::new();
    for (side, r) in [(false, o), (true, n)] {
        let Some(r) = r else { continue };
        for (table, layout) in [(LayoutTable::Gsub, &r.gsub), (LayoutTable::Gpos, &r.gpos)] {
            for script in layout.iter().flat_map(|l| &l.scripts) {
                for (lang, features) in &script.languages {
                    let row = rows
                        .entry((table, script.tag.clone(), lang.clone()))
                        .or_insert_with(|| ScriptRow {
                            table,
                            script: script.tag.clone(),
                            language: lang.clone(),
                            old: None,
                            new: None,
                        });
                    if side {
                        row.new = Some(features.clone());
                    } else {
                        row.old = Some(features.clone());
                    }
                }
            }
        }
    }
    rows.into_values().collect()
}

fn axis_rows(o: Option<&FaceReport>, n: Option<&FaceReport>) -> Vec<ItemRow> {
    let fields = |a: &crate::report::AxisInfo| {
        vec![
            ("Name".to_string(), a.name.clone()),
            ("Minimum".to_string(), a.min.to_string()),
            ("Default".to_string(), a.default.to_string()),
            ("Maximum".to_string(), a.max.to_string()),
            (
                "Hidden".to_string(),
                if a.hidden { "Yes" } else { "No" }.to_string(),
            ),
        ]
    };
    let mut tags: Vec<String> = Vec::new();
    for r in [o, n].into_iter().flatten() {
        for a in &r.axes {
            if !tags.contains(&a.tag) {
                tags.push(a.tag.clone());
            }
        }
    }
    tags.into_iter()
        .map(|tag| {
            let find = |r: Option<&FaceReport>| {
                r.and_then(|r| r.axes.iter().find(|a| a.tag == tag))
                    .map(fields)
            };
            let (of, nf) = (find(o), find(n));
            ItemRow {
                status: Status::of(of.as_ref(), nf.as_ref()),
                fields: merge_fields(of.as_deref(), nf.as_deref()),
                name: tag,
            }
        })
        .collect()
}

fn instance_rows(o: Option<&FaceReport>, n: Option<&FaceReport>) -> Vec<ItemRow> {
    let axes = n.or(o).map(|r| r.axes.clone()).unwrap_or_default();
    let fields = |i: &crate::report::InstanceInfo| {
        let mut f = vec![("Name".to_string(), i.name.clone())];
        if let Some(ps) = &i.ps_name {
            f.push(("PostScript name".to_string(), ps.clone()));
        }
        for (k, v) in i.coords.iter().enumerate() {
            let tag = axes
                .get(k)
                .map(|a| a.tag.clone())
                .unwrap_or_else(|| format!("axis {k}"));
            f.push((tag, v.to_string()));
        }
        f
    };
    let old: Vec<_> = o.map(|r| r.instances.clone()).unwrap_or_default();
    let new: Vec<_> = n.map(|r| r.instances.clone()).unwrap_or_default();
    let mut used = vec![false; old.len()];
    let mut rows = Vec::new();
    for ni in &new {
        // same name first, then same location (a rename)
        let found = old
            .iter()
            .enumerate()
            .position(|(k, oi)| !used[k] && oi.name == ni.name)
            .or_else(|| {
                old.iter()
                    .enumerate()
                    .position(|(k, oi)| !used[k] && oi.coords == ni.coords)
            });
        let of = found.map(|k| {
            used[k] = true;
            fields(&old[k])
        });
        let nf = fields(ni);
        rows.push(ItemRow {
            name: ni.name.clone(),
            status: Status::of(of.as_ref(), Some(&nf)),
            fields: merge_fields(of.as_deref(), Some(&nf)),
        });
    }
    for (k, oi) in old.iter().enumerate() {
        if !used[k] {
            let of = fields(oi);
            rows.push(ItemRow {
                name: oi.name.clone(),
                status: Status::Removed,
                fields: merge_fields(Some(&of), None),
            });
        }
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kv(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs
            .iter()
            .map(|(a, b)| (a.to_string(), b.to_string()))
            .collect()
    }

    #[test]
    fn merges_fields_by_label_in_order() {
        let old = kv(&[("a", "1"), ("b", "2"), ("b", "3")]);
        let new = kv(&[("b", "2"), ("c", "4"), ("a", "9")]);
        let f = merge_fields(Some(&old), Some(&new));
        let got: Vec<_> = f.iter().map(|f| (f.label.as_str(), f.status())).collect();
        assert_eq!(
            got,
            [
                ("a", Status::Modified),
                ("b", Status::Unchanged),
                ("b", Status::Removed),
                ("c", Status::Added),
            ]
        );
    }

    fn summary(index: u32, title: &str, ps: &str, hash: u64) -> FaceSummary {
        FaceSummary {
            index,
            title: title.to_string(),
            ps_name: Some(ps.to_string()),
            hash,
        }
    }

    #[test]
    fn pairs_faces_by_postscript_name() {
        let old = vec![
            summary(0, "Sans", "Sans-Regular", 1),
            summary(1, "Sans Bold", "Sans-Bold", 2),
        ];
        let new = vec![
            summary(0, "Sans Bold", "Sans-Bold", 2),
            summary(1, "Sans", "Sans-Regular", 3),
            summary(2, "Mono", "Mono-Regular", 4),
        ];
        let pairs = pair_faces(old, new);
        let got: Vec<_> = pairs.iter().map(|p| (p.title.as_str(), p.status)).collect();
        assert_eq!(
            got,
            [
                ("Sans Bold", Status::Unchanged),
                ("Sans", Status::Modified),
                ("Mono", Status::Added),
            ]
        );
    }

    #[test]
    fn a_single_renamed_face_stays_paired() {
        let pairs = pair_faces(
            vec![summary(0, "Old", "Old", 1)],
            vec![summary(0, "New", "New", 2)],
        );
        assert_eq!(pairs.len(), 1);
        assert_eq!(pairs[0].status, Status::Modified);
    }
}
