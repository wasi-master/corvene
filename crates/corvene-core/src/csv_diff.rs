//! Corvene `1318-csv-table-diff` (desktop/desktop#21518): a cell-level diff
//! of two versions of a CSV or TSV file.
//!
//! The first record is the header. Columns are matched by name when both
//! headers have unique names (by position otherwise), rows by a line diff
//! over the shared columns, then by their first column inside each block of
//! changes, so an edited row shows as one modified row with its changed
//! cells instead of a removal and an addition. GitHub Desktop only has the
//! text diff.

use std::collections::HashSet;

use corvene_models::{TableBlob, TableSheet};

/// Files with more records than this (both sides together) get the text
/// diff.
pub const MAX_ROWS: usize = 20_000;
/// How long each row diff may take (it runs while the diff renders); past
/// it the alignment is coarser, never wrong.
const DIFF_BUDGET: std::time::Duration = std::time::Duration::from_millis(100);

/// One row's (or key's) cells as a hash, so the row diff compares numbers.
fn hash_cells<'a>(cells: impl IntoIterator<Item = &'a String>) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    for c in cells {
        c.hash(&mut h);
    }
    h.finish()
}

fn diff_ops(a: &[u64], b: &[u64]) -> Vec<similar::DiffOp> {
    let deadline = std::time::Instant::now() + DIFF_BUDGET;
    similar::capture_diff_slices_deadline(similar::Algorithm::Myers, a, b, Some(deadline))
}

