use std::io::Write;
use std::sync::Arc;

use corvene_models::{
    ArchiveEntryKind, ArchiveEntryStatus, ArchiveFormat, ArchiveRoot, ArchiveSource, Diff,
};

use super::*;

fn zip_of(files: &[(&str, &[u8])]) -> Vec<u8> {
    zip_with(files, |o| o)
}

fn zip_with(
    files: &[(&str, &[u8])],
    options: impl Fn(zip::write::SimpleFileOptions) -> zip::write::SimpleFileOptions,
) -> Vec<u8> {
    let mut out = std::io::Cursor::new(Vec::new());
    {
        let mut writer = zip::ZipWriter::new(&mut out);
        for (name, bytes) in files {
            let o = options(
                zip::write::SimpleFileOptions::default()
                    .compression_method(zip::CompressionMethod::Deflated)
                    .last_modified_time(
                        zip::DateTime::from_date_and_time(2026, 10, 1, 12, 0, 0).unwrap(),
                    ),
            );
            if name.ends_with('/') {
                writer.add_directory(*name, o).unwrap();
            } else {
                writer.start_file(*name, o).unwrap();
                writer.write_all(bytes).unwrap();
            }
        }
        writer.finish().unwrap();
    }
    out.into_inner()
}

fn tar_of(files: &[(&str, &[u8])]) -> Vec<u8> {
    let mut builder = tar::Builder::new(Vec::new());
    for (name, bytes) in files {
        let mut header = tar::Header::new_gnu();
        header.set_size(bytes.len() as u64);
        header.set_mode(0o644);
        header.set_mtime(1_700_000_000);
        header.set_cksum();
        builder.append_data(&mut header, name, *bytes).unwrap();
    }
    builder.into_inner().unwrap()
}

fn gzip(bytes: &[u8]) -> Vec<u8> {
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
    encoder.write_all(bytes).unwrap();
    encoder.finish().unwrap()
}

/// Writes `bytes` to a file and pins it as a source.
fn source(dir: &std::path::Path, name: &str, bytes: &[u8]) -> ArchiveSource {
    let path = dir.join(name.replace('/', "_"));
    std::fs::write(&path, bytes).unwrap();
    let meta = std::fs::symlink_metadata(&path).unwrap();
    ArchiveSource {
        root: ArchiveRoot::File {
            path,
            len: meta.len(),
            mtime_ns: mtime_ns(&meta),
        },
        name: name.to_string(),
        chain: Vec::new(),
    }
}

fn git() -> Arc<GitBinary> {
    Arc::new(crate::find_git().unwrap())
}

fn status_of(diff: &ArchiveDiff, path: &str) -> ArchiveEntryStatus {
    diff.entry(path)
        .unwrap_or_else(|| panic!("{path} missing from {:?}", diff.entries))
        .status
}

#[test]
fn formats_from_extensions() {
    assert_eq!(archive_format("a/b.zip"), Some(ArchiveFormat::Zip));
    assert_eq!(archive_format("lib.JAR"), Some(ArchiveFormat::Zip));
    assert_eq!(archive_format("x.tar.gz"), Some(ArchiveFormat::TarGz));
    assert_eq!(archive_format("x.tgz"), Some(ArchiveFormat::TarGz));
    assert_eq!(archive_format("x.tar.zst"), Some(ArchiveFormat::TarZst));
    assert_eq!(archive_format("log.txt.gz"), Some(ArchiveFormat::Gzip));
    assert_eq!(archive_format("dump.xz"), Some(ArchiveFormat::Xz));
    assert_eq!(archive_format(".gz"), None);
    assert_eq!(archive_format("zip"), None);
    assert_eq!(archive_format("notes.txt"), None);
}

