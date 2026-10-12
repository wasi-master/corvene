use std::collections::HashSet;

use corvene_models::{NbCell, NbCellKind, NbData, NbMime, NbOutput, NbOutputKind, NotebookBlob};

use super::outputs::{OutputStatus, Render, html_table, latex_body, output_pairs, renderings};
use super::text::{LineKind, line_diff, segments, split_rows, word_changes};
use super::*;

fn cell(kind: NbCellKind, id: Option<&str>, source: &str) -> NbCell {
    NbCell {
        id: id.map(str::to_string),
        kind,
        source: source.to_string(),
        execution_count: None,
        outputs: Vec::new(),
        metadata: Vec::new(),
        attachments: Vec::new(),
        lines: (0, 0),
    }
}

fn code(source: &str) -> NbCell {
    cell(NbCellKind::Code, None, source)
}

fn md(source: &str) -> NbCell {
    cell(NbCellKind::Markdown, None, source)
}

fn nb(cells: Vec<NbCell>) -> NotebookBlob {
    NotebookBlob {
        format: (4, 5),
        language: "python".into(),
        file_extension: ".py".into(),
        cells,
        ..NotebookBlob::default()
    }
}

fn stream(text: &str) -> NbOutput {
    NbOutput {
        kind: NbOutputKind::Stream,
        name: "stdout".into(),
        text: text.into(),
        evalue: String::new(),
        execution_count: None,
        data: Vec::new(),
        metadata: Vec::new(),
    }
}

fn result(count: u64, mime: &str, data: NbData) -> NbOutput {
    NbOutput {
        kind: NbOutputKind::ExecuteResult,
        name: String::new(),
        text: String::new(),
        evalue: String::new(),
        execution_count: Some(count),
        data: vec![NbMime {
            mime: mime.into(),
            data,
        }],
        metadata: Vec::new(),
    }
}

fn statuses(d: &NotebookDiff) -> Vec<CellStatus> {
    d.pairs.iter().map(|p| p.status).collect()
}

#[test]
fn unchanged_notebook_is_all_same() {
    let a = nb(vec![md("# Title"), code("x = 1"), code("print(x)")]);
    let d = diff(Some(&a), Some(&a));
    assert!(statuses(&d).iter().all(|s| *s == CellStatus::Same));
    assert_eq!(
        d.change_indices(&NotebookFilters::default()),
        Vec::<usize>::new()
    );
}

#[test]
fn edited_inserted_and_deleted_cells() {
    let a = nb(vec![
        md("# Title"),
        code("import numpy as np\nx = np.arange(10)"),
        code("print('bye')"),
    ]);
    let b = nb(vec![
        md("# Title"),
        code("import numpy as np\nx = np.arange(20)"),
        md("Some new text here"),
    ]);
    let d = diff(Some(&a), Some(&b));
    assert_eq!(
        statuses(&d),
        vec![
            CellStatus::Same,
            CellStatus::Modified,
            CellStatus::Deleted,
            CellStatus::Added
        ]
    );
    assert!(d.pairs[1].changes.source);
}

#[test]
fn ids_pair_cells_even_when_rewritten() {
    let a = nb(vec![
        cell(NbCellKind::Code, Some("a1"), "x = 1"),
        cell(NbCellKind::Code, Some("b2"), "completely different"),
    ]);
    let b = nb(vec![
        cell(NbCellKind::Code, Some("a1"), "x = 1"),
        cell(NbCellKind::Code, Some("b2"), "nothing alike at all !!"),
    ]);
    let d = diff(Some(&a), Some(&b));
    assert_eq!(statuses(&d), vec![CellStatus::Same, CellStatus::Modified]);
}

#[test]
fn moved_cell_is_a_move() {
    let a = nb(vec![
        code("def load():\n    return open('data.csv').read()"),
        code("x = 1"),
        code("y = 2"),
        code("z = 3"),
    ]);
    let b = nb(vec![
        code("x = 1"),
        code("y = 2"),
        code("z = 3"),
        code("def load():\n    return open('data.csv').read()"),
    ]);
    let d = diff(Some(&a), Some(&b));
    let moved: Vec<_> = d
        .pairs
        .iter()
        .filter(|p| matches!(p.status, CellStatus::Moved { .. }))
        .collect();
    assert_eq!(moved.len(), 1);
    assert_eq!(moved[0].status, CellStatus::Moved { from: 0 });
    assert!(
        d.pairs
            .iter()
            .any(|p| p.status == CellStatus::MovedAway { to: 3 })
    );
    assert_eq!(d.summary(&NotebookFilters::default()).moved, 1);
}

#[test]
fn execution_counts_and_metadata_are_hidden_by_default() {
    let mut a = code("x = 1");
    a.execution_count = Some(1);
    a.outputs = vec![result(1, "text/plain", NbData::Text("1".into()))];
    let mut b = a.clone();
    b.execution_count = Some(7);
    b.outputs = vec![result(7, "text/plain", NbData::Text("1".into()))];
    b.metadata = vec![("ExecuteTime".into(), "{}".into())];
    let d = diff(Some(&nb(vec![a])), Some(&nb(vec![b])));
    let p = d.pairs[0];
    assert_eq!(p.status, CellStatus::Modified);
    assert!(p.changes.execution_count && p.changes.metadata && !p.changes.outputs);
    let filters = NotebookFilters::default();
    assert!(!p.is_change(&filters));
    assert_eq!(
        p.changes.hidden(&filters),
        vec![HiddenChange::ExecutionCount, HiddenChange::Metadata]
    );
    let all = NotebookFilters {
        hide_execution_counts: false,
        hide_metadata: false,
        ..filters
    };
    assert!(p.is_change(&all));
    assert_eq!(d.summary(&filters).hidden, 1);
}