/// The delimiter of a table file, from its extension.
pub fn table_delimiter(path: &str) -> Option<u8> {
    let ext = path.rsplit_once('.')?.1.to_ascii_lowercase();
    match ext.as_str() {
        "csv" => Some(b','),
        "tsv" | "tab" => Some(b'\t'),
        _ => None,
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TableError {
    #[error("unterminated quoted field starting on line {0}")]
    Unterminated(u32),
    #[error("more than {MAX_ROWS} rows")]
    TooLarge,
    #[error("no header row")]
    Empty,
}

/// One parsed record and the 1-based line it starts on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Record {
    pub line: u32,
    pub cells: Vec<String>,
}

/// RFC 4180 records of `text` (quoted fields may hold the delimiter,
/// line breaks and `""`); blank lines are skipped. A TSV file whose quotes
/// do not pair up is read again without quoting (plain TSV has none).
pub fn parse(text: &str, delimiter: u8) -> Result<Vec<Record>, TableError> {
    match parse_with(text, delimiter, true) {
        Err(TableError::Unterminated(_)) if delimiter == b'\t' => {
            parse_with(text, delimiter, false)
        }
        result => result,
    }
}

fn parse_with(text: &str, delimiter: u8, quoting: bool) -> Result<Vec<Record>, TableError> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let delimiter = delimiter as char;
    let mut out = Vec::new();
    let mut cells: Vec<String> = Vec::new();
    let mut field = String::new();
    let mut line = 1u32;
    let mut record_line = 1u32;
    let mut quoted = false;
    let mut quote_line = 0u32;
    let mut field_started = false;
    let mut chars = text.chars().peekable();
    let end_record = |cells: &mut Vec<String>,
                      field: &mut String,
                      field_started: bool,
                      out: &mut Vec<Record>,
                      record_line: u32| {
        if field_started || !cells.is_empty() {
            cells.push(std::mem::take(field));
            out.push(Record {
                line: record_line,
                cells: std::mem::take(cells),
            });
        }
        field.clear();
    };
    while let Some(c) = chars.next() {
        if quoted {
            match c {
                '"' if chars.peek() == Some(&'"') => {
                    chars.next();
                    field.push('"');
                }
                '"' => quoted = false,
                '\n' => {
                    line += 1;
                    field.push('\n');
                }
                _ => field.push(c),
            }
            continue;
        }
        match c {
            '"' if quoting && field.is_empty() => {
                quoted = true;
                quote_line = line;
                field_started = true;
            }
            '\r' if chars.peek() == Some(&'\n') => {}
            '\n' => {
                end_record(&mut cells, &mut field, field_started, &mut out, record_line);
                field_started = false;
                line += 1;
                record_line = line;
            }
            c if c == delimiter => {
                cells.push(std::mem::take(&mut field));
                field_started = true;
            }
            _ => {
                field.push(c);
                field_started = true;
            }
        }
    }
    if quoted {
        return Err(TableError::Unterminated(quote_line));
    }
    end_record(&mut cells, &mut field, field_started, &mut out, record_line);
    Ok(out)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColumnStatus {
    Same,
    Added,
    Deleted,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TableColumn {
    /// The new header's name (the old one for a deleted column).
    pub name: String,
    /// The old name when it differs (columns matched by position).
    pub old_name: Option<String>,
    pub status: ColumnStatus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RowKind {
    Same,
    Added,
    Deleted,
    Modified,
}

/// One row of the table: both sides' cells in column order (empty where a
/// side has no such cell) and which cells changed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TableRow {
    pub kind: RowKind,
    pub old_line: Option<u32>,
    pub new_line: Option<u32>,
    pub old: Vec<String>,
    pub new: Vec<String>,
    pub changed: Vec<bool>,
}

impl TableRow {
    /// The cell to show for a column of a row that is not modified.
    pub fn cell(&self, column: usize) -> &str {
        let side = if self.kind == RowKind::Deleted {
            &self.old
        } else {
            &self.new
        };
        side.get(column).map(String::as_str).unwrap_or("")
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TableDiff {
    pub columns: Vec<TableColumn>,
    /// The header row's line on each side (1 when present in a CSV file).
    pub header_lines: (Option<u32>, Option<u32>),
    pub rows: Vec<TableRow>,
}

impl TableDiff {
    /// How many rows changed (added, deleted or modified).
    pub fn changed_rows(&self) -> usize {
        self.rows.iter().filter(|r| r.kind != RowKind::Same).count()
    }
}

/// The table diff of two file versions (`old`/`new` = `None` for a side
/// that does not exist, an added or deleted file).
pub fn table_diff(
    old: Option<&str>,
    new: Option<&str>,
    delimiter: u8,
) -> Result<TableDiff, TableError> {
    let old = match old {
        Some(text) => parse(text, delimiter)?,
        None => Vec::new(),
    };
    let new = match new {
        Some(text) => parse(text, delimiter)?,
        None => Vec::new(),
    };
    records_diff(old, new)
}

/// `1321-table-file-diff`: a spreadsheet sheet's or database table's rows
/// as records numbered from `first_line`.
pub fn sheet_records(first_line: u32, rows: &[Vec<String>]) -> Vec<Record> {
    rows.iter()
        .enumerate()
        .map(|(ix, cells)| Record {
            line: first_line.saturating_add(ix as u32),
            cells: cells.clone(),
        })
        .collect()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SheetStatus {
    Same,
    Added,
    Deleted,
    Modified,
}

/// `1321-table-file-diff`: one sheet or table of a table file's diff.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SheetEntry {
    pub name: String,
    pub status: SheetStatus,
}

fn find_sheet<'a>(blob: Option<&'a TableBlob>, name: &str) -> Option<&'a TableSheet> {
    blob?.sheets.iter().find(|s| s.name == name)
}

/// The sheets of both versions: the new version's in order, then the
/// removed ones.
pub fn sheet_list(previous: Option<&TableBlob>, current: Option<&TableBlob>) -> Vec<SheetEntry> {
    let mut out: Vec<SheetEntry> = Vec::new();
    for sheet in current.iter().flat_map(|b| &b.sheets) {
        let status = match find_sheet(previous, &sheet.name) {
            None => SheetStatus::Added,
            Some(old) if old.rows != sheet.rows => SheetStatus::Modified,
            Some(_) => SheetStatus::Same,
        };
        out.push(SheetEntry {
            name: sheet.name.clone(),
            status,
        });
    }
    for sheet in previous.iter().flat_map(|b| &b.sheets) {
        if find_sheet(current, &sheet.name).is_none() {
            out.push(SheetEntry {
                name: sheet.name.clone(),
                status: SheetStatus::Deleted,
            });
        }
    }
    out
}

/// The table diff of one sheet, and whether either side was cut off.
pub fn sheet_diff(
    previous: Option<&TableBlob>,
    current: Option<&TableBlob>,
    name: &str,
) -> Result<(TableDiff, bool), TableError> {
    let records = |sheet: Option<&TableSheet>| {
        sheet.map_or_else(Vec::new, |s| sheet_records(s.first_line, &s.rows))
    };
    let (old, new) = (find_sheet(previous, name), find_sheet(current, name));
    let truncated = old.is_some_and(|s| s.truncated) || new.is_some_and(|s| s.truncated);
    Ok((records_diff(records(old), records(new))?, truncated))
}

/// The table diff of two versions' records (the first being the header;
/// empty for a side that does not exist).
pub fn records_diff(old: Vec<Record>, new: Vec<Record>) -> Result<TableDiff, TableError> {
    if old.len() + new.len() > MAX_ROWS {
        return Err(TableError::TooLarge);
    }
    let (old_header, old_rows) = match old.split_first() {
        Some((h, rest)) => (Some(h), rest),
        None => (None, &[][..]),
    };
    let (new_header, new_rows) = match new.split_first() {
        Some((h, rest)) => (Some(h), rest),
        None => (None, &[][..]),
    };
    if old_header.is_none() && new_header.is_none() {
        return Err(TableError::Empty);
    }
    let widest = |rows: &[Record], header: Option<&Record>| {
        rows.iter()
            .chain(header)
            .map(|r| r.cells.len())
            .max()
            .unwrap_or(0)
    };
    let mapping = map_columns(
        old_header.map(|h| h.cells.as_slice()),
        new_header.map(|h| h.cells.as_slice()),
        widest(old_rows, old_header),
        widest(new_rows, new_header),
    );
    let columns: Vec<TableColumn> = mapping
        .iter()
        .map(|&(o, n)| {
            let name_of = |h: Option<&Record>, ix: Option<usize>| -> Option<String> {
                Some(h?.cells.get(ix?)?.clone())
            };
            let (old_name, new_name) = (name_of(old_header, o), name_of(new_header, n));
            let status = match (o, n) {
                (Some(_), Some(_)) => ColumnStatus::Same,
                (None, _) => ColumnStatus::Added,
                (_, None) => ColumnStatus::Deleted,
            };
            let name = new_name.clone().or(old_name.clone()).unwrap_or_default();
            TableColumn {
                old_name: old_name.filter(|o| status == ColumnStatus::Same && *o != name),
                name,
                status,
            }
        })
        .collect();
    let lay_out = |record: &Record, old_side: bool| -> Vec<String> {
        mapping
            .iter()
            .map(|&(o, n)| {
                let ix = if old_side { o } else { n };
                ix.and_then(|ix| record.cells.get(ix).cloned())
                    .unwrap_or_default()
            })
            .collect()
    };
    let shared: Vec<usize> = mapping
        .iter()
        .enumerate()
        .filter(|(_, (o, n))| o.is_some() && n.is_some())
        .map(|(ix, _)| ix)
        .collect();
    let old_cells: Vec<Vec<String>> = old_rows.iter().map(|r| lay_out(r, true)).collect();
    let new_cells: Vec<Vec<String>> = new_rows.iter().map(|r| lay_out(r, false)).collect();
    let pick = |cells: &Vec<String>| hash_cells(shared.iter().map(|&c| &cells[c]));
    let old_keys: Vec<u64> = old_cells.iter().map(pick).collect();
    let new_keys: Vec<u64> = new_cells.iter().map(pick).collect();

    let mut rows = Vec::new();
    let row = |kind: RowKind, o: Option<usize>, n: Option<usize>| -> TableRow {
        let width = mapping.len();
        let old = o.map(|o| old_cells[o].clone()).unwrap_or_default();
        let new = n.map(|n| new_cells[n].clone()).unwrap_or_default();
        let mut changed = vec![false; width];
        let mut kind = kind;
        if let (Some(_), Some(_)) = (o, n) {
            for (c, flag) in changed.iter_mut().enumerate() {
                *flag = match columns[c].status {
                    ColumnStatus::Same => old[c] != new[c],
                    ColumnStatus::Added => !new[c].is_empty(),
                    ColumnStatus::Deleted => !old[c].is_empty(),
                };
            }
            kind = if changed.iter().any(|c| *c) {
                RowKind::Modified
            } else {
                RowKind::Same
            };
        }
        TableRow {
            kind,
            old_line: o.map(|o| old_rows[o].line),
            new_line: n.map(|n| new_rows[n].line),
            old,
            new,
            changed,
        }
    };
    for op in diff_ops(&old_keys, &new_keys) {
        let (tag, o, n) = op.as_tag_tuple();
        match tag {
            similar::DiffTag::Equal => {
                for (o, n) in o.zip(n) {
                    rows.push(row(RowKind::Same, Some(o), Some(n)));
                }
            }
            similar::DiffTag::Delete => {
                rows.extend(o.map(|o| row(RowKind::Deleted, Some(o), None)))
            }
            similar::DiffTag::Insert => rows.extend(n.map(|n| row(RowKind::Added, None, Some(n)))),
            similar::DiffTag::Replace => {
                pair_block(o, n, &old_cells, &new_cells, &shared, &mut |kind, o, n| {
                    rows.push(row(kind, o, n))
                });
            }
        }
    }
    Ok(TableDiff {
        columns,
        // a database table's header is line 0: no number
        header_lines: (
            old_header.map(|h| h.line).filter(|&l| l > 0),
            new_header.map(|h| h.line).filter(|&l| l > 0),
        ),
        rows,
    })
}

/// Inside a block of changed rows: rows with the same first shared cell
/// (a key, usually) pair up as modified rows; the rest pair by position
/// when both runs between keys are as long, else stay removed and added.
fn pair_block(
    o: std::ops::Range<usize>,
    n: std::ops::Range<usize>,
    old_cells: &[Vec<String>],
    new_cells: &[Vec<String>],
    shared: &[usize],
    emit: &mut dyn FnMut(RowKind, Option<usize>, Option<usize>),
) {
    let key = |cells: &Vec<String>| hash_cells(shared.first().map(|&c| &cells[c]));
    let ok: Vec<u64> = o.clone().map(|i| key(&old_cells[i])).collect();
    let nk: Vec<u64> = n.clone().map(|i| key(&new_cells[i])).collect();
    for op in diff_ops(&ok, &nk) {
        let (tag, a, b) = op.as_tag_tuple();
        let (a, b) = (
            (o.start + a.start)..(o.start + a.end),
            (n.start + b.start)..(n.start + b.end),
        );
        match tag {
            similar::DiffTag::Equal => {
                for (x, y) in a.zip(b) {
                    emit(RowKind::Modified, Some(x), Some(y));
                }
            }
            similar::DiffTag::Replace if a.len() == b.len() => {
                for (x, y) in a.zip(b) {
                    emit(RowKind::Modified, Some(x), Some(y));
                }
            }
            _ => {
                for x in a {
                    emit(RowKind::Deleted, Some(x), None);
                }
                for y in b {
                    emit(RowKind::Added, None, Some(y));
                }
            }
        }
    }
}

/// Column pairs (old index, new index) in display order: the new file's
/// columns, then the old-only ones.
fn map_columns(
    old: Option<&[String]>,
    new: Option<&[String]>,
    old_width: usize,
    new_width: usize,
) -> Vec<(Option<usize>, Option<usize>)> {
    let unique = |h: &[String]| h.iter().collect::<HashSet<_>>().len() == h.len();
    match (old, new) {
        (Some(o), Some(n)) if unique(o) && unique(n) && o != n => {
            let mut out: Vec<(Option<usize>, Option<usize>)> = (0..new_width)
                .map(|ni| {
                    let oi = n.get(ni).and_then(|name| o.iter().position(|x| x == name));
                    (oi, Some(ni))
                })
                .collect();
            for oi in 0..old_width {
                let kept = o.get(oi).is_some_and(|name| n.iter().any(|x| x == name));
                if !kept {
                    out.push((Some(oi), None));
                }
            }
            out
        }
        _ => {
            let (has_old, has_new) = (old.is_some(), new.is_some());
            (0..old_width.max(new_width))
                .map(|ix| {
                    (
                        (has_old && ix < old_width).then_some(ix),
                        (has_new && ix < new_width).then_some(ix),
                    )
                })
                .collect()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(diff: &TableDiff) -> String {
        diff.rows
            .iter()
            .map(|r| match r.kind {
                RowKind::Same => '.',
                RowKind::Added => '+',
                RowKind::Deleted => '-',
                RowKind::Modified => 'm',
            })
            .collect()
    }

    #[test]
    fn parses_quotes_and_line_breaks() {
        let records = parse("a,b\n\"x, y\",\"say \"\"hi\"\"\"\n\"two\nlines\",3\n", b',').unwrap();
        assert_eq!(records.len(), 3);
        assert_eq!(records[1].cells, vec!["x, y", "say \"hi\""]);
        assert_eq!(records[2].cells, vec!["two\nlines", "3"]);
        assert_eq!(records[2].line, 3);
        assert_eq!(parse("a\tb\n1\t2", b'\t').unwrap()[1].cells, vec!["1", "2"]);
        assert_eq!(parse("a,\"b\n", b','), Err(TableError::Unterminated(1)));
        assert_eq!(parse("a,b\n\n1,2\n", b',').unwrap().len(), 2);
        assert_eq!(parse("a,\n", b',').unwrap()[0].cells, vec!["a", ""]);
        // plain TSV: a lone quote is text
        assert_eq!(
            parse("a\tb\n\"x\t2\n", b'\t').unwrap()[1].cells,
            vec!["\"x", "2"]
        );
    }

    #[test]
    fn edited_row_is_modified_with_changed_cells() {
        let old = "id,name,score\n1,Alice,90\n2,Bob,85\n3,Carol,77\n";
        let new = "id,name,score\n1,Alice,92\n3,Carol,77\n4,Dave,60\n";
        let diff = table_diff(Some(old), Some(new), b',').unwrap();
        assert_eq!(kinds(&diff), "m-.+");
        assert_eq!(diff.rows[0].changed, vec![false, false, true]);
        assert_eq!(diff.rows[0].old_line, Some(2));
        assert_eq!(diff.rows[3].new_line, Some(4));
        assert_eq!(diff.changed_rows(), 3);
    }

    #[test]
    fn columns_match_by_name() {
        let old = "id,name,city\n1,Alice,Paris\n";
        let new = "id,city,name,age\n1,Paris,Alice,30\n";
        let diff = table_diff(Some(old), Some(new), b',').unwrap();
        let names: Vec<&str> = diff.columns.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, vec!["id", "city", "name", "age"]);
        assert_eq!(diff.columns[3].status, ColumnStatus::Added);
        assert_eq!(kinds(&diff), "m");
        assert_eq!(diff.rows[0].changed, vec![false, false, false, true]);
        let diff = table_diff(Some(new), Some(old), b',').unwrap();
        assert_eq!(diff.columns[3].status, ColumnStatus::Deleted);
        assert_eq!(diff.columns[3].name, "age");
    }

    #[test]
    fn renamed_header_matches_by_position() {
        let old = "a,b\n1,2\n";
        let new = "a,c\n1,2\n";
        let diff = table_diff(Some(old), Some(new), b',').unwrap();
        // names differ but are unique: b is deleted and c added
        assert_eq!(diff.columns.len(), 3);
        let dup = table_diff(Some("x,x\n1,2\n"), Some("x,y\n1,2\n"), b',').unwrap();
        assert_eq!(dup.columns.len(), 2);
        assert_eq!(dup.columns[1].old_name.as_deref(), Some("x"));
        assert_eq!(kinds(&dup), ".");
    }

    #[test]
    fn added_and_deleted_files() {
        let diff = table_diff(None, Some("a,b\n1,2\n"), b',').unwrap();
        assert_eq!(kinds(&diff), "+");
        assert_eq!(diff.columns[0].status, ColumnStatus::Added);
        let diff = table_diff(Some("a,b\n1,2\n"), None, b',').unwrap();
        assert_eq!(kinds(&diff), "-");
        assert_eq!(diff.rows[0].cell(1), "2");
        assert_eq!(table_diff(None, Some(""), b','), Err(TableError::Empty));
    }

    #[test]
    fn unkeyed_runs_of_equal_length_pair_up() {
        let old = "k,v\na,1\nb,2\n";
        let new = "k,v\nc,1\nd,2\n";
        let diff = table_diff(Some(old), Some(new), b',').unwrap();
        assert_eq!(kinds(&diff), "mm");
        assert_eq!(diff.rows[1].changed, vec![true, false]);
    }

    #[test]
    fn sheets_of_two_versions() {
        let sheet = |name: &str, rows: &[&[&str]]| TableSheet {
            name: name.into(),
            first_line: 1,
            rows: rows
                .iter()
                .map(|r| r.iter().map(|c| c.to_string()).collect())
                .collect(),
            truncated: false,
        };
        let old = TableBlob {
            sheets: vec![
                sheet("Kept", &[&["a"], &["1"]]),
                sheet("Edited", &[&["a"], &["1"]]),
                sheet("Gone", &[&["a"]]),
            ],
        };
        let new = TableBlob {
            sheets: vec![
                sheet("Edited", &[&["a"], &["2"]]),
                sheet("Kept", &[&["a"], &["1"]]),
                sheet("New", &[&["a"]]),
            ],
        };
        let list = sheet_list(Some(&old), Some(&new));
        let got: Vec<_> = list.iter().map(|e| (e.name.as_str(), e.status)).collect();
        assert_eq!(
            got,
            [
                ("Edited", SheetStatus::Modified),
                ("Kept", SheetStatus::Same),
                ("New", SheetStatus::Added),
                ("Gone", SheetStatus::Deleted),
            ]
        );
        let (diff, truncated) = sheet_diff(Some(&old), Some(&new), "Edited").unwrap();
        assert!(!truncated);
        assert_eq!(kinds(&diff), "m");
        assert_eq!(diff.rows[0].new_line, Some(2));
        let (diff, _) = sheet_diff(Some(&old), Some(&new), "Gone").unwrap();
        assert_eq!(diff.columns[0].name, "a");
    }

    #[test]
    fn delimiter_from_path() {
        assert_eq!(table_delimiter("data/x.CSV"), Some(b','));
        assert_eq!(table_delimiter("x.tsv"), Some(b'\t'));
        assert_eq!(table_delimiter("x.txt"), None);
    }
}