#[test]
fn lists_and_compares_zip_versions() {
    let dir = tempfile::tempdir().unwrap();
    let old = zip_of(&[
        ("src/", b""),
        ("src/main.rs", b"fn main() {}\n"),
        ("README.md", b"hello\n"),
        ("old/name.txt", b"moved contents\n"),
        ("gone.txt", b"bye\n"),
    ]);
    let new = zip_of(&[
        ("src/", b""),
        ("src/main.rs", b"fn main() {\n    run();\n}\n"),
        ("README.md", b"hello\n"),
        ("new/name.txt", b"moved contents\n"),
        ("fresh.txt", b"hi\n"),
    ]);
    let git = git();
    let diff = diff_sources(
        &git,
        dir.path(),
        Some(source(dir.path(), "old.zip", &old)),
        Some(source(dir.path(), "new.zip", &new)),
    );
    assert_eq!(diff.unreadable, None);
    assert_eq!(diff.format, ArchiveFormat::Zip);
    assert_eq!(
        status_of(&diff, "src/main.rs"),
        ArchiveEntryStatus::Modified
    );
    assert_eq!(status_of(&diff, "README.md"), ArchiveEntryStatus::Unchanged);
    assert_eq!(
        status_of(&diff, "new/name.txt"),
        ArchiveEntryStatus::Renamed
    );
    assert_eq!(status_of(&diff, "gone.txt"), ArchiveEntryStatus::Deleted);
    assert_eq!(status_of(&diff, "fresh.txt"), ArchiveEntryStatus::Added);
    assert!(diff.entry("src").is_some_and(|e| e.is_dir()));
    let counts = diff.counts();
    assert_eq!(
        (
            counts.added,
            counts.deleted,
            counts.modified,
            counts.renamed
        ),
        (1, 1, 1, 1)
    );

    let entry = entry_diff(&git, dir.path(), &diff, "src/main.rs", false).unwrap();
    let Diff::Text { hunks, .. } = &entry.diff else {
        panic!("text diff expected: {:?}", entry.diff);
    };
    let added: Vec<_> = hunks[0]
        .lines
        .iter()
        .filter(|l| l.kind == corvene_models::DiffLineKind::Add)
        .map(|l| l.text.as_str())
        .collect();
    assert!(added.iter().any(|t| t.contains("run();")), "{added:?}");
    assert_eq!(entry.new_lines.as_ref().map(Vec::len), Some(3));
    assert_eq!(entry.old_lines.as_ref().map(Vec::len), Some(1));

    // an added entry diffs against nothing
    let fresh = entry_diff(&git, dir.path(), &diff, "fresh.txt", false).unwrap();
    assert!(matches!(fresh.diff, Diff::Text { .. }));
    assert_eq!(fresh.old_lines, None);
}

#[test]
fn metadata_only_changes_are_told_apart() {
    let dir = tempfile::tempdir().unwrap();
    let files: &[(&str, &[u8])] = &[("a.txt", b"same\n")];
    let old = zip_of(files);
    let new = zip_with(files, |o| {
        o.last_modified_time(zip::DateTime::from_date_and_time(2026, 10, 12, 9, 30, 0).unwrap())
    });
    let git = git();
    let diff = diff_sources(
        &git,
        dir.path(),
        Some(source(dir.path(), "a.zip", &old)),
        Some(source(dir.path(), "b.zip", &new)),
    );
    assert_eq!(status_of(&diff, "a.txt"), ArchiveEntryStatus::MetadataOnly);
    assert_eq!(diff.counts().content_changes(), 0);
    let entry = entry_diff(&git, dir.path(), &diff, "a.txt", false).unwrap();
    assert_eq!(entry.diff, Diff::Empty);
}

