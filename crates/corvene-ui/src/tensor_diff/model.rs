//! The pure parts of the tensor diff: tensor names grouped into a tree by
//! their dotted or slashed prefixes, the rows a list shows for a set of
//! options, and number formatting.

use std::collections::{HashMap, HashSet};

use corvene_models::tensors::{MetaRow, TensorChange, TensorInfo, TensorRow};

/// How many tensors each change kind covers.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Counts {
    pub added: usize,
    pub removed: usize,
    pub changed: usize,
    pub renamed: usize,
    pub unchanged: usize,
}

impl Counts {
    pub fn add(&mut self, change: TensorChange) {
        match change {
            TensorChange::Added => self.added += 1,
            TensorChange::Removed => self.removed += 1,
            TensorChange::Changed => self.changed += 1,
            TensorChange::Renamed => self.renamed += 1,
            TensorChange::Unchanged => self.unchanged += 1,
        }
    }

    fn merge(&mut self, other: Counts) {
        self.added += other.added;
        self.removed += other.removed;
        self.changed += other.changed;
        self.renamed += other.renamed;
        self.unchanged += other.unchanged;
    }

    /// Tensors whose layout changed.
    pub fn layout_changes(&self) -> usize {
        self.added + self.removed + self.changed + self.renamed
    }

    pub fn total(&self) -> usize {
        self.layout_changes() + self.unchanged
    }

    pub fn of(rows: &[TensorRow]) -> Counts {
        let mut c = Counts::default();
        for r in rows {
            c.add(r.change);
        }
        c
    }
}

/// A group of tensors sharing a name prefix.
#[derive(Debug, Default)]
pub struct Node {
    /// What the group adds to its parent's prefix (`layers.`).
    pub label: String,
    /// The whole prefix (`model.layers.`); "" for the root.
    pub prefix: String,
    pub children: Vec<Child>,
    /// Every tensor below.
    pub counts: Counts,
    /// Elements of every tensor below (new side, old for removed ones).
    pub parameters: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Child {
    Group(usize),
    Leaf(usize),
}

/// The tensors of a diff grouped by name prefix; `nodes[0]` is the root.
#[derive(Debug, Default)]
pub struct Tree {
    pub nodes: Vec<Node>,
}

/// The tensor of a row on the side that has it (new first).
pub fn info(row: &TensorRow) -> Option<&TensorInfo> {
    row.new.as_ref().or(row.old.as_ref())
}

/// `model.layers.0.weight` as `model.`, `layers.`, `0.`, `weight`.
fn segments(name: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut start = 0;
    for (i, c) in name.char_indices() {
        if c == '.' || c == '/' {
            out.push(&name[start..=i]);
            start = i + 1;
        }
    }
    if start < name.len() {
        out.push(&name[start..]);
    }
    out
}

impl Tree {
    /// Groups `rows` by name prefix, in first-seen order. A group holding a
    /// single tensor becomes that tensor, and a group holding only one
    /// group merges with it (`model.layers.` rather than `model.` over
    /// `layers.`).
    pub fn build(rows: &[TensorRow]) -> Tree {
        let mut nodes = vec![Node::default()];
        let mut lookup: Vec<HashMap<String, usize>> = vec![HashMap::new()];
        for (ix, row) in rows.iter().enumerate() {
            let segs = segments(row.name());
            let mut at = 0usize;
            let mut prefix = String::new();
            for seg in segs.iter().take(segs.len().saturating_sub(1)) {
                prefix.push_str(seg);
                let next = match lookup[at].get(*seg) {
                    Some(n) => *n,
                    None => {
                        let n = nodes.len();
                        nodes.push(Node {
                            label: seg.to_string(),
                            prefix: prefix.clone(),
                            ..Node::default()
                        });
                        lookup.push(HashMap::new());
                        lookup[at].insert(seg.to_string(), n);
                        nodes[at].children.push(Child::Group(n));
                        n
                    }
                };
                at = next;
            }
            nodes[at].children.push(Child::Leaf(ix));
        }
        let mut tree = Tree { nodes };
        tree.total(0, rows);
        tree.compress(0);
        tree
    }

