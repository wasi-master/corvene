//! Port of GitHub Desktop's `app/test/unit/git/diff-test.ts`.
//!
//! Corvene equivalents of GitHub Desktop's `lib/git/diff.ts`:
//!
//! - `getWorkingDirectoryDiff(repository, file)` is
//!   `corvene_git::working_directory_diff(git, path, file, false, false,
//!   false, None)` ([`get_working_directory_diff`]): no hidden whitespace
//!   (GitHub Desktop's default), `renamed_against_head` off (the
//!   `github-desktop` value of flag `743-renamed-diff-against-head`),
//!   `as_text` off (`749-binary-diff-as-text` only turns it on when asked
//!   for one file) and no cancel token (`764-cancel-stale-diffs`).
//!   A Text diff is read through [`get_text_diff`]; GitHub Desktop has no
//!   empty diff kind (a diff without hunks is a Text diff with no hunks),
//!   so Corvene's `Diff::Empty` is that.
//! - `getWorkingDirectoryImage(repository, file)` has no function of its
//!   own: `working_directory_diff` reads the working copy into the
//!   `current` side of `Diff::Image` (`image_diff`), which
//!   [`get_working_directory_image`] returns.
//! - `getBlobImage(repository, path, commitish)` is `getBlobContents` plus
//!   `getMediaType`, which are `corvene_git::blob_bytes` and
//!   `corvene_models::image_media_type` ([`get_blob_image`]).
//! - GitHub Desktop's `Image.contents` is the base64 of the bytes; Corvene's
//!   `ImageBlob` keeps the bytes, so the tests encode them ([`base64`]).
//! - `new WorkingDirectoryFileChange(path, { kind }, selection)` is
//!   [`working_directory_file_change`]: GitHub Desktop's file change has
//!   only a path, a kind and a selection, while Corvene's also carries the
//!   index / working tree columns `working_directory_diff` reads, which come
//!   from Corvene's own status of that path.
//! - `getBinaryPaths(repository, ref, conflictedFilesInIndex)` is
//!   `corvene_git::binary_paths(git, path, ref, conflicted_paths)`, given
//!   the paths of the status entries ([`get_binary_paths`]).
//! - `getBranchMergeBaseChangedFiles` is `corvene_git::merge_base_changed_files`
//!   and `getBranchMergeBaseDiff` is `corvene_git::merge_base_file_diff`.
//! - GitHub Desktop's `DiffLine.text` is the line as the unified diff prints
//!   it, marker included (`+foo`); Corvene's `DiffLine.text` is GitHub
//!   Desktop's `DiffLine.content` (the text after the marker) and the marker
//!   is its `kind`. A hunk header line keeps its whole text in both.
//!   [`ghd_text`] puts the marker back.
//! - GitHub Desktop's `ITextDiff.text` (`IRawDiff.contents`) is every hunk
//!   line's `text` joined with `\n`, without the `\ No newline at end of
//!   file` markers (`ui/diff/text-diff-expansion.ts` `getDiffTextFromHunks`
//!   builds it the same way). Corvene keeps only the hunks; [`diff_text`]
//!   joins them so.
//! - The `git(['merge', …], { expectedErrors })` setup step is
//!   [`git_expecting`]: exit code 0 returns, otherwise dugite's `parseError`
//!   (`corvene_git::known_git_error`) of stderr, then stdout, must be one of
//!   the expected errors.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use corvene_git::{
    GitError, KnownGitError, blob_bytes, known_git_error, merge_base_changed_files,
    merge_base_file_diff,
};
use corvene_models::{
    CommittedFileChange, Diff, DiffHunk, DiffSelection, DiffWarnings, FileStatus, FileStatusKind,
    GitStatusEntry, ImageBlob, SubmoduleDiff, WorkingDirectoryFileChange, image_media_type,
};
use corvene_test_support::{
    TestRepo, Tree, TreeEntry, append_file, base64, exec, exec_ok, get_status_or_throw,
    get_working_directory_diff, ghd_text, git, make_commit, setup_empty_repository,
    setup_fixture_repository, switch_to,
};

/// GitHub Desktop's `ITextDiff.text` of these hunks.
fn diff_text(hunks: &[DiffHunk]) -> String {
    hunks
        .iter()
        .flat_map(|hunk| hunk.lines.iter())
        .map(ghd_text)
        .collect::<Vec<_>>()
        .join("\n")
}