#[test]
fn unchanged_runs_fold_with_context() {
    let cells: Vec<NbCell> = (0..10).map(|i| code(&format!("x{i} = {i}"))).collect();
    let mut changed = cells.clone();
    changed[5].source = "x5 = 500".into();
    let d = diff(Some(&nb(cells)), Some(&nb(changed)));
    let filters = NotebookFilters::default();
    let rows = rows(&d, &filters, &HashSet::new());
    assert_eq!(
        rows,
        vec![
            Row::Folded { start: 0, end: 4 },
            Row::Cell(4),
            Row::Cell(5),
            Row::Cell(6),
            Row::Folded { start: 7, end: 10 },
        ]
    );
    let opened = rows_expanded(&d, &filters, 0);
    assert_eq!(opened[0], Row::Cell(0));
}

fn rows_expanded(d: &NotebookDiff, f: &NotebookFilters, start: usize) -> Vec<Row> {
    rows(d, f, &HashSet::from([start]))
}

#[test]
fn type_change_keeps_the_cell() {
    let a = nb(vec![code("Some words here")]);
    let b = nb(vec![md("Some words here")]);
    let d = diff(Some(&a), Some(&b));
    assert_eq!(statuses(&d), vec![CellStatus::Modified]);
    assert!(d.pairs[0].changes.kind);
}

#[test]
fn new_and_deleted_notebooks() {
    let a = nb(vec![code("x"), md("y")]);
    let d = diff(None, Some(&a));
    assert_eq!(statuses(&d), vec![CellStatus::Added, CellStatus::Added]);
    let d = diff(Some(&a), None);
    assert_eq!(statuses(&d), vec![CellStatus::Deleted, CellStatus::Deleted]);
}

#[test]
fn line_diff_marks_words() {
    let lines = line_diff(
        "a = 1\nb = foo(x)\nc = 3",
        "a = 1\nb = bar(x)\nc = 3\nd = 4",
    );
    let kinds: Vec<LineKind> = lines.iter().map(|l| l.kind).collect();
    assert_eq!(
        kinds,
        vec![
            LineKind::Context,
            LineKind::Deleted,
            LineKind::Added,
            LineKind::Context,
            LineKind::Added
        ]
    );
    assert_eq!(&lines[1].text[lines[1].marks[0].clone()], "foo");
    assert_eq!(&lines[2].text[lines[2].marks[0].clone()], "bar");
    let split = split_rows(&lines);
    assert_eq!(split[1], (Some(1), Some(2)));
    assert_eq!(split[3], (None, Some(4)));
    let (a, b) = word_changes("let x = 1;", "let y = 1;");
    assert_eq!((a.len(), b.len()), (1, 1));
}

#[test]
fn long_context_folds() {
    let old: String = (0..30).map(|i| format!("line {i}\n")).collect();
    let new = old.replace("line 15\n", "line fifteen\n");
    let lines = line_diff(&old, &new);
    let segs = segments(&lines, 2);
    assert_eq!(segs.len(), 3);
    assert!(matches!(segs[0], text::Segment::Folded { .. }));
}

#[test]
fn outputs_pair_by_shape() {
    let old = vec![
        stream("loss 0.5\n"),
        result(3, "text/plain", NbData::Text("42".into())),
    ];
    let new = vec![
        stream("loss 0.4\n"),
        result(4, "text/plain", NbData::Text("43".into())),
    ];
    let pairs = output_pairs(&old, &new);
    assert_eq!(pairs.len(), 2);
    assert!(pairs.iter().all(|p| p.status == OutputStatus::Modified));
}

#[test]
fn renderings_prefer_rich_types() {
    let out = NbOutput {
        data: vec![
            NbMime {
                mime: "text/plain".into(),
                data: NbData::Text("<Figure>".into()),
            },
            NbMime {
                mime: "image/png".into(),
                data: NbData::Base64("iVBORw0KGgo=".into()),
            },
        ],
        ..result(1, "text/plain", NbData::Text(String::new()))
    };
    let r = renderings(&out);
    assert!(matches!(r[0].render, Render::Image { .. }));
    assert_eq!(r.len(), 2);
    assert_eq!(latex_body("$\\displaystyle x^{2}$"), "\\displaystyle x^{2}");
    assert_eq!(latex_body("$$a$$"), "a");
}

#[test]
fn dataframe_html_reads_as_rows() {
    let html = r#"<div><style scoped>.x{}</style><table border="1" class="dataframe">
  <thead><tr style="text-align: right;"><th></th><th>a</th><th>b</th></tr></thead>
  <tbody>
    <tr><th>0</th><td>1</td><td>x &amp; y</td></tr>
    <tr><th rowspan="2">1</th><td>2</td><td>z</td></tr>
    <tr><td>3</td><td>w</td></tr>
  </tbody></table></div>"#;
    let rows = html_table(html).unwrap();
    assert_eq!(rows[0], vec!["", "a", "b"]);
    assert_eq!(rows[1], vec!["0", "1", "x & y"]);
    assert_eq!(rows[3], vec!["1", "3", "w"]);
    let out = result(1, "text/html", NbData::Text(html.into()));
    assert!(matches!(renderings(&out)[0].render, Render::Table(_)));
}

#[test]
fn script_view_reads_like_code() {
    let mut c = code("x = 1\nprint(x)");
    c.execution_count = Some(2);
    c.outputs = vec![stream("1\n")];
    let s = script::script(&nb(vec![md("# Intro\nText"), c]));
    assert_eq!(
        s,
        "# %% [markdown]\n# # Intro\n# Text\n\n# %% In [2]\nx = 1\nprint(x)\n# stdout:\n#   1\n"
    );
    assert_eq!(script::strip_ansi("\x1b[31mred\x1b[0m"), "red");
}
