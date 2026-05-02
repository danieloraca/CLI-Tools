mod api;
mod app;
mod auth;
mod contacts;
mod profiles;
mod prompt;
mod session;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Debug, Parser)]
#[command(name = "gecko")]
#[command(about = "Gecko account CLI")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Debug, Subcommand)]
enum Commands {
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
        #[arg(
            long,
            env = "GECKO_ACCOUNT_API_URL",
            default_value = "https://account-api-stage.geckoengage.com"
        )]
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
        #[arg(
            long,
            env = "GECKO_APP_API_URL",
            default_value = contacts::DEFAULT_APP_API_URL
        )]
        app_api_base_url: String,

        /// Selected profile session output file.
        #[arg(long)]
        session_file: Option<PathBuf>,

        /// App-scoped token output file used for Geckoform API calls.
        #[arg(long)]
        app_token_file: Option<PathBuf>,

        /// Skip the post-profile app menu.
        #[arg(long)]
        no_menu: bool,
    },

    /// List/select profiles using the saved login token.
    Profiles {
        /// Account API base URL.
        #[arg(
            long,
            env = "GECKO_ACCOUNT_API_URL",
            default_value = "https://account-api-stage.geckoengage.com"
        )]
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
        #[arg(
            long,
            env = "GECKO_APP_API_URL",
            default_value = contacts::DEFAULT_APP_API_URL
        )]
        app_api_base_url: String,

        /// Selected profile session output file.
        #[arg(long)]
        session_file: Option<PathBuf>,

        /// App-scoped token output file used for Geckoform API calls.
        #[arg(long)]
        app_token_file: Option<PathBuf>,

        /// Skip the post-profile app menu.
        #[arg(long)]
        no_menu: bool,
    },

    /// Show contacts for the saved selected profile.
    Contacts {
        /// App API base URL.
        #[arg(
            long,
            env = "GECKO_APP_API_URL",
            default_value = contacts::DEFAULT_APP_API_URL
        )]
        app_api_base_url: String,

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
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
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
                            app_api_base_url,
                            contacts_page: 1,
                            contacts_per_page: 15,
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
                        app_api_base_url,
                        contacts_page: 1,
                        contacts_per_page: 15,
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
            let contacts = contacts::ContactService::new(app_api_base_url)?
                .list_contacts(&tokens, &session, page, per_page)?;

            println!("{}", contacts::render_contacts_page(&contacts));
        }
    }

    Ok(())
}