/// GitHub Desktop's `git(args, path, name, { expectedErrors })`
/// (`lib/git/core.ts`) for a setup step whose result is not read.
fn git_expecting(args: &[&str], path: &Path, expected_errors: &[KnownGitError]) {
    let result = exec(args, path);
    if result.exit_code == 0 {
        return;
    }
    let git_error = known_git_error(&result.stderr).or_else(|| known_git_error(&result.stdout));
    match git_error {
        Some(error) if expected_errors.contains(&error) => {}
        _ => panic!(
            "`git {}` exited with an unexpected code: {}.\n{}\n(The error was parsed as {git_error:?})",
            args.join(" "),
            result.exit_code,
            result.stderr
        ),
    }
}

/// GitHub Desktop's `ITextDiff` as these tests read it.
struct TextDiff {
    hunks: Vec<DiffHunk>,
    warnings: DiffWarnings,
}

/// The file's helper `getTextDiff(repo, file)`: the working directory diff,
/// which must be a Text diff.
fn get_text_diff(repo: &TestRepo, file: &WorkingDirectoryFileChange) -> TextDiff {
    match get_working_directory_diff(repo, file) {
        Diff::Text { hunks, warnings } => TextDiff { hunks, warnings },
        // GitHub Desktop: a Text diff without hunks
        Diff::Empty => TextDiff {
            hunks: Vec::new(),
            warnings: DiffWarnings::default(),
        },
        other => panic!(
            "assert.equal(diff.kind, DiffType.Text): got {}",
            kind_name(&other)
        ),
    }
}

/// The `DiffType` name of a diff, for failure messages (an image diff's
/// `Debug` is megabytes of bytes).
fn kind_name(diff: &Diff) -> &'static str {
    match diff {
        Diff::Text { .. } => "Text",
        Diff::LargeText { .. } => "LargeText",
        Diff::Binary => "Binary",
        Diff::Image { .. } => "Image",
        Diff::Table { .. } => "Table",
        Diff::Empty => "Empty",
        Diff::TooLarge => "Unrenderable",
        Diff::Submodule(_) => "Submodule",
    }
}

/// `new WorkingDirectoryFileChange(path, { kind }, DiffSelection.fromInitialSelection(DiffSelectionType.All))`.
fn working_directory_file_change(
    repo: &TestRepo,
    path: &str,
    kind: FileStatusKind,
) -> WorkingDirectoryFileChange {
    let mut file = get_status_or_throw(repo)
        .files
        .into_iter()
        .find(|f| f.path == path)
        .unwrap_or_else(|| panic!("{path} has no changes in {}", repo.path().display()));
    file.status.kind = kind;
    file.selection = DiffSelection::all();
    file
}

/// GitHub Desktop's `getWorkingDirectoryImage(repository, file)`.
fn get_working_directory_image(repo: &TestRepo, file: &WorkingDirectoryFileChange) -> ImageBlob {
    match get_working_directory_diff(repo, file) {
        Diff::Image {
            current: Some(current),
            ..
        } => current,
        Diff::Image { current: None, .. } => {
            panic!("getWorkingDirectoryImage({}): no current image", file.path)
        }
        other => panic!(
            "getWorkingDirectoryImage({}): the diff is {}, not an image",
            file.path,
            kind_name(&other)
        ),
    }
}

/// GitHub Desktop's `getBlobImage(repository, path, commitish)`.
fn get_blob_image(repo: &TestRepo, path: &str, commitish: &str) -> ImageBlob {
    let bytes = blob_bytes(git(), repo.path(), commitish, path)
        .unwrap_or_else(|err| panic!("getBlobContents({commitish}:{path}): {err}"));
    ImageBlob {
        bytes,
        media_type: image_media_type(path).unwrap_or_default().to_string(),
        original: None,
    }
}

/// GitHub Desktop's `IStatusEntry` (`lib/status-parser.ts`), as
/// `getBinaryPaths` takes the conflicted files.
#[allow(dead_code)] // `getBinaryPaths` reads only the path
struct StatusEntry {
    path: &'static str,
    status_code: &'static str,
    submodule_status_code: &'static str,
}

/// GitHub Desktop's `getBinaryPaths(repository, ref,
/// conflictedFilesInIndex)` (`lib/git/diff.ts`):
/// `corvene_git::binary_paths` with the entries' paths.
fn get_binary_paths(
    repository: &TestRepo,
    reference: &str,
    conflicted_files_in_index: &[StatusEntry],
) -> Result<Vec<String>, GitError> {
    let paths: Vec<String> = conflicted_files_in_index
        .iter()
        .map(|entry| entry.path.to_string())
        .collect();
    corvene_git::binary_paths(git(), repository.path(), reference, &paths)
}

