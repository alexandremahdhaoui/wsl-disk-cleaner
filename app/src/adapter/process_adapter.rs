use std::process::{Command, Stdio};

use anyhow::{anyhow, Context};
use wdc_core::port::{CommandRunner, PortError};

pub struct ProcessAdapter;

impl CommandRunner for ProcessAdapter {
    fn run(&self, argv: &[String]) -> Result<(), PortError> {
        let (bin, args) = argv.split_first().context("running an empty command")?;
        let out = Command::new(bin)
            .args(args)
            .stdin(Stdio::null())
            .output()
            .with_context(|| format!("starting {bin}"))?;
        if !out.status.success() {
            return Err(anyhow!(
                "{bin} exited with {}: {}",
                out.status,
                String::from_utf8_lossy(&out.stderr).trim()
            )
            .into());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(parts: &[&str]) -> Vec<String> {
        parts.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn a_successful_command_returns_ok() {
        assert!(ProcessAdapter.run(&argv(&["true"])).is_ok());
    }

    #[test]
    fn a_failing_command_reports_its_exit_status_and_stderr() {
        let err = ProcessAdapter
            .run(&argv(&["sh", "-c", "echo boom >&2; exit 3"]))
            .unwrap_err();
        assert_eq!(err.to_string(), "sh exited with exit status: 3: boom");
    }

    #[test]
    fn an_empty_command_is_refused() {
        assert!(ProcessAdapter.run(&[]).is_err());
    }
}