    fn total(&mut self, node: usize, rows: &[TensorRow]) -> (Counts, u64) {
        let mut counts = Counts::default();
        let mut parameters = 0;
        for child in self.nodes[node].children.clone() {
            match child {
                Child::Leaf(ix) => {
                    counts.add(rows[ix].change);
                    parameters += info(&rows[ix]).map_or(0, |t| t.elements);
                }
                Child::Group(g) => {
                    let (c, p) = self.total(g, rows);
                    counts.merge(c);
                    parameters += p;
                }
            }
        }
        self.nodes[node].counts = counts;
        self.nodes[node].parameters = parameters;
        (counts, parameters)
    }

    /// The only tensor below `node`, when it holds one.
    fn single_leaf(&self, node: usize) -> Option<usize> {
        if self.nodes[node].counts.total() != 1 {
            return None;
        }
        let mut at = node;
        loop {
            match self.nodes[at].children.first()? {
                Child::Leaf(ix) => return Some(*ix),
                Child::Group(g) => at = *g,
            }
        }
    }

    fn compress(&mut self, node: usize) {
        let children = std::mem::take(&mut self.nodes[node].children);
        let mut out = Vec::with_capacity(children.len());
        for child in children {
            match child {
                Child::Leaf(_) => out.push(child),
                Child::Group(g) => {
                    if let Some(leaf) = self.single_leaf(g) {
                        out.push(Child::Leaf(leaf));
                        continue;
                    }
                    // fold chains of lone groups into one
                    while let [Child::Group(only)] = self.nodes[g].children.as_slice() {
                        let only = *only;
                        let label = std::mem::take(&mut self.nodes[only].label);
                        let prefix = std::mem::take(&mut self.nodes[only].prefix);
                        let grand = std::mem::take(&mut self.nodes[only].children);
                        self.nodes[g].label.push_str(&label);
                        self.nodes[g].prefix = prefix;
                        self.nodes[g].children = grand;
                    }
                    self.compress(g);
                    out.push(Child::Group(g));
                }
            }
        }
        self.nodes[node].children = out;
    }

    /// Whether the tree has any group (otherwise the list is flat).
    pub fn grouped(&self) -> bool {
        self.nodes[0]
            .children
            .iter()
            .any(|c| matches!(c, Child::Group(_)))
    }
}

/// One line of the tensor list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Item {
    /// A group header; `value_changes` counts tensors below whose values
    /// were found to differ.
    Group {
        node: usize,
        depth: usize,
        collapsed: bool,
        shown: usize,
        value_changes: usize,
    },
    /// A tensor; its label is the name from byte `label_from` on.
    Tensor {
        row: usize,
        depth: usize,
        label_from: usize,
        expanded: bool,
    },
    /// The details under an expanded tensor.
    Detail { row: usize },
}

/// What decides which rows are listed.
pub struct Shown<'a> {
    pub show_unchanged: bool,
    pub collapsed: &'a HashSet<String>,
    pub expanded: &'a HashSet<usize>,
    /// Lowercase; empty for none.
    pub filter: &'a str,
    /// Whether a row's values were compared and differ.
    pub value_changed: &'a dyn Fn(usize) -> bool,
}

impl Shown<'_> {
    fn listed(&self, row: &TensorRow, ix: usize) -> bool {
        self.show_unchanged || row.change != TensorChange::Unchanged || (self.value_changed)(ix)
    }

    fn matches(&self, row: &TensorRow) -> bool {
        self.filter.is_empty()
            || [&row.old, &row.new]
                .into_iter()
                .flatten()
                .any(|t| t.name.to_lowercase().contains(self.filter))
    }
}

