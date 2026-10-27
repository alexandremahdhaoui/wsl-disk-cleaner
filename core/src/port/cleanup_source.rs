use super::PortError;
use crate::types::Discovery;

#[cfg_attr(test, mockall::automock)]
pub trait CleanupSource: Send + Sync {
    fn name(&self) -> String;
    fn discover(&self) -> Result<Discovery, PortError>;
}