// GHD: unit/git/diff-test.ts › git/diff › getWorkingDirectoryImage › retrieves valid image for new file
#[test]
fn retrieves_valid_image_for_new_file() {
    let repository = setup_fixture_repository("repo-with-image-changes");
    let file = working_directory_file_change(&repository, "new-image.png", FileStatusKind::New);
    let current = get_working_directory_image(&repository, &file);

    assert_eq!(current.media_type, "image/png");
    assert!(base64(&current.bytes).ends_with("A2HkbLsBYSgAAAABJRU5ErkJggg=="));
}

// GHD: unit/git/diff-test.ts › git/diff › getWorkingDirectoryImage › retrieves valid images for modified file
#[test]
fn retrieves_valid_images_for_modified_file() {
    let repository = setup_fixture_repository("repo-with-image-changes");
    let file =
        working_directory_file_change(&repository, "modified-image.jpg", FileStatusKind::Modified);
    let current = get_working_directory_image(&repository, &file);
    assert_eq!(current.media_type, "image/jpg");
    assert!(base64(&current.bytes).ends_with("gdTTb6MClWJ3BU8T8PTtXoB88kFL/9k="));
}

// GHD: unit/git/diff-test.ts › git/diff › getBlobImage › retrieves valid image for modified file
#[test]
fn get_blob_image_retrieves_valid_image_for_modified_file() {
    let repository = setup_fixture_repository("repo-with-image-changes");
    let file =
        working_directory_file_change(&repository, "modified-image.jpg", FileStatusKind::Modified);
    let current = get_blob_image(&repository, &file.path, "HEAD");

    assert_eq!(current.media_type, "image/jpg");
    assert!(base64(&current.bytes).contains("zcabBFNf6G8U1y7QpBYtbOWQivIsDU8T4kYKKTQFg7v/9k="));
}

// GHD: unit/git/diff-test.ts › git/diff › getBlobImage › retrieves valid images for deleted file
#[test]
fn retrieves_valid_images_for_deleted_file() {
    let repository = setup_fixture_repository("repo-with-image-changes");
    let file = working_directory_file_change(
        &repository,
        "new-animated-image.gif",
        FileStatusKind::Deleted,
    );
    let previous = get_blob_image(&repository, &file.path, "HEAD");

    assert_eq!(previous.media_type, "image/gif");
    assert!(
        base64(&previous.bytes).ends_with("pSQ0J85QG55rqWbgLdEmOWQJ1MjFS3WWA2slfZxeEAtp3AykkAAA7")
    );
}

// GHD: unit/git/diff-test.ts › git/diff › imageDiff › changes for images are set
#[test]
fn changes_for_images_are_set() {
    let repository = setup_fixture_repository("repo-with-image-changes");
    let file =
        working_directory_file_change(&repository, "modified-image.jpg", FileStatusKind::Modified);
    let diff = get_working_directory_diff(&repository, &file);

    let Diff::Image { previous, current } = diff else {
        panic!(
            "assert.equal(diff.kind, DiffType.Image): got {}",
            kind_name(&diff)
        )
    };
    assert!(previous.is_some());
    assert!(current.is_some());
}

// GHD: unit/git/diff-test.ts › git/diff › imageDiff › changes for text are not set
#[test]
fn changes_for_text_are_not_set() {
    let repository = setup_fixture_repository("repo-with-changes");

    let file = working_directory_file_change(&repository, "new-file.md", FileStatusKind::New);
    let diff = get_text_diff(&repository, &file);

    assert!(!diff.hunks.is_empty());
}

// GHD: unit/git/diff-test.ts › git/diff › getWorkingDirectoryDiff › counts lines for new file
#[test]
fn counts_lines_for_new_file() {
    let repository = setup_fixture_repository("repo-with-changes");

    let file = working_directory_file_change(&repository, "new-file.md", FileStatusKind::New);
    let diff = get_text_diff(&repository, &file);

    let hunk = &diff.hunks[0];

    assert!(ghd_text(&hunk.lines[0]).contains("@@ -0,0 +1,33 @@"));

    assert!(ghd_text(&hunk.lines[1]).contains("+Lorem ipsum dolor sit amet,"));
    assert!(ghd_text(&hunk.lines[2]).contains("+ullamcorper sit amet tellus eget, "));

    assert!(ghd_text(&hunk.lines[33]).contains("+ urna, ac porta justo leo sed magna."));
}