/// The rows to list: the tree with folded groups closed and groups with
/// nothing listed left out, or a flat list of matches while filtering.
/// Lists a tensor, and its details when opened.
fn push_tensor(shown: &Shown, out: &mut Vec<Item>, row: usize, depth: usize, label_from: usize) {
    let expanded = shown.expanded.contains(&row);
    out.push(Item::Tensor {
        row,
        depth,
        label_from,
        expanded,
    });
    if expanded {
        out.push(Item::Detail { row });
    }
}

pub fn flatten(tree: &Tree, rows: &[TensorRow], shown: &Shown) -> Vec<Item> {
    let mut out = Vec::new();
    if !shown.filter.is_empty() || !tree.grouped() {
        for (ix, row) in rows.iter().enumerate() {
            if shown.listed(row, ix) && shown.matches(row) {
                push_tensor(shown, &mut out, ix, 0, 0);
            }
        }
        return out;
    }

    /// Lists `node`'s children at `depth`; returns (tensors listed, value
    /// changes) below it.
    fn walk(
        tree: &Tree,
        rows: &[TensorRow],
        shown: &Shown,
        node: usize,
        depth: usize,
        emit: bool,
        out: &mut Vec<Item>,
    ) -> (usize, usize) {
        let mut listed = 0;
        let mut value_changes = 0;
        let from = tree.nodes[node].prefix.len();
        for child in &tree.nodes[node].children {
            match *child {
                Child::Leaf(ix) => {
                    if shown.listed(&rows[ix], ix) {
                        listed += 1;
                        if (shown.value_changed)(ix) {
                            value_changes += 1;
                        }
                        if emit {
                            push_tensor(shown, out, ix, depth, from);
                        }
                    }
                }
                Child::Group(g) => {
                    let collapsed = shown.collapsed.contains(&tree.nodes[g].prefix);
                    let at = out.len();
                    if emit {
                        out.push(Item::Group {
                            node: g,
                            depth,
                            collapsed,
                            shown: 0,
                            value_changes: 0,
                        });
                    }
                    let (n, v) = walk(tree, rows, shown, g, depth + 1, emit && !collapsed, out);
                    if emit {
                        if n == 0 {
                            out.truncate(at);
                        } else if let Some(Item::Group {
                            shown,
                            value_changes,
                            ..
                        }) = out.get_mut(at)
                        {
                            *shown = n;
                            *value_changes = v;
                        }
                    }
                    listed += n;
                    value_changes += v;
                }
            }
        }
        (listed, value_changes)
    }
    walk(tree, rows, shown, 0, 0, true, &mut out);
    out
}

/// One line of the metadata list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MetaItem {
    /// A section heading with how many of its entries changed.
    Section {
        title: String,
        changed: usize,
    },
    Entry(usize),
}

/// The metadata rows to list, under their section headings.
pub fn flatten_metadata(rows: &[MetaRow], show_unchanged: bool, filter: &str) -> Vec<MetaItem> {
    let mut out = Vec::new();
    let mut section: Option<&str> = None;
    let mut heading_at = 0usize;
    for (ix, row) in rows.iter().enumerate() {
        if section != Some(row.section.as_str()) {
            if let Some(MetaItem::Section { .. }) = out.last() {
                out.pop();
            }
            section = Some(row.section.as_str());
            heading_at = out.len();
            out.push(MetaItem::Section {
                title: row.section.clone(),
                changed: 0,
            });
        }
        let changed = !row.unchanged();
        let matches = filter.is_empty()
            || row.key.to_lowercase().contains(filter)
            || [&row.old, &row.new]
                .into_iter()
                .flatten()
                .any(|v| v.to_lowercase().contains(filter));
        if (changed || show_unchanged) && matches {
            if changed && let Some(MetaItem::Section { changed: n, .. }) = out.get_mut(heading_at) {
                *n += 1;
            }
            out.push(MetaItem::Entry(ix));
        }
    }
    if let Some(MetaItem::Section { .. }) = out.last() {
        out.pop();
    }
    out
}