#[test]
fn reads_compressed_tar_and_single_files() {
    let dir = tempfile::tempdir().unwrap();
    let git = git();
    let old = gzip(&tar_of(&[
        ("site/index.html", b"<p>a</p>\n"),
        ("site/x.css", b"a{}\n"),
    ]));
    let new = gzip(&tar_of(&[
        ("site/index.html", b"<p>b</p>\n"),
        ("site/x.css", b"a{}\n"),
    ]));
    let diff = diff_sources(
        &git,
        dir.path(),
        Some(source(dir.path(), "dist/site.tar.gz", &old)),
        Some(source(dir.path(), "dist/site2.tar.gz", &new)),
    );
    assert_eq!(diff.format, ArchiveFormat::TarGz);
    assert_eq!(
        status_of(&diff, "site/index.html"),
        ArchiveEntryStatus::Modified
    );
    assert_eq!(
        status_of(&diff, "site/x.css"),
        ArchiveEntryStatus::Unchanged
    );
    let meta = diff.entry("site/index.html").unwrap().new.clone().unwrap();
    assert_eq!(meta.mode, Some(0o644));
    assert_eq!(meta.mtime, Some(1_700_000_000));

    // a .gz that holds a tar archive is read as one
    let sneaky = source(dir.path(), "bundle.gz", &new);
    let listed = diff_sources(&git, dir.path(), None, Some(sneaky));
    assert_eq!(listed.format, ArchiveFormat::TarGz);
    assert_eq!(listed.entries.len(), 2);

    // one compressed file: its contents diff directly
    let diff = diff_sources(
        &git,
        dir.path(),
        Some(source(dir.path(), "logs/run.txt.gz", &gzip(b"one\ntwo\n"))),
        Some(source(
            dir.path(),
            "logs/run2.txt.gz",
            &gzip(b"one\nthree\n"),
        )),
    );
    assert_eq!(diff.format, ArchiveFormat::Gzip);
    assert_eq!(diff.single.as_deref(), Some("run2.txt"));
    assert_eq!(diff.entries.len(), 1);
    assert_eq!(diff.entries[0].status, ArchiveEntryStatus::Modified);
    let entry = entry_diff(&git, dir.path(), &diff, &diff.entries[0].path, false).unwrap();
    assert!(matches!(entry.diff, Diff::Text { .. }), "{:?}", entry.diff);
}

#[test]
fn reads_bzip2_xz_and_zstd() {
    let dir = tempfile::tempdir().unwrap();
    let git = git();
    let tar = tar_of(&[("a.txt", b"alpha\n")]);
    let mut bz = bzip2::write::BzEncoder::new(Vec::new(), bzip2::Compression::fast());
    bz.write_all(&tar).unwrap();
    let bz = bz.finish().unwrap();
    let mut xz = lzma_rust2::XzWriter::new(Vec::new(), lzma_rust2::XzOptions::default()).unwrap();
    xz.write_all(&tar).unwrap();
    let xz = xz.finish().unwrap();
    let zst =
        ruzstd::encoding::compress_to_vec(&tar[..], ruzstd::encoding::CompressionLevel::Fastest);
    for (name, bytes, format) in [
        ("x.tar.bz2", bz, ArchiveFormat::TarBz2),
        ("x.txz", xz, ArchiveFormat::TarXz),
        ("x.tar.zst", zst, ArchiveFormat::TarZst),
    ] {
        let diff = diff_sources(
            &git,
            dir.path(),
            None,
            Some(source(dir.path(), name, &bytes)),
        );
        assert_eq!(diff.unreadable, None, "{name}");
        assert_eq!(diff.format, format, "{name}");
        assert_eq!(
            status_of(&diff, "a.txt"),
            ArchiveEntryStatus::Added,
            "{name}"
        );
    }
}

#[test]
fn opens_nested_archives() {
    let dir = tempfile::tempdir().unwrap();
    let git = git();
    let inner_old = zip_of(&[("com/A.class", b"old class")]);
    let inner_new = zip_of(&[("com/A.class", b"new class"), ("com/B.class", b"b")]);
    let old = zip_of(&[("libs/lib.jar", &inner_old)]);
    let new = zip_of(&[("libs/lib.jar", &inner_new)]);
    let outer = diff_sources(
        &git,
        dir.path(),
        Some(source(dir.path(), "app.zip", &old)),
        Some(source(dir.path(), "app2.zip", &new)),
    );
    assert_eq!(
        status_of(&outer, "libs/lib.jar"),
        ArchiveEntryStatus::Modified
    );
    let entry = entry_diff(&git, dir.path(), &outer, "libs/lib.jar", false).unwrap();
    let Diff::Archive(inner) = entry.diff else {
        panic!("nested archive expected");
    };
    assert_eq!(inner.new.as_ref().unwrap().source.chain, ["libs/lib.jar"]);
    assert_eq!(
        status_of(&inner, "com/A.class"),
        ArchiveEntryStatus::Modified
    );
    assert_eq!(status_of(&inner, "com/B.class"), ArchiveEntryStatus::Added);
    let bytes = entry_bytes(&git, dir.path(), &inner, "com/A.class", EntrySide::Old).unwrap();
    assert_eq!(&bytes[..], b"old class");
}

