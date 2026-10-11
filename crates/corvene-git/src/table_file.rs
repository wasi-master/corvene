//! Corvene `1321-table-file-diff`: binary spreadsheet and database files
//! read as tables, so their diff can use the CSV table view.
//!
//! Spreadsheets (.xlsx, .xlsm, .xlsb, .xls, .ods) are read with calamine,
//! one table per sheet. SQLite databases are copied to a temporary
//! directory and opened there, one table per database table. dBASE (.dbf)
//! files are one table. GitHub Desktop shows all of them as binary files
//! (`app/src/lib/git/diff.ts` `getBinaryPaths`).

use std::sync::atomic::{AtomicBool, Ordering};

use corvene_models::{Diff, FileStatusKind, TableBlob, TableSheet};

/// Set by the dispatcher from the flag.
static READ_TABLE_FILES: AtomicBool = AtomicBool::new(false);

/// Turn table diffs of binary table files on or off.
pub fn set_read_table_files(enabled: bool) {
    READ_TABLE_FILES.store(enabled, Ordering::Relaxed);
}

fn read_table_files() -> bool {
    READ_TABLE_FILES.load(Ordering::Relaxed)
}

/// Larger files stay binary.
pub const MAX_BYTES: usize = 64 << 20;
/// Data rows read per sheet; the rest are left out (both sides together
/// stay within the table diff's row limit).
pub const MAX_ROWS: usize = 10_000;
/// Longest cell text kept (a database can hold whole documents).
const MAX_CELL: usize = 1_000;

const SQLITE_MAGIC: &[u8] = b"SQLite format 3\0";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TableFormat {
    Spreadsheet,
    Sqlite,
    Dbase,
}

/// The table format of a file, from its extension.
pub fn table_format(path: &str) -> Option<TableFormat> {
    let ext = path.rsplit_once('.')?.1.to_ascii_lowercase();
    match ext.as_str() {
        "xlsx" | "xlsm" | "xlsb" | "xls" | "xla" | "xlam" | "ods" => Some(TableFormat::Spreadsheet),
        "sqlite" | "sqlite3" | "db" | "db3" | "s3db" | "sl3" | "gpkg" | "mbtiles" => {
            Some(TableFormat::Sqlite)
        }
        "dbf" => Some(TableFormat::Dbase),
        _ => None,
    }
}

/// A binary change of a table file as a [`Diff::Table`]; `None` when the
/// flag is off, the path is not a table file, a side is too large or does
/// not read (the caller keeps its binary diff).
pub fn table_diff(
    path: &str,
    kind: FileStatusKind,
    current: impl FnOnce() -> Option<Vec<u8>>,
    previous: impl FnOnce() -> Option<Vec<u8>>,
) -> Option<Diff> {
    if !read_table_files() {
        return None;
    }
    let format = table_format(path)?;
    let side = |bytes: Option<Vec<u8>>| -> Option<Option<TableBlob>> {
        match bytes {
            None => Some(None),
            Some(bytes) if bytes.len() > MAX_BYTES => None,
            Some(bytes) => match read(format, &bytes) {
                Ok(blob) => Some(Some(blob)),
                Err(e) => {
                    tracing::debug!(path, error = %e, "table file did not read");
                    None
                }
            },
        }
    };
    let current = side((kind != FileStatusKind::Deleted).then(current).flatten())?;
    let previous = side((!kind.is_new_or_untracked()).then(previous).flatten())?;
    if current.is_none() && previous.is_none() {
        return None;
    }
    Some(Diff::Table { previous, current })
}

/// The tables of one version of a file.
pub fn read(format: TableFormat, bytes: &[u8]) -> Result<TableBlob, String> {
    match format {
        TableFormat::Spreadsheet => read_spreadsheet(bytes),
        TableFormat::Sqlite => read_sqlite(bytes),
        TableFormat::Dbase => read_dbase(bytes),
    }
}

fn clip(mut text: String) -> String {
    if text.len() > MAX_CELL {
        let mut end = MAX_CELL;
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        text.truncate(end);
        text.push('…');
    }
    text
}

fn read_spreadsheet(bytes: &[u8]) -> Result<TableBlob, String> {
    use calamine::Reader;
    let mut book = calamine::open_workbook_auto_from_rs(std::io::Cursor::new(bytes.to_vec()))
        .map_err(|e| e.to_string())?;
    let mut sheets = Vec::new();
    for name in book.sheet_names() {
        let range = book.worksheet_range(&name).map_err(|e| e.to_string())?;
        let first_line = range.start().map_or(1, |(row, _)| row + 1);
        let mut rows: Vec<Vec<String>> = range
            .rows()
            .take(MAX_ROWS + 2)
            .map(|row| row.iter().map(|c| clip(cell_text(c))).collect())
            .collect();
        let truncated = rows.len() > MAX_ROWS + 1;
        rows.truncate(MAX_ROWS + 1);
        sheets.push(TableSheet {
            name,
            first_line,
            rows,
            truncated,
        });
    }
    Ok(TableBlob { sheets })
}