// GHD: unit/git/diff-test.ts › git/diff › getWorkingDirectoryDiff › counts lines for modified file
#[test]
fn counts_lines_for_modified_file() {
    let repository = setup_fixture_repository("repo-with-changes");

    let file =
        working_directory_file_change(&repository, "modified-file.md", FileStatusKind::Modified);
    let diff = get_text_diff(&repository, &file);

    let first = &diff.hunks[0];
    assert!(ghd_text(&first.lines[0]).contains("@@ -4,10 +4,6 @@"));

    assert!(ghd_text(&first.lines[4]).contains("-Aliquam leo ipsum"));
    assert!(ghd_text(&first.lines[5]).contains("-nisl eget hendrerit"));
    assert!(ghd_text(&first.lines[6]).contains("-eleifend mi."));
    assert!(ghd_text(&first.lines[7]).contains('-'));

    let second = &diff.hunks[1];
    assert!(ghd_text(&second.lines[0]).contains("@@ -21,6 +17,10 @@"));

    assert!(ghd_text(&second.lines[4]).contains("+Aliquam leo ipsum"));
    assert!(ghd_text(&second.lines[5]).contains("+nisl eget hendrerit"));
    assert!(ghd_text(&second.lines[6]).contains("+eleifend mi."));
    assert!(ghd_text(&second.lines[7]).contains('+'));
}

// GHD: unit/git/diff-test.ts › git/diff › getWorkingDirectoryDiff › counts lines for staged file
#[test]
fn counts_lines_for_staged_file() {
    let repository = setup_fixture_repository("repo-with-changes");

    let file =
        working_directory_file_change(&repository, "staged-file.md", FileStatusKind::Modified);
    let diff = get_text_diff(&repository, &file);

    let first = &diff.hunks[0];
    assert!(ghd_text(&first.lines[0]).contains("@@ -2,7 +2,7 @@ "));

    assert!(ghd_text(&first.lines[4]).contains(
        "-tortor placerat facilisis. Ut sed ex tortor. Duis consectetur at ex vel mattis."
    ));
    assert!(ghd_text(&first.lines[5]).contains("+tortor placerat facilisis."));

    let second = &diff.hunks[1];
    assert!(ghd_text(&second.lines[0]).contains("@@ -17,9 +17,7 @@ "));

    assert!(ghd_text(&second.lines[4]).contains("-vel sagittis nisl rutrum. "));
    assert!(ghd_text(&second.lines[5]).contains("-tempor a ligula. Proin pretium ipsum "));
    assert!(ghd_text(&second.lines[6]).contains("-elementum neque id tellus gravida rhoncus."));
    assert!(ghd_text(&second.lines[7]).contains("+vel sagittis nisl rutrum."));
}

// GHD: unit/git/diff-test.ts › git/diff › getWorkingDirectoryDiff › displays a binary diff for a docx file
#[test]
fn displays_a_binary_diff_for_a_docx_file() {
    let repo = setup_fixture_repository("diff-rendering-docx");

    let status = get_status_or_throw(&repo);
    let files = status.files;

    assert_eq!(files.len(), 1);

    let diff = get_working_directory_diff(&repo, &files[0]);

    assert_eq!(kind_name(&diff), "Binary");
}

// GHD: unit/git/diff-test.ts › git/diff › getWorkingDirectoryDiff › is empty for a renamed file
#[test]
fn is_empty_for_a_renamed_file() {
    let repo = setup_empty_repository();

    std::fs::write(repo.join("foo"), "foo\n").unwrap();

    exec(["add", "foo"], repo.path());
    exec(["commit", "-m", "Initial commit"], repo.path());
    exec(["mv", "foo", "bar"], repo.path());

    let status = get_status_or_throw(&repo);
    let files = status.files;

    assert_eq!(files.len(), 1);

    let diff = get_text_diff(&repo, &files[0]);

    assert_eq!(diff.hunks.len(), 0);
}