#[test]
fn corrupt_versions_say_why() {
    let dir = tempfile::tempdir().unwrap();
    let git = git();
    let diff = diff_sources(
        &git,
        dir.path(),
        None,
        Some(source(dir.path(), "broken.zip", b"PK\x03\x04 nothing else")),
    );
    assert!(diff.unreadable.is_some());
    assert!(diff.entries.is_empty());
    let diff = diff_sources(
        &git,
        dir.path(),
        None,
        Some(source(dir.path(), "plain.gz", b"not compressed")),
    );
    assert!(diff.unreadable.is_some());
}

#[test]
fn pinned_files_refuse_changed_contents() {
    let dir = tempfile::tempdir().unwrap();
    let git = git();
    clear_cache();
    let pinned = source(dir.path(), "a.zip", &zip_of(&[("a", b"1")]));
    let ArchiveRoot::File { path, .. } = &pinned.root else {
        unreachable!()
    };
    std::fs::write(path, zip_of(&[("a", b"12345")])).unwrap();
    let diff = diff_sources(&git, dir.path(), None, Some(pinned));
    assert_eq!(
        diff.unreadable.as_deref(),
        Some("The file changed on disk.")
    );
}

#[test]
fn caps_entry_counts() {
    let dir = tempfile::tempdir().unwrap();
    let git = git();
    let names: Vec<String> = (0..MAX_ENTRIES + 5).map(|i| format!("f{i}")).collect();
    let tar = {
        let mut builder = tar::Builder::new(Vec::new());
        for name in &names {
            let mut header = tar::Header::new_gnu();
            header.set_size(0);
            header.set_cksum();
            builder.append_data(&mut header, name, &b""[..]).unwrap();
        }
        builder.into_inner().unwrap()
    };
    let diff = diff_sources(
        &git,
        dir.path(),
        None,
        Some(source(dir.path(), "many.tar", &tar)),
    );
    assert!(diff.truncated);
    assert_eq!(diff.entries.len(), MAX_ENTRIES);
}

#[test]
fn links_and_directories_are_kinds() {
    let dir = tempfile::tempdir().unwrap();
    let git = git();
    let mut builder = tar::Builder::new(Vec::new());
    let mut header = tar::Header::new_gnu();
    header.set_entry_type(tar::EntryType::Symlink);
    header.set_size(0);
    builder
        .append_link(&mut header, "link", "target/file")
        .unwrap();
    let mut header = tar::Header::new_gnu();
    header.set_entry_type(tar::EntryType::Directory);
    header.set_size(0);
    header.set_cksum();
    builder.append_data(&mut header, "dir/", &b""[..]).unwrap();
    let tar = builder.into_inner().unwrap();
    let diff = diff_sources(
        &git,
        dir.path(),
        None,
        Some(source(dir.path(), "l.tar", &tar)),
    );
    let link = diff.entry("link").unwrap().new.clone().unwrap();
    assert_eq!(link.kind, ArchiveEntryKind::Symlink("target/file".into()));
    assert!(diff.entry("dir").unwrap().is_dir());
    let entry = entry_diff(&git, dir.path(), &diff, "link", false).unwrap();
    assert_eq!(entry.diff, Diff::Empty);
}

