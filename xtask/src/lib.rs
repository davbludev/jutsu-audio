use std::env;
use std::ffi::OsString;
use std::process::Command;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct QualityStep {
    pub cargo_subcommand: String,
    pub arguments: Vec<String>,
}

impl QualityStep {
    #[must_use]
    pub fn new(cargo_subcommand: &str, arguments: &[&str]) -> Self {
        Self {
            cargo_subcommand: cargo_subcommand.into(),
            arguments: arguments
                .iter()
                .map(|argument| (*argument).into())
                .collect(),
        }
    }
}

#[must_use]
pub fn quality_steps() -> Vec<QualityStep> {
    vec![
        QualityStep::new("fmt", &["--all", "--", "--check"]),
        QualityStep::new(
            "clippy",
            &[
                "--workspace",
                "--all-targets",
                "--all-features",
                "--",
                "-D",
                "warnings",
            ],
        ),
        QualityStep::new("test", &["--workspace", "--all-targets", "--all-features"]),
        QualityStep::new("check", &["--workspace", "--benches", "--all-features"]),
    ]
}

pub fn run_quality() -> Result<(), String> {
    let cargo = env::var_os("CARGO").unwrap_or_else(|| OsString::from("cargo"));
    for step in quality_steps() {
        eprintln!(
            "> cargo {} {}",
            step.cargo_subcommand,
            step.arguments.join(" ")
        );
        let status = Command::new(&cargo)
            .arg(&step.cargo_subcommand)
            .args(&step.arguments)
            .status()
            .map_err(|error| format!("failed to run cargo {}: {error}", step.cargo_subcommand))?;
        if !status.success() {
            return Err(format!(
                "cargo {} failed with {status}",
                step.cargo_subcommand
            ));
        }
    }
    Ok(())
}