/// Each dimension of a shape as text (symbolic names where present).
pub fn dims(t: &TensorInfo) -> Vec<String> {
    t.shape
        .iter()
        .enumerate()
        .map(|(i, d)| match t.dim_names.get(i) {
            Some(n) if !n.is_empty() => n.clone(),
            _ => d.to_string(),
        })
        .collect()
}

/// A count in short form: `512`, `1.5 K`, `124 M`, `7.24 B`.
pub fn human_count(n: u64) -> String {
    const UNITS: [(f64, &str); 4] = [(1e12, "T"), (1e9, "B"), (1e6, "M"), (1e3, "K")];
    let v = n as f64;
    for (scale, unit) in UNITS {
        if v >= scale {
            let x = v / scale;
            let text = if x < 10. {
                format!("{x:.2}")
            } else if x < 100. {
                format!("{x:.1}")
            } else {
                format!("{x:.0}")
            };
            return format!("{} {unit}", trim_zeros(&text));
        }
    }
    n.to_string()
}

fn trim_zeros(text: &str) -> String {
    if text.contains('.') && !text.contains('e') {
        text.trim_end_matches('0').trim_end_matches('.').to_string()
    } else {
        text.to_string()
    }
}

/// A value for a stats table: four significant digits, exponent form for
/// very large or small magnitudes.
pub fn value(v: f64) -> String {
    if v.is_nan() {
        return "NaN".into();
    }
    if v.is_infinite() {
        return if v > 0. {
            "∞".into()
        } else {
            "−∞".into()
        };
    }
    if v == 0. {
        return "0".into();
    }
    let a = v.abs();
    let text = if !(1e-3..1e5).contains(&a) {
        let e = format!("{v:.2e}");
        // `1.50e-4` → `1.5e-4`
        match e.split_once('e') {
            Some((m, x)) => format!("{}e{x}", trim_zeros(m)),
            None => e,
        }
    } else {
        let digits = (3 - a.log10().floor() as i32).clamp(0, 6) as usize;
        trim_zeros(&format!("{v:.digits$}"))
    };
    text.replace('-', "−")
}

/// A share of elements: `0%`, `<0.01%`, `0.42%`, `12.5%`, `100%`.
pub fn percent(part: u64, whole: u64) -> String {
    if whole == 0 || part == 0 {
        return "0%".into();
    }
    if part == whole {
        return "100%".into();
    }
    let p = part as f64 / whole as f64 * 100.;
    if p < 0.01 {
        "<0.01%".into()
    } else if p < 1. {
        format!("{}%", trim_zeros(&format!("{p:.2}")))
    } else {
        format!("{}%", trim_zeros(&format!("{:.1}", p.min(99.9))))
    }
}

/// A cosine similarity, enough digits to tell 0.99999 from 1.
pub fn cosine(c: f64) -> String {
    if c >= 1. {
        "1".into()
    } else {
        format!("{c:.5}").replace('-', "−")
    }
}