/// A spreadsheet cell as text; dates in ISO form (calamine prints their
/// serial number).
fn cell_text(cell: &calamine::Data) -> String {
    match cell {
        calamine::Data::DateTime(d) if d.is_datetime() => {
            let (y, mo, day, h, mi, s, _) = d.to_ymd_hms_milli();
            let value = d.as_f64();
            if value < 1.0 {
                format!("{h:02}:{mi:02}:{s:02}")
            } else if value.fract() == 0.0 {
                format!("{y:04}-{mo:02}-{day:02}")
            } else {
                format!("{y:04}-{mo:02}-{day:02} {h:02}:{mi:02}:{s:02}")
            }
        }
        other => other.to_string(),
    }
}

fn read_sqlite(bytes: &[u8]) -> Result<TableBlob, String> {
    if !bytes.starts_with(SQLITE_MAGIC) {
        return Err("not an SQLite database".into());
    }
    // a private copy: opened read-write so a WAL database can make its
    // shared-memory file, and the version on disk is never locked
    let dir = tempfile::tempdir().map_err(|e| e.to_string())?;
    let path = dir.path().join("table.db");
    std::fs::write(&path, bytes).map_err(|e| e.to_string())?;
    let db = rusqlite::Connection::open_with_flags(
        &path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|e| e.to_string())?;
    let names: Vec<String> = {
        let mut stmt = db
            .prepare(
                "SELECT name FROM sqlite_schema WHERE type = 'table' \
                 AND name NOT LIKE 'sqlite_%' ORDER BY name",
            )
            .map_err(|e| e.to_string())?;
        stmt.query_map([], |row| row.get(0))
            .map_err(|e| e.to_string())?
            .collect::<Result<_, _>>()
            .map_err(|e| e.to_string())?
    };
    let mut sheets = Vec::new();
    for name in names {
        match sqlite_table(&db, &name) {
            Ok((rows, truncated)) => sheets.push(TableSheet {
                name,
                first_line: 0,
                rows,
                truncated,
            }),
            // a virtual table whose module this build lacks
            Err(e) => tracing::debug!(table = name, error = %e, "table did not read"),
        }
    }
    Ok(TableBlob { sheets })
}

fn sqlite_table(
    db: &rusqlite::Connection,
    name: &str,
) -> rusqlite::Result<(Vec<Vec<String>>, bool)> {
    let quoted = name.replace('"', "\"\"");
    let mut stmt = db.prepare(&format!(
        "SELECT * FROM \"{quoted}\" LIMIT {}",
        MAX_ROWS + 1
    ))?;
    let header: Vec<String> = stmt.column_names().into_iter().map(String::from).collect();
    let width = header.len();
    let mut rows = vec![header];
    let mut query = stmt.query([])?;
    while let Some(row) = query.next()? {
        let mut cells = Vec::with_capacity(width);
        for ix in 0..width {
            cells.push(clip(sqlite_value(row.get_ref(ix)?)));
        }
        rows.push(cells);
    }
    let truncated = rows.len() > MAX_ROWS + 1;
    rows.truncate(MAX_ROWS + 1);
    Ok((rows, truncated))
}

fn sqlite_value(value: rusqlite::types::ValueRef<'_>) -> String {
    use rusqlite::types::ValueRef;
    match value {
        ValueRef::Null => "NULL".into(),
        ValueRef::Integer(i) => i.to_string(),
        ValueRef::Real(f) => f.to_string(),
        ValueRef::Text(t) => String::from_utf8_lossy(t).into_owned(),
        ValueRef::Blob(b) => blob_text(b),
    }
}

/// A blob as a SQL hex literal, cut after 32 bytes.
fn blob_text(bytes: &[u8]) -> String {
    use std::fmt::Write;
    let mut out = String::from("x'");
    for b in bytes.iter().take(32) {
        let _ = write!(out, "{b:02x}");
    }
    out.push('\'');
    if bytes.len() > 32 {
        let _ = write!(out, " ({} bytes)", bytes.len());
    }
    out
}