#[test]
fn safe_names_for_extracted_entries() {
    assert_eq!(safe_file_name("../../etc/passwd", EntrySide::New), "passwd");
    assert_eq!(
        safe_file_name("a/b.txt", EntrySide::Old),
        "b (previous).txt"
    );
    assert_eq!(safe_file_name("a/..", EntrySide::New), "entry");
    assert_eq!(safe_file_name("x/what?.md", EntrySide::New), "what_.md");
}

fn run(path: &std::path::Path, args: &[&str]) {
    let ok = std::process::Command::new("git")
        .args(args)
        .current_dir(path)
        .env("GIT_AUTHOR_NAME", "T")
        .env("GIT_AUTHOR_EMAIL", "t@example.com")
        .env("GIT_COMMITTER_NAME", "T")
        .env("GIT_COMMITTER_EMAIL", "t@example.com")
        .status()
        .unwrap()
        .success();
    assert!(ok, "git {args:?}");
}

#[test]
fn working_and_committed_archives_diff_as_archives() {
    set_read_archives(true);
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path();
    run(path, &["init", "-q", "-b", "main"]);
    run(path, &["config", "commit.gpgsign", "false"]);
    std::fs::write(path.join("b.zip"), zip_of(&[("a.txt", b"one\n")])).unwrap();
    run(path, &["add", "."]);
    run(path, &["commit", "-q", "-m", "one"]);
    std::fs::write(path.join("b.zip"), zip_of(&[("a.txt", b"two\n")])).unwrap();

    let git = git();
    let status = crate::get_status(git.clone(), path).unwrap();
    let diff = crate::working_directory_diff(
        git.clone(),
        path,
        &status.files[0],
        false,
        false,
        false,
        None,
    )
    .unwrap();
    let Diff::Archive(archive) = &diff else {
        panic!("archive diff expected: {diff:?}");
    };
    assert_eq!(status_of(archive, "a.txt"), ArchiveEntryStatus::Modified);
    assert!(matches!(
        archive.old.as_ref().unwrap().source.root,
        ArchiveRoot::Blob { .. }
    ));
    assert!(matches!(
        archive.new.as_ref().unwrap().source.root,
        ArchiveRoot::File { .. }
    ));
    // `-w` reaches the entry's text diff
    let entry = entry_diff(&git, path, archive, "a.txt", true).unwrap();
    assert!(matches!(entry.diff, Diff::Text { .. }));

    run(path, &["commit", "-q", "-am", "two"]);
    let changes = crate::get_changed_files(git.clone(), path, "HEAD", false).unwrap();
    let diff = crate::commit_file_diff(git.clone(), path, &changes.files[0], false).unwrap();
    let Diff::Archive(archive) = &diff else {
        panic!("archive diff expected: {diff:?}");
    };
    assert_eq!(status_of(archive, "a.txt"), ArchiveEntryStatus::Modified);
    let old = entry_bytes(&git, path, archive, "a.txt", EntrySide::Old).unwrap();
    assert_eq!(&old[..], b"one\n");
}

#[test]
fn a_textconv_driver_keeps_its_text() {
    set_read_archives(true);
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path();
    run(path, &["init", "-q", "-b", "main"]);
    run(path, &["config", "commit.gpgsign", "false"]);
    run(path, &["config", "diff.listing.textconv", "wc -c"]);
    std::fs::write(path.join(".gitattributes"), "*.zip diff=listing\n").unwrap();
    std::fs::write(path.join("b.zip"), zip_of(&[("a.txt", b"one\n")])).unwrap();
    run(path, &["add", "."]);
    run(path, &["commit", "-q", "-m", "one"]);
    std::fs::write(path.join("b.zip"), zip_of(&[("a.txt", b"two two\n")])).unwrap();
    let git = git();
    let status = crate::get_status(git.clone(), path).unwrap();
    let file = status.files.iter().find(|f| f.path == "b.zip").unwrap();
    let diff = crate::working_directory_diff(git, path, file, false, false, false, None).unwrap();
    assert!(matches!(diff, Diff::Text { .. }), "{diff:?}");
}
