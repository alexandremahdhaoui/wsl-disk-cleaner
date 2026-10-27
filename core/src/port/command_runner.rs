use super::PortError;

#[cfg_attr(test, mockall::automock)]
pub trait CommandRunner: Send + Sync {
    fn run(&self, argv: &[String]) -> Result<(), PortError>;
}
