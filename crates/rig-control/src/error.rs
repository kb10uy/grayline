use thiserror::Error;

use crate::script::ScriptError;

/// What rig control publishes when something went wrong.
///
/// The transport's own failures arrive through the script that ran the
/// command, so this is the script's error and the one case that happens before
/// any script runs.
#[derive(Clone, Debug, Error)]
pub enum RigControlError {
    /// The script could not be read, loaded, or called.
    #[error(transparent)]
    Script(#[from] ScriptError),
    /// The worker thread could not be started.
    #[error("rig control is unavailable")]
    WorkerUnavailable,
}
