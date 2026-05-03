use crate::auth::TokenSet;
use crate::contacts::{ContactRow, ContactService};
use crate::session::AppSession;
use crate::tui::{ContactsAction, MenuAction, run_contacts_tui, run_main_menu_tui};
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
                if let Some(contact) = browse_contacts(tokens, session, options.clone())? {
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

pub fn browse_contacts(
    tokens: &TokenSet,
    session: &AppSession,
    options: AppMenuOptions,
) -> Result<Option<ContactRow>> {
    let service = ContactService::new(&options.app_api_base_url)?;
    let mut page = options.contacts_page.max(1);

    loop {
        let contacts = service.list_contacts(tokens, session, page, options.contacts_per_page)?;
        let pagination = contacts.pagination;

        match run_contacts_tui(contacts)? {
            ContactsAction::Selected(contact) => return Ok(Some(contact)),
            ContactsAction::Quit => return Ok(None),
            ContactsAction::PreviousPage => {
                if pagination.can_go_previous() {
                    page = page.saturating_sub(1).max(1);
                }
            }
            ContactsAction::NextPage => {
                if pagination.can_go_next() {
                    page = page.saturating_add(1);
                }
            }
        }
    }
}
