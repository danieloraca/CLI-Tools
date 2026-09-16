mod api;
mod app;
mod app_identity;
mod auth;
mod catalog;
mod contacts;
mod export;
mod gecko;
mod profiles;
mod progress;
mod prompt;
mod query;
mod scenarios;
mod session;
mod storage;
#[cfg(test)]
mod test_support;
mod tui;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Debug, Parser)]
#[command(name = "cli_tools")]
#[command(about = "CLI Tools")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Debug, Subcommand)]
enum Commands {
    /// Discover contact fields and their IDs/types/choices.
    Fields {
        #[command(subcommand)]
        command: catalog::CatalogCommand,
    },
    /// Discover saved contact filters.
    Filters {
        #[command(subcommand)]
        command: catalog::CatalogCommand,
    },
    /// Generate and apply repeatable Gecko development scenarios.
    Scenario {
        #[command(subcommand)]
        command: scenarios::ScenarioCommand,
    },
    /// Log in with the account API used by the web app.
    Login {
        /// Email address to log in with.
        #[arg(short, long)]
        email: Option<String>,

        /// Password. If omitted, the CLI prompts without echoing.
        #[arg(short, long)]
        password: Option<String>,

        /// MFA code. If omitted and required, the CLI prompts.
        #[arg(long)]
        mfa_code: Option<String>,

        /// MFA method to select when the account offers multiple methods.
        #[arg(long)]
        mfa_method: Option<String>,

        /// Account API base URL.
        #[arg(long, env = "CLI_TOOLS_ACCOUNT_API_URL", hide_env_values = true)]
        base_url: String,

        /// Token output file. Use --no-store to avoid writing tokens.
        #[arg(long)]
        token_file: Option<PathBuf>,

        /// Do not persist tokens after login.
        #[arg(long)]
        no_store: bool,

        /// Print the token response JSON after login.
        #[arg(long)]
        print_tokens: bool,

        /// Skip loading and choosing a profile after login.
        #[arg(long)]
        no_profile_select: bool,

        /// Client to request redirect URLs for.
        #[arg(long, default_value = "web")]
        client: String,

        /// App to request redirect URLs for.
        #[arg(long)]
        app: Option<String>,

        /// Select a profile by id without prompting.
        #[arg(long)]
        profile_id: Option<String>,

        /// Select automatically when only one open profile is returned.
        #[arg(long)]
        auto_select_single: bool,

        /// App API base URL for post-login menu actions.
        #[arg(long, env = "CLI_TOOLS_APP_API_URL", hide_env_values = true)]
        app_api_base_url: Option<String>,

        /// Selected profile session output file.
        #[arg(long)]
        session_file: Option<PathBuf>,

        /// App-scoped token output file used for app API calls.
        #[arg(long)]
        app_token_file: Option<PathBuf>,

        /// Skip the post-profile app menu.
        #[arg(long)]
        no_menu: bool,
    },

    /// List/select profiles using the saved login token.
    Profiles {
        /// Account API base URL.
        #[arg(long, env = "CLI_TOOLS_ACCOUNT_API_URL", hide_env_values = true)]
        base_url: String,

        /// Token file to read.
        #[arg(long)]
        token_file: Option<PathBuf>,

        /// Client to request redirect URLs for.
        #[arg(long, default_value = "web")]
        client: String,

        /// App to request redirect URLs for.
        #[arg(long)]
        app: Option<String>,

        /// Select a profile by id without prompting.
        #[arg(long)]
        profile_id: Option<String>,

        /// Select automatically when only one open profile is returned.
        #[arg(long)]
        auto_select_single: bool,

        /// App API base URL for post-login menu actions.
        #[arg(long, env = "CLI_TOOLS_APP_API_URL", hide_env_values = true)]
        app_api_base_url: Option<String>,

        /// Selected profile session output file.
        #[arg(long)]
        session_file: Option<PathBuf>,

        /// App-scoped token output file used for app API calls.
        #[arg(long)]
        app_token_file: Option<PathBuf>,

        /// Skip the post-profile app menu.
        #[arg(long)]
        no_menu: bool,
    },

    /// Show contacts for the saved selected profile.
    Contacts {
        /// App API base URL.
        #[arg(long, env = "CLI_TOOLS_APP_API_URL", hide_env_values = true)]
        app_api_base_url: Option<String>,

        /// App-scoped token file to read. Deprecated alias for --app-token-file.
        #[arg(long)]
        token_file: Option<PathBuf>,

        /// App-scoped token file to read.
        #[arg(long)]
        app_token_file: Option<PathBuf>,

        /// Selected profile session file to read.
        #[arg(long)]
        session_file: Option<PathBuf>,

        /// Contacts page to load.
        #[arg(long, default_value_t = 1)]
        page: u32,

        /// Contacts per page to load.
        #[arg(long, default_value_t = 15)]
        per_page: u32,

        /// Print a plain table instead of opening the TUI.
        #[arg(long)]
        plain: bool,
        /// Print structured JSON instead of opening the TUI.
        #[arg(long, conflicts_with = "plain")]
        json: bool,
        #[command(flatten)]
        query: query::ContactQuery,
        #[command(flatten)]
        export: export::ExportArgs,
    },
}

