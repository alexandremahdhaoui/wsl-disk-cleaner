use std::path::{Path, PathBuf};

use super::PortError;

#[cfg_attr(test, mockall::automock)]
pub trait PathFacts: Send + Sync {
    fn canonical(&self, path: &Path) -> Result<PathBuf, PortError>;
    fn device(&self, path: &Path) -> Result<u64, PortError>;
    fn exists(&self, path: &Path) -> bool;
    fn tracked_files(&self, path: &Path) -> Result<usize, PortError>;
    fn size(&self, path: &Path) -> Result<u64, PortError>;
    fn root_used_bytes(&self) -> Result<u64, PortError>;
}
