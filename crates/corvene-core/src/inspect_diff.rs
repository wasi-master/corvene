//! Corvene `1340-tensor-diff` / `1341-signing-material-diff`: the work the
//! tensor and signing material views ask for after the diff is shown
//! (comparing tensor values, unlocking a PKCS#12 bundle). The diffs are
//! read in `corvene_git::inspect`.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use corvene_models::tensors::TensorValues;
use corvene_models::{SigningFile, TensorFile};

use crate::dispatcher::Dispatcher;
use crate::host::Host;

/// Bytes read per side by Compare Values.
pub const TENSOR_VALUES_MAX_BYTES: u64 = 2 << 30;

/// What comparing tensor values needs from a repository; run on a
/// background thread.
pub struct TensorValuesJob {
    git: Arc<corvene_git::GitBinary>,
    workdir: PathBuf,
}

impl TensorValuesJob {
    /// The values of the tensors `names` (all when empty) of both versions
    /// compared; stops early once `cancel` is set.
    pub fn run(
        &self,
        previous: Option<&TensorFile>,
        current: Option<&TensorFile>,
        names: &[String],
        cancel: &AtomicBool,
    ) -> Result<Vec<TensorValues>, String> {
        corvene_git::inspect::tensor_values(
            &self.git,
            &self.workdir,
            previous,
            current,
            names,
            TENSOR_VALUES_MAX_BYTES,
            cancel,
        )
    }
}

impl Dispatcher {
    /// Corvene `1340-tensor-diff`: the job reading tensor values of the
    /// repository `repo`.
    pub fn tensor_values_job(repo: u64, cx: &dyn Host) -> Option<TensorValuesJob> {
        let (git, workdir) = Self::repo_context(repo, cx)?;
        Some(TensorValuesJob { git, workdir })
    }
}

/// Corvene `1341-signing-material-diff`: `file` (at `path`) read again with
/// `password`; `None` when it still cannot be read. Slow (key derivation):
/// call it off the main thread. The password is not kept.
pub fn unlock_signing_file(path: &str, file: &SigningFile, password: &str) -> Option<SigningFile> {
    corvene_git::inspect::unlock_signing_file(path, file, password)
}