/// A dBASE III+ style table: a 32-byte header, 32-byte field descriptors
/// ended by 0x0D, then fixed-width records each led by a deletion flag.
fn read_dbase(bytes: &[u8]) -> Result<TableBlob, String> {
    let short = || "truncated dBASE file".to_string();
    let u16_at = |at: usize| -> Option<usize> {
        Some(u16::from_le_bytes(bytes.get(at..at + 2)?.try_into().ok()?) as usize)
    };
    let count = u32::from_le_bytes(
        bytes
            .get(4..8)
            .ok_or_else(short)?
            .try_into()
            .map_err(|_| short())?,
    ) as usize;
    let header_len = u16_at(8).ok_or_else(short)?;
    let record_len = u16_at(10).ok_or_else(short)?;
    let mut fields = Vec::new();
    let mut at = 32;
    while at + 32 <= header_len.min(bytes.len()) && bytes[at] != 0x0d {
        let desc = &bytes[at..at + 32];
        let name_end = desc[..11].iter().position(|&b| b == 0).unwrap_or(11);
        let name = String::from_utf8_lossy(&desc[..name_end]).into_owned();
        fields.push((name, desc[16] as usize));
        at += 32;
    }
    if fields.is_empty() || record_len == 0 {
        return Err("no dBASE fields".into());
    }
    let mut rows = vec![fields.iter().map(|(n, _)| n.clone()).collect::<Vec<_>>()];
    let mut truncated = false;
    for ix in 0..count {
        let start = header_len + ix * record_len;
        let Some(record) = bytes.get(start..start + record_len) else {
            break;
        };
        if record[0] == b'*' {
            continue;
        }
        if rows.len() > MAX_ROWS {
            truncated = true;
            break;
        }
        let mut pos = 1;
        let mut cells = Vec::with_capacity(fields.len());
        for (_, len) in &fields {
            let cell = record.get(pos..pos + len).unwrap_or(&[]);
            cells.push(clip(String::from_utf8_lossy(cell).trim().to_string()));
            pos += len;
        }
        rows.push(cells);
    }
    Ok(TableBlob {
        sheets: vec![TableSheet {
            name: "Table".into(),
            first_line: 0,
            rows,
            truncated,
        }],
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_from_extensions() {
        assert_eq!(table_format("a/Book.XLSX"), Some(TableFormat::Spreadsheet));
        assert_eq!(table_format("app.db"), Some(TableFormat::Sqlite));
        assert_eq!(table_format("x.dbf"), Some(TableFormat::Dbase));
        assert_eq!(table_format("x.csv"), None);
        assert_eq!(table_format("Makefile"), None);
    }

    fn sqlite_bytes(sql: &str) -> Vec<u8> {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.db");
        let db = rusqlite::Connection::open(&path).unwrap();
        db.execute_batch(sql).unwrap();
        drop(db);
        std::fs::read(path).unwrap()
    }

    #[test]
    fn reads_sqlite_tables() {
        let bytes = sqlite_bytes(
            "CREATE TABLE b(id INTEGER, name TEXT, data BLOB, score REAL); \
             INSERT INTO b VALUES (1, 'one', x'00ff', 1.5), (2, NULL, NULL, NULL); \
             CREATE TABLE a(x); INSERT INTO a VALUES ('y');",
        );
        let blob = read(TableFormat::Sqlite, &bytes).unwrap();
        let names: Vec<_> = blob.sheets.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, ["a", "b"]);
        let b = &blob.sheets[1];
        assert_eq!(b.rows[0], ["id", "name", "data", "score"]);
        assert_eq!(b.rows[1], ["1", "one", "x'00ff'", "1.5"]);
        assert_eq!(b.rows[2], ["2", "NULL", "NULL", "NULL"]);
        assert!(!b.truncated);
    }

    #[test]
    fn reads_wal_sqlite_and_rejects_other_bytes() {
        let bytes =
            sqlite_bytes("PRAGMA journal_mode=WAL; CREATE TABLE t(v); INSERT INTO t VALUES (7);");
        let blob = read(TableFormat::Sqlite, &bytes).unwrap();
        assert_eq!(blob.sheets[0].rows[1], ["7"]);
        assert!(read(TableFormat::Sqlite, b"not a database").is_err());
    }

    #[test]
    fn reads_dbase_records() {
        let mut bytes = vec![0u8; 32];
        bytes[0] = 3;
        bytes[4..8].copy_from_slice(&2u32.to_le_bytes());
        let header_len = 32 + 2 * 32 + 1;
        bytes[8..10].copy_from_slice(&(header_len as u16).to_le_bytes());
        bytes[10..12].copy_from_slice(&(1u16 + 4 + 3).to_le_bytes());
        for (name, kind, len) in [("NAME", b'C', 4u8), ("AGE", b'N', 3)] {
            let mut desc = [0u8; 32];
            desc[..name.len()].copy_from_slice(name.as_bytes());
            desc[11] = kind;
            desc[16] = len;
            bytes.extend_from_slice(&desc);
        }
        bytes.push(0x0d);
        bytes.extend_from_slice(b" Ann  42*Bob   7");
        let blob = read(TableFormat::Dbase, &bytes).unwrap();
        assert_eq!(
            blob.sheets[0].rows,
            [vec!["NAME", "AGE"], vec!["Ann", "42"]]
        );
    }

    #[test]
    fn diff_needs_the_flag_and_a_table_path() {
        let bytes = sqlite_bytes("CREATE TABLE t(v);");
        set_read_table_files(true);
        let diff = table_diff(
            "x.sqlite",
            FileStatusKind::New,
            || Some(bytes.clone()),
            || None,
        );
        assert!(matches!(
            diff,
            Some(Diff::Table {
                previous: None,
                current: Some(_)
            })
        ));
        assert!(
            table_diff(
                "x.bin",
                FileStatusKind::New,
                || Some(bytes.clone()),
                || None
            )
            .is_none()
        );
        assert!(
            table_diff(
                "x.db",
                FileStatusKind::Modified,
                || Some(b"junk".to_vec()),
                || None
            )
            .is_none()
        );
    }
}