// A renamed file in the working directory is just two staged files
// with high similarity. If we don't take the rename into account
// when generating the diffs we'd be looking at a diff with only
// additions.
// GHD: unit/git/diff-test.ts › git/diff › getWorkingDirectoryDiff › only shows modifications after move for a renamed and modified file
#[test]
fn only_shows_modifications_after_move_for_a_renamed_and_modified_file() {
    let repo = setup_empty_repository();

    std::fs::write(repo.join("foo"), "foo\n").unwrap();

    exec(["add", "foo"], repo.path());
    exec(["commit", "-m", "Initial commit"], repo.path());
    exec(["mv", "foo", "bar"], repo.path());

    std::fs::write(repo.join("bar"), "bar\n").unwrap();

    let status = get_status_or_throw(&repo);
    let files = status.files;

    assert_eq!(files.len(), 1);

    let diff = get_text_diff(&repo, &files[0]);

    assert_eq!(diff.hunks.len(), 1);

    let first = &diff.hunks[0];
    assert_eq!(first.lines.len(), 3);
    assert_eq!(ghd_text(&first.lines[1]), "-foo");
    assert_eq!(ghd_text(&first.lines[2]), "+bar");
}

// GHD: unit/git/diff-test.ts › git/diff › getWorkingDirectoryDiff › handles unborn repository with mixed state
#[test]
fn handles_unborn_repository_with_mixed_state() {
    let repo = setup_empty_repository();

    std::fs::write(repo.join("foo"), "WRITING THE FIRST LINE\n").unwrap();

    exec(["add", "foo"], repo.path());

    std::fs::write(repo.join("foo"), "WRITING OVER THE TOP\n").unwrap();

    let status = get_status_or_throw(&repo);
    let files = status.files;

    assert_eq!(files.len(), 1);

    let diff = get_text_diff(&repo, &files[0]);

    assert_eq!(diff.hunks.len(), 1);

    let first = &diff.hunks[0];
    assert_eq!(first.lines.len(), 2);
    assert_eq!(ghd_text(&first.lines[1]), "+WRITING OVER THE TOP");
}

// GHD: unit/git/diff-test.ts › git/diff › getWorkingDirectoryDiff/line-endings › displays line endings change from LF to CRLF
#[test]
fn displays_line_endings_change_from_lf_to_crlf() {
    let repo = setup_empty_repository();
    let file_path = repo.join("foo");

    let mut line_ending = "\r\n";

    std::fs::write(
        &file_path,
        format!(
            "WRITING MANY LINES {line_ending} USING THIS LINE ENDING {line_ending} TO SHOW THAT GIT{line_ending} WILL INSERT IT WITHOUT CHANGING THING {line_ending} HA HA BUSINESS"
        ),
    )
    .unwrap();

    exec(["add", "foo"], repo.path());
    exec(["commit", "-m", "commit first file with LF"], repo.path());

    // change config on-the-fly to trigger the line endings change warning
    exec(["config", "core.autocrlf", "true"], repo.path());
    line_ending = "\n\n";

    std::fs::write(
        &file_path,
        format!(
            "WRITING MANY LINES {line_ending} USING THIS LINE ENDING {line_ending} TO SHOW THAT GIT{line_ending} WILL INSERT IT WITHOUT CHANGING THING {line_ending} HA HA BUSINESS"
        ),
    )
    .unwrap();

    let status = get_status_or_throw(&repo);
    let files = status.files;

    assert_eq!(files.len(), 1);

    let diff = get_text_diff(&repo, &files[0]);

    let line_endings_change = diff.warnings.line_endings;
    assert!(line_endings_change.is_some());
    let line_endings_change = line_endings_change.unwrap();
    assert_eq!(line_endings_change.from, "LF");
    assert_eq!(line_endings_change.to, "CRLF");
}

// GHD: unit/git/diff-test.ts › git/diff › getWorkingDirectoryDiff/unicode › displays unicode characters
#[test]
fn displays_unicode_characters() {
    let repo = setup_empty_repository();
    let file_path = repo.join("foo");

    let test_string = "here are some cool characters: • é  漢字";
    std::fs::write(&file_path, test_string).unwrap();

    let status = get_status_or_throw(&repo);
    let files = status.files;
    assert_eq!(files.len(), 1);

    let diff = get_text_diff(&repo, &files[0]);
    assert_eq!(
        diff_text(&diff.hunks),
        format!("@@ -0,0 +1 @@\n+{test_string}")
    );
}

// GHD: unit/git/diff-test.ts › git/diff › getBinaryPaths › in empty repo › throws since HEAD doesnt exist
#[test]
fn throws_since_head_doesnt_exist() {
    let repo = setup_empty_repository();
    assert!(get_binary_paths(&repo, "HEAD", &[]).is_err());
}

