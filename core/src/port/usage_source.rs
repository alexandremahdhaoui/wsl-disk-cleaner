use super::PortError;
use crate::types::VolumeUsage;

#[cfg_attr(test, mockall::automock)]
pub trait UsageSource: Send + Sync {
    fn name(&self) -> String;
    fn usage(&self) -> Result<VolumeUsage, PortError>;
}
