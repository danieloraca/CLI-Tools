use crate::{auth, gecko::GeckoApi, session};
use anyhow::{Context, Result};
use clap::{Args, Subcommand};
use std::path::PathBuf;

#[derive(Debug, Clone, Args)]
pub struct Connection {
    #[arg(long, env = "CLI_TOOLS_APP_API_URL", hide_env_values = true)]
    pub app_api_base_url: String,
    #[arg(long)]
    pub app_token_file: Option<PathBuf>,
    #[arg(long)]
    pub session_file: Option<PathBuf>,
}
impl Connection {
    pub fn open(&self) -> Result<GeckoApi> {
        let tokens = auth::load_tokens(
            &self
                .app_token_file
                .clone()
                .or_else(auth::default_app_token_file)
                .context("no app token path")?,
        )?;
        let session = session::load_session(
            &self
                .session_file
                .clone()
                .or_else(session::default_session_file)
                .context("no session path")?,
        )?;
        GeckoApi::new(&self.app_api_base_url, &tokens, &session)
    }
}
#[derive(Debug, Subcommand)]
pub enum CatalogCommand {
    List(Connection),
}

pub fn run_filters(command: CatalogCommand) -> Result<()> {
    let CatalogCommand::List(connection) = command;
    let filters = connection
        .open()?
        .collection("filters", &[("saved", "1".into())])?;
    let rows: Vec<_> = filters.iter().map(|f| serde_json::json!({"id": f["id"], "name": f["name"], "requirement": f["requirement"]})).collect();
    println!(
        "{}",
        serde_json::to_string_pretty(&serde_json::json!({"filters": rows}))?
    );
    Ok(())
}