// GHD: unit/git/diff-test.ts › git/diff › getBinaryPaths › with files using binary merge driver › includes plain text files using binary driver
#[test]
fn includes_plain_text_files_using_binary_driver() {
    let repo = setup_empty_repository();
    std::fs::write(repo.join("foo.bin"), "foo\n").unwrap();
    std::fs::write(repo.join(".gitattributes"), "*.bin merge=binary\n").unwrap();
    exec_ok(["add", "."], repo.path());
    exec_ok(["commit", "-m", "initial"], repo.path());
    exec_ok(["checkout", "-b", "branch-a"], repo.path());
    std::fs::write(repo.join("foo.bin"), "bar\n").unwrap();
    exec_ok(["commit", "-a", "-m", "second"], repo.path());
    exec_ok(["checkout", "-"], repo.path());
    std::fs::write(repo.join("foo.bin"), "foozball\n").unwrap();
    exec_ok(["commit", "-a", "-m", "third"], repo.path());
    git_expecting(
        &["merge", "branch-a"],
        repo.path(),
        &[KnownGitError::MergeConflicts],
    );

    assert_eq!(
        get_binary_paths(
            &repo,
            "MERGE_HEAD",
            &[StatusEntry {
                path: "foo.bin",
                status_code: "UU",
                submodule_status_code: "????",
            }],
        )
        .unwrap(),
        vec!["foo.bin".to_string()]
    );
}

// GHD: unit/git/diff-test.ts › git/diff › getBinaryPaths › in repo with text only files › returns an empty array
#[test]
fn returns_an_empty_array() {
    let repo = setup_fixture_repository("repo-with-changes");
    assert_eq!(get_binary_paths(&repo, "HEAD", &[]).unwrap().len(), 0);
}

// GHD: unit/git/diff-test.ts › git/diff › getBinaryPaths › in repo with image changes › returns all changed image files
#[test]
fn returns_all_changed_image_files() {
    let repo = setup_fixture_repository("repo-with-image-changes");
    assert_eq!(
        get_binary_paths(&repo, "HEAD", &[]).unwrap(),
        vec![
            "modified-image.jpg".to_string(),
            "new-animated-image.gif".to_string(),
            "new-image.png".to_string(),
        ]
    );
}

// GHD: unit/git/diff-test.ts › git/diff › getBinaryPaths › in repo with merge conflicts on image files › returns all conflicted image files
#[test]
fn returns_all_conflicted_image_files() {
    let repo = setup_fixture_repository("detect-conflict-in-binary-file");
    exec(["checkout", "make-a-change"], repo.path());
    exec(["merge", "master"], repo.path());

    assert_eq!(
        get_binary_paths(&repo, "MERGE_HEAD", &[]).unwrap(),
        vec!["my-cool-image.png".to_string()]
    );
}

/// The `with submodules` block's `getSubmodulePath(repoPath, ...components)`.
fn get_submodule_path(repo_path: &Path, components: &[&str]) -> PathBuf {
    let mut path = repo_path.join("foo").join("submodule");
    for component in components {
        path.push(component);
    }
    path
}

/// The `with submodules` block's `getSubmoduleDiff(repository)`.
fn get_submodule_diff(repository: &TestRepo) -> SubmoduleDiff {
    let status = get_status_or_throw(repository);
    let file = &status.files[0];
    let diff = get_working_directory_diff(repository, file);
    match diff {
        Diff::Submodule(diff) => diff,
        other => panic!(
            "assert.equal(diff.kind, DiffType.Submodule): got {}",
            kind_name(&other)
        ),
    }
}

// GHD: unit/git/diff-test.ts › git/diff › with submodules › can get the diff for a submodule with the right paths
#[test]
fn can_get_the_diff_for_a_submodule_with_the_right_paths() {
    let repository = setup_fixture_repository("submodule-basic-setup");
    let repo_path = repository.path();

    // Just make any change to the submodule to get a diff
    std::fs::write(get_submodule_path(repo_path, &["README.md"]), "hello\n").unwrap();

    let diff = get_submodule_diff(&repository);
    assert_eq!(diff.full_path, get_submodule_path(repo_path, &[]));
    // Even on Windows, the path separator is '/' for this specific attribute
    assert_eq!(diff.path, "foo/submodule");
}

