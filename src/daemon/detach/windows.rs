//! Windows stub for [`super::detach`].
//!
//! Real implementation will use `DETACHED_PROCESS` to relaunch
//! `vik run` without `-d`.

use std::path::Path;

use super::DetachError;

pub fn detach(_log_dir: &Path) -> Result<(), DetachError> {
  tracing::error!("vik run -d is not supported on Windows yet");
  Err(DetachError::PlatformUnsupported)
}
