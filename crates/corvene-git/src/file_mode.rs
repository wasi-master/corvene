//! Corvene (`1119-file-mode-probe`, desktop#9443): `core.fileMode` for a
//! repository on a filesystem that keeps no executable bit (vfat and
//! exFAT, CIFS/SMB shares, 9p and other mounts that give every file the
//! same mode).
//!
//! git decides `core.fileMode` once, when `git init` or `git clone`
//! creates the repository (`create_default_files` in git's `setup.c`): it
//! flips the owner's executable bit of `.git/config`, and the filesystem
//! keeps file modes only when the change shows in a fresh `lstat` and can
//! be undone; a config file that comes out executable counts as no
//! (everything is executable there). A repository made elsewhere and then
//! copied or mounted onto such a filesystem keeps `core.fileMode=true`, so
//! every file whose mode on disk differs from the index shows as changed:
//! on a share that reports 0755 for everything, each ordinary file goes
//! from 100644 to 100755 with "No content changes found" (GHD 3.6.6
//! `app/src/lib/git/status.ts` lists them, as `git status` does).
//!
//! [`probe`] is git's test on the repository's config file, and
//! [`distrust_unkept_file_modes`] sets `core.fileMode=false` in the
//! repository's own config when the test fails and git currently trusts
//! file modes, as re-running `git init` there would. It never sets it back
//! to true: a false someone set on purpose (WSL, a shared drive) stays.

use std::path::Path;
use std::sync::Arc;

use crate::detect::GitBinary;
use crate::error::Result;

/// Whether the filesystem holding `config` keeps an executable bit, by
/// git's test (`create_default_files`): flip the owner's executable bit,
/// see that `lstat` reports the change, put it back. An executable config
/// file means no. `None` when `config` cannot be read or the test cannot
/// run (not a Unix filesystem API).
#[cfg(unix)]
pub fn probe(config: &Path) -> Option<bool> {
    use std::os::unix::fs::PermissionsExt;

    let before = std::fs::symlink_metadata(config).ok()?;
    if !before.file_type().is_file() {
        return None;
    }
    let mode = before.permissions().mode();
    let flipped = std::fs::set_permissions(config, std::fs::Permissions::from_mode(mode ^ 0o100));
    let kept = flipped.is_ok()
        && std::fs::symlink_metadata(config)
            .is_ok_and(|after| after.permissions().mode() & 0o7777 != mode & 0o7777);
    // put it back whatever happened (git: `!chmod(path, st1.st_mode)`)
    let restored = std::fs::set_permissions(config, std::fs::Permissions::from_mode(mode)).is_ok();
    Some(kept && restored && mode & 0o100 == 0)
}

#[cfg(not(unix))]
pub fn probe(_config: &Path) -> Option<bool> {
    None
}

/// `core.fileMode` as git reads it in the repository at `workdir` (every
/// scope; git's default is true), and the config file [`probe`] tests:
/// the common git directory's `config`.
fn current(workdir: &Path) -> Option<(bool, std::path::PathBuf)> {
    let repo = crate::handle::open(workdir).ok()?;
    let trusted = repo
        .config_snapshot()
        .boolean("core.fileMode")
        .unwrap_or(true);
    Some((trusted, repo.common_dir().join("config")))
}

