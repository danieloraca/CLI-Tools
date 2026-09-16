mod apply;
mod cleanup;
mod fixture;
mod verify;

use anyhow::{Context, Result};
use clap::{Args, Subcommand};
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::PathBuf;

#[derive(Debug, Subcommand)]
pub enum ScenarioCommand {
    /// Generate a fixture offline. A quoted request can describe the supported edge cases.
    Generate(GenerateArgs),
    /// Apply a fixture to an explicitly selected development profile, recording progress.
    Apply(apply::ApplyArgs),
    /// Verify recorded resources and values without changing Gecko.
    Verify(apply::ApplyArgs),
    /// Preview or remove recorded owned resources; retain shared/unverifiable resources.
    Cleanup(cleanup::CleanupArgs),
}

#[derive(Debug, Args)]
pub struct GenerateArgs {
    /// E.g. "create a profile with 500 contacts, duplicate emails, custom fields, and restricted permissions".
    pub request: Option<String>,
    /// Scenario name, using lowercase letters, digits and hyphens (up to 24 characters).
    #[arg(long, default_value = "edge-cases")]
    pub name: String,
    /// Fixed random seed. Same options and seed produce identical fixture bytes.
    #[arg(long, default_value_t = 42)]
    pub seed: u64,
    /// Number of contacts (1–10000). Overrides the quoted request.
    #[arg(long)]
    pub contacts: Option<usize>,
    /// Number of extra contacts that reuse an email. Overrides the quoted request.
    #[arg(long)]
    pub duplicate_emails: Option<usize>,
    /// Number of contacts without an email. Overrides the quoted request.
    #[arg(long)]
    pub missing_emails: Option<usize>,
    /// Add a field as KEY:TYPE; types: text, number, textarea. Repeat for multiple fields.
    #[arg(long)]
    pub custom_field: Vec<String>,
    /// Include course, cohort and access_notes custom fields.
    #[arg(long)]
    pub custom_fields: bool,
    /// Create a separate group with contacts_view permission (plus Gecko's mandatory permissions).
    #[arg(long)]
    pub restricted_permissions: bool,
    /// Write JSON to a new file. Omit to print JSON to stdout.
    #[arg(short, long)]
    pub output: Option<PathBuf>,
}

pub fn run(command: ScenarioCommand) -> Result<()> {
    match command {
        ScenarioCommand::Generate(args) => {
            let scenario = fixture::generate(&args)?;
            let mut data = serde_json::to_vec_pretty(&scenario)?;
            data.push(b'\n');
            if let Some(path) = args.output {
                let mut file = OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(&path)
                    .with_context(|| {
                        format!("cannot create {}; choose a new output file", path.display())
                    })?;
                file.write_all(&data)?;
                eprintln!(
                    "Generated {} contacts, {} custom fields and {} groups in {} (seed {}).",
                    scenario.contacts.len(),
                    scenario.fields.len() - 2,
                    scenario.groups.len(),
                    path.display(),
                    scenario.seed
                );
            } else {
                io::stdout().lock().write_all(&data)?;
            }
            Ok(())
        }
        ScenarioCommand::Cleanup(args) => {
            let scenario: fixture::Scenario = serde_json::from_slice(&fs::read(&args.run.fixture)?)
                .context("invalid scenario fixture")?;
            cleanup::run(&args, &scenario)
        }
        ScenarioCommand::Verify(args) => {
            let scenario: fixture::Scenario = serde_json::from_slice(&fs::read(&args.fixture)?)
                .context("invalid scenario fixture")?;
            verify::run(&args, &scenario)
        }
        ScenarioCommand::Apply(args) => {
            let data = fs::read(&args.fixture)
                .with_context(|| format!("cannot read {}", args.fixture.display()))?;
            let scenario: fixture::Scenario =
                serde_json::from_slice(&data).context("invalid scenario fixture")?;
            apply::run(&args, scenario)
        }
    }
}

pub(crate) use apply::{lock_state, recorded_contacts};
