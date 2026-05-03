use crate::auth::TokenSet;
use crate::contacts::{ContactRow, ContactService};
use crate::session::AppSession;
use crate::tui::{
    MainMenuOutcome, run_contacts_browser_tui, run_main_menu_tui_with_contacts_loader,
};
use anyhow::Result;

#[derive(Debug, Clone)]
pub struct AppMenuOptions {
    pub app_api_base_url: String,
    pub contacts_page: u32,
    pub contacts_per_page: u32,
}

pub fn run_menu(tokens: &TokenSet, session: &AppSession, options: AppMenuOptions) -> Result<()> {
    loop {
        let service = ContactService::new(&options.app_api_base_url)?;
        let initial_page = run_main_menu_tui_with_contacts_loader(
            &session.account_name,
            &session.app_description,
            || {
                service.list_contacts(
                    tokens,
                    session,
                    options.contacts_page.max(1),
                    options.contacts_per_page,
                )
            },
        )?;

        match initial_page {
            MainMenuOutcome::Contacts(initial_page) => {
                if let Some(contact) =
                    browse_contacts_from_page(tokens, session, options.clone(), initial_page)?
                {
                    println!(
                        "Selected contact: {} <{}>\n",
                        contact.full_name, contact.email
                    );
                }
            }
            MainMenuOutcome::Quit => return Ok(()),
        }
    }
}

pub fn browse_contacts(
    tokens: &TokenSet,
    session: &AppSession,
    options: AppMenuOptions,
) -> Result<Option<ContactRow>> {
    let service = ContactService::new(&options.app_api_base_url)?;
    let initial_page = service.list_contacts(
        tokens,
        session,
        options.contacts_page.max(1),
        options.contacts_per_page,
    )?;

    browse_contacts_from_page(tokens, session, options, initial_page)
}

fn browse_contacts_from_page(
    tokens: &TokenSet,
    session: &AppSession,
    options: AppMenuOptions,
    initial_page: crate::contacts::ContactsPage,
) -> Result<Option<ContactRow>> {
    let service = ContactService::new(&options.app_api_base_url)?;

    run_contacts_browser_tui(initial_page, |page, per_page| {
        service.list_contacts(tokens, session, page, per_page)
    })
}
