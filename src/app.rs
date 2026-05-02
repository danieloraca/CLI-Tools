use crate::auth::TokenSet;
use crate::contacts::ContactService;
use crate::session::AppSession;
use crate::tui::{MenuAction, run_contacts_tui, run_main_menu_tui};
use anyhow::Result;

#[derive(Debug, Clone)]
pub struct AppMenuOptions {
    pub app_api_base_url: String,
    pub contacts_page: u32,
    pub contacts_per_page: u32,
}

pub fn run_menu(tokens: &TokenSet, session: &AppSession, options: AppMenuOptions) -> Result<()> {
    loop {
        match run_main_menu_tui(&session.account_name, &session.app_description)? {
            MenuAction::Contacts => {
                let contacts = ContactService::new(&options.app_api_base_url)?.list_contacts(
                    tokens,
                    session,
                    options.contacts_page,
                    options.contacts_per_page,
                )?;
                if let Some(contact) = run_contacts_tui(contacts)? {
                    println!(
                        "Selected contact: {} <{}>\n",
                        contact.full_name, contact.email
                    );
                }
            }
            MenuAction::Quit => return Ok(()),
        }
    }
}