/// Set `core.fileMode=false` in the repository at `workdir` when git trusts
/// file modes there but its filesystem keeps none ([`probe`]). `Ok(true)`
/// when the setting was changed.
pub fn distrust_unkept_file_modes(git: Arc<GitBinary>, workdir: &Path) -> Result<bool> {
    let Some((true, config)) = current(workdir) else {
        return Ok(false);
    };
    if probe(&config) != Some(false) {
        return Ok(false);
    }
    crate::config::set_local_config_value(git, workdir, "core.fileMode", "false")?;
    tracing::info!(
        workdir = %workdir.display(),
        "the filesystem keeps no executable bit: core.fileMode set to false"
    );
    Ok(true)
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn a_filesystem_that_keeps_modes_passes_and_the_mode_is_put_back() {
        let dir = tempfile::tempdir().unwrap();
        let config = dir.path().join("config");
        std::fs::write(&config, "[core]\n").unwrap();
        std::fs::set_permissions(&config, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert_eq!(probe(&config), Some(true));
        let mode = std::fs::metadata(&config).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o644);
    }

    #[test]
    fn an_executable_config_counts_as_no_executable_bit() {
        let dir = tempfile::tempdir().unwrap();
        let config = dir.path().join("config");
        std::fs::write(&config, "[core]\n").unwrap();
        std::fs::set_permissions(&config, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(probe(&config), Some(false));
        let mode = std::fs::metadata(&config).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o755);
    }

    #[test]
    fn a_missing_config_is_unknown() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(probe(&dir.path().join("config")), None);
    }

    fn init(dir: &Path) {
        let status = std::process::Command::new("git")
            .args(["init", "-q"])
            .current_dir(dir)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .status()
            .unwrap();
        assert!(status.success());
    }

    fn file_mode(git: Arc<GitBinary>, dir: &Path) -> Option<bool> {
        crate::config::boolean_config_value(git, dir, "core.fileMode", true)
    }

    #[test]
    fn a_repository_whose_modes_are_kept_is_left_alone() {
        let git = Arc::new(crate::find_git().unwrap());
        let dir = tempfile::tempdir().unwrap();
        init(dir.path());
        assert_eq!(file_mode(git.clone(), dir.path()), Some(true));
        assert!(!distrust_unkept_file_modes(git.clone(), dir.path()).unwrap());
        assert_eq!(file_mode(git, dir.path()), Some(true));
    }

    #[test]
    fn a_repository_whose_modes_are_not_kept_stops_trusting_them() {
        let git = Arc::new(crate::find_git().unwrap());
        let dir = tempfile::tempdir().unwrap();
        init(dir.path());
        // what a filesystem that makes everything executable shows
        let config = dir.path().join(".git/config");
        std::fs::set_permissions(&config, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(distrust_unkept_file_modes(git.clone(), dir.path()).unwrap());
        assert_eq!(file_mode(git.clone(), dir.path()), Some(false));
        // once false, nothing more to do
        assert!(!distrust_unkept_file_modes(git, dir.path()).unwrap());
    }

    /// A real drive without executable bits: `CORVENE_FILE_MODE_TEST_REPO`
    /// names a repository with committed files, copied onto one (vfat
    /// through fusefat, a share that reports 0755 for everything), whose
    /// `core.fileMode` is still true (`packaging/linux/file-mode-test.sh`
    /// makes them). Files show as changed until the probe turns
    /// `core.fileMode` off.
    #[test]
    fn a_drive_without_executable_bits_loses_its_mode_only_changes() {
        let Some(repo) = std::env::var_os("CORVENE_FILE_MODE_TEST_REPO") else {
            return;
        };
        let repo = std::path::PathBuf::from(repo);
        let git = Arc::new(crate::find_git().unwrap());
        let status = |in_process| {
            let options = crate::status::StatusOptions {
                in_process,
                ..Default::default()
            };
            crate::status::get_status_with(git.clone(), &repo, options)
                .unwrap()
                .files
                .len()
        };
        let (cli, gix) = (status(false), status(true));
        eprintln!("before: {cli} changed files (git), {gix} (gitoxide)");
        assert!(cli > 0 && gix > 0, "the copy shows no mode changes");
        assert_eq!(probe(&repo.join(".git/config")), Some(false));
        // fusefat refuses every chmod, even git's own on `config.lock`, so
        // nothing can write the config there (the kernel's vfat allows it)
        if std::env::var_os("CORVENE_FILE_MODE_TEST_DETECT_ONLY").is_some_and(|v| !v.is_empty()) {
            return;
        }
        assert!(distrust_unkept_file_modes(git.clone(), &repo).unwrap());
        let (cli, gix) = (status(false), status(true));
        eprintln!("after: {cli} changed files (git), {gix} (gitoxide)");
        assert_eq!((cli, gix), (0, 0));
    }
}