// GHD: unit/git/diff-test.ts › git/diff › with submodules › can get the diff for a submodule with only modified changes
#[test]
fn can_get_the_diff_for_a_submodule_with_only_modified_changes() {
    let repository = setup_fixture_repository("submodule-basic-setup");
    let repo_path = repository.path();

    // Modify README.md file. Now the submodule has modified changes.
    std::fs::write(get_submodule_path(repo_path, &["README.md"]), "hello\n").unwrap();

    let diff = get_submodule_diff(&repository);
    assert!(diff.old_sha.is_none());
    assert!(diff.new_sha.is_none());
    assert!(!diff.status.commit_changed);
    assert!(diff.status.modified_changes);
    assert!(!diff.status.untracked_changes);
}

// GHD: unit/git/diff-test.ts › git/diff › with submodules › can get the diff for a submodule with only untracked changes
#[test]
fn can_get_the_diff_for_a_submodule_with_only_untracked_changes() {
    let repository = setup_fixture_repository("submodule-basic-setup");
    let repo_path = repository.path();

    // Create NEW.md file. Now the submodule has untracked changes.
    std::fs::write(get_submodule_path(repo_path, &["NEW.md"]), "hello\n").unwrap();

    let diff = get_submodule_diff(&repository);
    assert!(diff.old_sha.is_none());
    assert!(diff.new_sha.is_none());
    assert!(!diff.status.commit_changed);
    assert!(!diff.status.modified_changes);
    assert!(diff.status.untracked_changes);
}

// GHD: unit/git/diff-test.ts › git/diff › with submodules › can get the diff for a submodule a commit change
#[test]
fn can_get_the_diff_for_a_submodule_a_commit_change() {
    let repository = setup_fixture_repository("submodule-basic-setup");
    let repo_path = repository.path();

    // Make a change and commit it. Now the submodule has a commit change.
    std::fs::write(get_submodule_path(repo_path, &["README.md"]), "hello\n").unwrap();
    exec(
        ["commit", "-a", "-m", "test"],
        get_submodule_path(repo_path, &[]),
    );

    let diff = get_submodule_diff(&repository);
    assert!(diff.old_sha.is_some());
    assert!(diff.new_sha.is_some());
    assert!(diff.status.commit_changed);
    assert!(!diff.status.modified_changes);
    assert!(!diff.status.untracked_changes);
}

// GHD: unit/git/diff-test.ts › git/diff › with submodules › can get the diff for a submodule a all kinds of changes
#[test]
fn can_get_the_diff_for_a_submodule_a_all_kinds_of_changes() {
    let repository = setup_fixture_repository("submodule-basic-setup");
    let repo_path = repository.path();

    std::fs::write(get_submodule_path(repo_path, &["README.md"]), "hello\n").unwrap();
    exec(
        ["commit", "-a", "-m", "test"],
        get_submodule_path(repo_path, &[]),
    );
    std::fs::write(get_submodule_path(repo_path, &["README.md"]), "bye\n").unwrap();
    std::fs::write(get_submodule_path(repo_path, &["NEW.md"]), "new!!\n").unwrap();

    let diff = get_submodule_diff(&repository);
    assert!(diff.old_sha.is_some());
    assert!(diff.new_sha.is_some());
    assert!(diff.status.commit_changed);
    assert!(diff.status.modified_changes);
    assert!(diff.status.untracked_changes);
}

// GHD: unit/git/diff-test.ts › git/diff › getBranchMergeBaseChangedFiles › loads the files changed between two branches if merged
#[test]
fn loads_the_files_changed_between_two_branches_if_merged() {
    let repository = setup_fixture_repository("submodule-basic-setup");

    // create feature branch from initial master commit
    exec(["branch", "feature-branch"], repository.path());

    let first_commit = Tree::new([TreeEntry::new("A.md", "A")]);
    make_commit(&repository, &first_commit);

    // switch to the feature branch and add feature.md and add foo.md
    switch_to(&repository, "feature-branch");

    let second_commit = Tree::new([TreeEntry::new("feature.md", "feature")]);
    make_commit(&repository, &second_commit);

    /*
      Now, we have:

         B
      A  |  -- Feature
      |  /
      I -- Master

      If we did `git diff master feature`, we would see files changes
      from just A and B.

      We are testing `git diff --merge-base master feature`, which will
      display the diff of the resulting merge of `feature` into `master`.
      Thus, we will see changes from B only.
    */

    let changeset_data = merge_base_changed_files(
        git(),
        repository.path(),
        "master",
        "feature-branch",
        "irrelevantToTest",
    )
    .unwrap_or_else(|err| panic!("getBranchMergeBaseChangedFiles: {err}"));

    assert!(changeset_data.is_some());
    let Some(changeset_data) = changeset_data else {
        return;
    };

    assert_eq!(changeset_data.files.len(), 1);
    assert_eq!(changeset_data.files[0].path, "feature.md");
}