fn main() -> Result<()> {
    dotenvy::dotenv().ok();
    let cli = Cli::parse();

    match cli.command {
        Commands::Fields { command } => catalog::run_fields(command)?,
        Commands::Filters { command } => catalog::run_filters(command)?,
        Commands::Scenario { command } => scenarios::run(command)?,
        Commands::Login {
            email,
            password,
            mfa_code,
            mfa_method,
            base_url,
            token_file,
            no_store,
            print_tokens,
            no_profile_select,
            client,
            app,
            profile_id,
            auto_select_single,
            app_api_base_url,
            session_file,
            app_token_file,
            no_menu,
        } => {
            let mut options = auth::LoginOptions {
                email,
                password,
                mfa_code,
                mfa_method,
                base_url: base_url.clone(),
                print_tokens,
                ..auth::LoginOptions::default()
            };

            if no_store {
                options.token_file = None;
            } else if token_file.is_some() {
                options.token_file = token_file;
            }

            let tokens = auth::login(options)?;

            if !no_profile_select {
                let session = profiles::select_profile_after_login(
                    &base_url,
                    &tokens,
                    profiles::ProfileSelectionOptions {
                        client,
                        app,
                        profile_id,
                        auto_select_single,
                        ..profiles::ProfileSelectionOptions::default()
                    },
                )?;
                let app_tokens = auth::claim_app_tokens(&base_url, &session.redirect_url)?;
                app_identity::validate_token_profile(&app_tokens, &session)?;

                if !no_store {
                    let app_token_file = app_token_file
                        .or_else(auth::default_app_token_file)
                        .context(
                            "app token file path was not provided and no home directory was found",
                        )?;
                    auth::persist_tokens(&app_tokens, Some(&app_token_file))?;

                    let session_file = session_file
                        .or_else(session::default_session_file)
                        .context(
                            "session file path was not provided and no home directory was found",
                        )?;
                    session::save_session(&session, Some(&session_file))?;
                }

                if !no_menu {
                    app::run_menu(
                        &app_tokens,
                        &session,
                        app::AppMenuOptions {
                            app_api_base_url: required_app_api_base_url(app_api_base_url)?,
                            contacts_page: 1,
                            contacts_per_page: 15,
                            contacts_query: Default::default(),
                        },
                    )?;
                }
            }
        }
        Commands::Profiles {
            base_url,
            token_file,
            client,
            app,
            profile_id,
            auto_select_single,
            app_api_base_url,
            session_file,
            app_token_file,
            no_menu,
        } => {
            let token_file = token_file
                .or_else(auth::default_token_file)
                .context("token file path was not provided and no home directory was found")?;
            let tokens = auth::load_tokens(&token_file)?;

            let session = profiles::select_profile_after_login(
                &base_url,
                &tokens,
                profiles::ProfileSelectionOptions {
                    client,
                    app,
                    profile_id,
                    auto_select_single,
                    ..profiles::ProfileSelectionOptions::default()
                },
            )?;
            let app_tokens = auth::claim_app_tokens(&base_url, &session.redirect_url)?;
            app_identity::validate_token_profile(&app_tokens, &session)?;

            let session_file = session_file
                .or_else(session::default_session_file)
                .context("session file path was not provided and no home directory was found")?;
            session::save_session(&session, Some(&session_file))?;
            let app_token_file = app_token_file
                .or_else(auth::default_app_token_file)
                .context("app token file path was not provided and no home directory was found")?;
            auth::persist_tokens(&app_tokens, Some(&app_token_file))?;

            if !no_menu {
                app::run_menu(
                    &app_tokens,
                    &session,
                    app::AppMenuOptions {
                        app_api_base_url: required_app_api_base_url(app_api_base_url)?,
                        contacts_page: 1,
                        contacts_per_page: 15,
                        contacts_query: Default::default(),
                    },
                )?;
            }
        }
        Commands::Contacts {
            app_api_base_url,
            token_file,
            app_token_file,
            session_file,
            page,
            per_page,
            plain,
            json,
            query,
            export,
        } => {
            let token_file = app_token_file
                .or(token_file)
                .or_else(auth::default_app_token_file)
                .context("app token file path was not provided and no home directory was found")?;
            let session_file = session_file
                .or_else(session::default_session_file)
                .context("session file path was not provided and no home directory was found")?;
            let tokens = auth::load_tokens(&token_file)?;
            let session = session::load_session(&session_file)?;
            let app_api_base_url = required_app_api_base_url(app_api_base_url)?;

            query.validate()?;
            if export.active() {
                export::run(
                    &app_api_base_url,
                    &tokens,
                    &session,
                    query,
                    &export,
                    page,
                    per_page,
                    plain,
                )?;
            } else if plain || json {
                let contacts = contacts::ContactService::new(&app_api_base_url)?
                    .with_query(query)?
                    .list_contacts(&tokens, &session, page, per_page)?;
                if json {
                    println!("{}", serde_json::to_string_pretty(&contacts)?);
                } else {
                    println!("{}", contacts::render_contacts_page(&contacts));
                }
            } else if let Some(contact) = app::browse_contacts(
                &tokens,
                &session,
                app::AppMenuOptions {
                    app_api_base_url,
                    contacts_page: page,
                    contacts_per_page: per_page,
                    contacts_query: query,
                },
            )? {
                println!(
                    "Selected contact: {} <{}>",
                    contact.full_name, contact.email
                );
            }
        }
    }

    Ok(())
}

fn required_app_api_base_url(value: Option<String>) -> Result<String> {
    value.context(
        "app API base URL was not provided; set CLI_TOOLS_APP_API_URL or pass --app-api-base-url",
    )
}