/// The coordinates of flat element `index` in `shape`; `fastest_first`
/// when the first dimension varies fastest (GGUF, column-major data).
pub fn coords(shape: &[u64], mut index: u64, fastest_first: bool) -> Option<Vec<u64>> {
    if shape.len() < 2 || shape.contains(&0) {
        return None;
    }
    let mut out = vec![0; shape.len()];
    let order: Vec<usize> = if fastest_first {
        (0..shape.len()).collect()
    } else {
        (0..shape.len()).rev().collect()
    };
    for k in order {
        out[k] = index % shape[k];
        index /= shape[k];
    }
    (index == 0).then_some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use corvene_models::TensorFile;
    use corvene_models::tensors::{DType, align};

    fn file(names: &[&str]) -> TensorFile {
        TensorFile {
            tensors: names
                .iter()
                .map(|n| TensorInfo {
                    name: n.to_string(),
                    dtype: DType::F32,
                    shape: vec![2, 2],
                    elements: 4,
                    bytes: 16,
                    ..TensorInfo::default()
                })
                .collect(),
            ..TensorFile::default()
        }
    }

    fn labels(tree: &Tree, node: usize, rows: &[TensorRow], out: &mut Vec<String>, depth: usize) {
        let from = tree.nodes[node].prefix.len();
        for c in &tree.nodes[node].children {
            match *c {
                Child::Leaf(ix) => {
                    out.push(format!("{}{}", " ".repeat(depth), &rows[ix].name()[from..]))
                }
                Child::Group(g) => {
                    out.push(format!("{}{}", " ".repeat(depth), tree.nodes[g].label));
                    labels(tree, g, rows, out, depth + 1);
                }
            }
        }
    }

    #[test]
    fn groups_by_prefix_and_folds_chains() {
        let f = file(&[
            "model.embed.weight",
            "model.layers.0.attn.q.weight",
            "model.layers.0.attn.k.weight",
            "model.layers.0.norm.weight",
            "model.layers.1.attn.q.weight",
            "model.layers.1.attn.k.weight",
            "lm_head.weight",
        ]);
        let rows = align(None, Some(&f));
        let tree = Tree::build(&rows);
        let mut out = Vec::new();
        labels(&tree, 0, &rows, &mut out, 0);
        assert_eq!(
            out,
            [
                "model.",
                " embed.weight",
                " layers.",
                "  0.",
                "   attn.",
                "    q.weight",
                "    k.weight",
                "   norm.weight",
                "  1.attn.",
                "   q.weight",
                "   k.weight",
                "lm_head.weight",
            ]
        );
        assert_eq!(tree.nodes[0].counts.added, 7);
    }

    #[test]
    fn flatten_hides_unchanged_and_collapsed() {
        let old = file(&["a.x", "a.y", "b.x", "b.y"]);
        let mut new = file(&["a.x", "a.y", "b.x", "b.y", "b.z"]);
        new.tensors[0].shape = vec![4];
        let rows = align(Some(&old), Some(&new));
        let tree = Tree::build(&rows);
        let collapsed = HashSet::new();
        let expanded = HashSet::from([0]);
        let no = |_: usize| false;
        let shown = Shown {
            show_unchanged: false,
            collapsed: &collapsed,
            expanded: &expanded,
            filter: "",
            value_changed: &no,
        };
        let items = flatten(&tree, &rows, &shown);
        assert_eq!(items.len(), 5);
        assert!(matches!(items[0], Item::Group { shown: 1, .. }));
        assert!(matches!(items[2], Item::Detail { row: 0 }));
        let collapsed = HashSet::from(["a.".to_string()]);
        let shown = Shown {
            collapsed: &collapsed,
            ..shown
        };
        assert_eq!(flatten(&tree, &rows, &shown).len(), 3);
        let shown = Shown {
            filter: "z",
            ..shown
        };
        assert_eq!(flatten(&tree, &rows, &shown).len(), 1);
    }

    #[test]
    fn formats_numbers() {
        assert_eq!(human_count(512), "512");
        assert_eq!(human_count(7_241_732_096), "7.24 B");
        assert_eq!(human_count(124_000_000), "124 M");
        assert_eq!(human_count(1_500), "1.5 K");
        assert_eq!(value(0.0001234), "1.23e−4");
        assert_eq!(value(-0.5), "−0.5");
        assert_eq!(value(1234.5678), "1235");
        assert_eq!(value(0.0123456), "0.01235");
        assert_eq!(percent(1, 3), "33.3%");
        assert_eq!(percent(1, 1_000_000), "<0.01%");
        assert_eq!(cosine(0.999987), "0.99999");
        assert_eq!(coords(&[2, 3], 4, false), Some(vec![1, 1]));
        assert_eq!(coords(&[3, 2], 4, true), Some(vec![1, 1]));
    }
}