// GHD: unit/git/diff-test.ts › git/diff › getBranchMergeBaseChangedFiles › returns null for unrelated histories
#[test]
fn returns_null_for_unrelated_histories() {
    let repository = setup_fixture_repository("submodule-basic-setup");

    // create a second branch that's orphaned from our current branch
    exec(
        ["checkout", "--orphan", "orphaned-branch"],
        repository.path(),
    );

    // add a commit to this new branch
    exec(
        ["commit", "--allow-empty", "-m", "first commit on gh-pages"],
        repository.path(),
    );

    let changeset_data = merge_base_changed_files(
        git(),
        repository.path(),
        "master",
        "feature-branch",
        "irrelevantToTest",
    )
    .unwrap_or_else(|err| panic!("getBranchMergeBaseChangedFiles: {err}"));

    assert!(changeset_data.is_none());
}

// GHD: unit/git/diff-test.ts › git/diff › getBranchMergeBaseDiff › loads the diff of a file between two branches if merged
#[test]
fn loads_the_diff_of_a_file_between_two_branches_if_merged() {
    let repository = setup_fixture_repository("submodule-basic-setup");

    // Add foo.md to master
    let foo_path = repository.join("foo.md");
    std::fs::write(&foo_path, "foo\n").unwrap();
    exec(["commit", "-a", "-m", "foo"], repository.path());

    // Create feature branch from commit with foo.md
    exec(["branch", "feature-branch"], repository.path());

    // Commit a line "bar" to foo.md on master branch
    append_file(&foo_path, "bar\n");
    exec([OsStr::new("add"), foo_path.as_os_str()], repository.path());
    exec(["commit", "-m", "A"], repository.path());

    // switch to the feature branch and add feature to foo.md
    switch_to(&repository, "feature-branch");

    // Commit a line of "feature" to foo.md on feature branch
    append_file(&foo_path, "feature\n");
    exec([OsStr::new("add"), foo_path.as_os_str()], repository.path());
    exec(["commit", "-m", "B"], repository.path());

    /*
      Now, we have:

         B
      A  |  -- Feature
      |  /
      Foo -- Master

      A adds line of "bar" to foo.md
      B adds line "feature" to foo.md

      If we did `git diff master feature`, we would see both lines
      "bar" and "feature" added to foo.md

      We are testing `git diff --merge-base master feature`, which will
      display the diff of the resulting merge of `feature` into `master`.
      Thus, we will see changes from B only or the line "feature".
    */

    let diff = merge_base_file_diff(
        git(),
        repository.path(),
        &file_change("foo.md", FileStatusKind::New),
        "master",
        "feature-branch",
        false,
        "irrelevantToTest",
    )
    .unwrap_or_else(|err| panic!("getBranchMergeBaseDiff: {err}"));
    assert_eq!(kind_name(&diff), "Text");

    let Diff::Text { hunks, .. } = diff else {
        return;
    };

    assert!(!diff_text(&hunks).contains("bar"));
    assert!(diff_text(&hunks).contains("feature"));
}

/// `new FileChange(path, { kind })`: a file of a commit as Corvene's
/// `parse_raw_log_with_numstat` builds one for that kind (GitHub Desktop's
/// `FileChange` has no commitish).
fn file_change(path: &str, kind: FileStatusKind) -> CommittedFileChange {
    let (index, code) = match kind {
        FileStatusKind::New => (GitStatusEntry::Added, "A"),
        FileStatusKind::Deleted => (GitStatusEntry::Deleted, "D"),
        _ => (GitStatusEntry::Modified, "M"),
    };
    CommittedFileChange {
        path: path.to_string(),
        old_path: None,
        status: FileStatus {
            kind,
            index,
            working_tree: GitStatusEntry::Unchanged,
            score: None,
            code: code.to_string(),
            submodule: false,
            submodule_status: None,
            conflict_markers: None,
        },
        commitish: String::new(),
    }
}
