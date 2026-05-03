use crate::contacts::{ContactRow, ContactsPage, render_contacts_page};
use crate::profiles::ProfileChoice;
use anyhow::Result;
use crossterm::{
    event::{self, Event, KeyCode, KeyEventKind},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{
    Frame, Terminal,
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout, Margin},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{
        Block, Borders, Cell, List, ListItem, ListState, Paragraph, Row, Table, TableState, Wrap,
    },
};
use std::{io, time::Duration};

const TURQUOISE: Color = Color::Rgb(64, 224, 208);

#[derive(Debug, Clone)]
pub struct ProfileSelectorTui<'a> {
    choices: &'a [&'a ProfileChoice],
    state: ListState,
}

impl<'a> ProfileSelectorTui<'a> {
    pub fn new(choices: &'a [&'a ProfileChoice]) -> Self {
        let mut state = ListState::default();
        if !choices.is_empty() {
            state.select(Some(0));
        }

        Self { choices, state }
    }

    #[cfg(test)]
    fn selected_index(&self) -> Option<usize> {
        self.state.selected()
    }

    fn selected_choice(&self) -> Option<&ProfileChoice> {
        self.state
            .selected()
            .and_then(|index| self.choices.get(index).copied())
    }

    pub fn next(&mut self) {
        let len = self.choices.len();
        if len == 0 {
            self.state.select(None);
            return;
        }

        let index = match self.state.selected() {
            Some(index) if index + 1 < len => index + 1,
            _ => 0,
        };
        self.state.select(Some(index));
    }

    pub fn previous(&mut self) {
        let len = self.choices.len();
        if len == 0 {
            self.state.select(None);
            return;
        }

        let index = match self.state.selected() {
            Some(0) | None => len - 1,
            Some(index) => index - 1,
        };
        self.state.select(Some(index));
    }
}

pub fn run_profile_selector_tui(choices: &[&ProfileChoice]) -> Result<usize> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let mut app = ProfileSelectorTui::new(choices);
    let result = run_profile_selector_loop(&mut terminal, &mut app);

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    result
}

fn run_profile_selector_loop(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    app: &mut ProfileSelectorTui<'_>,
) -> Result<usize> {
    loop {
        terminal.draw(|frame| draw_profile_selector(frame, app))?;

        if event::poll(Duration::from_millis(250))? {
            let Event::Key(key) = event::read()? else {
                continue;
            };

            if key.kind == KeyEventKind::Release {
                continue;
            }

            match key.code {
                KeyCode::Char('j') | KeyCode::Down | KeyCode::Tab => app.next(),
                KeyCode::Char('k') | KeyCode::Up | KeyCode::BackTab => app.previous(),
                KeyCode::Enter => return Ok(app.state.selected().unwrap_or(0)),
                KeyCode::Char('q') | KeyCode::Esc => {
                    anyhow::bail!("profile selection cancelled")
                }
                _ => {}
            }
        }
    }
}

fn draw_profile_selector(frame: &mut Frame, app: &mut ProfileSelectorTui<'_>) {
    let area = frame.area();
    let shell = Block::default()
        .title(Line::from(vec![
            Span::styled(" Select Profile ", Style::default().fg(Color::White)),
            Span::styled(
                "enter choose | up/down move | q/esc cancel",
                Style::default().fg(Color::Gray),
            ),
        ]))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(TURQUOISE));
    frame.render_widget(shell, area);

    let inner = area.inner(Margin {
        horizontal: 2,
        vertical: 1,
    });
    let layout = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(58), Constraint::Percentage(42)])
        .split(inner);

    let items = app
        .choices
        .iter()
        .map(|choice| {
            ListItem::new(vec![
                Line::from(Span::styled(
                    &choice.account_name,
                    Style::default().add_modifier(Modifier::BOLD),
                )),
                Line::from(Span::styled(
                    &choice.app_description,
                    Style::default().fg(Color::Gray),
                )),
            ])
        })
        .collect::<Vec<_>>();

    let list = List::new(items)
        .block(Block::default().title(" Profiles ").borders(Borders::ALL))
        .highlight_style(
            Style::default()
                .bg(TURQUOISE)
                .fg(Color::Black)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol(">> ");
    frame.render_stateful_widget(list, layout[0], &mut app.state);

    let details = profile_details(app.selected_choice(), app.choices.len());
    frame.render_widget(details, layout[1]);
}

fn profile_details(choice: Option<&ProfileChoice>, count: usize) -> Paragraph<'static> {
    let lines = if let Some(choice) = choice {
        vec![
            Line::from(Span::styled(
                choice.account_name.clone(),
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD),
            )),
            Line::from(""),
            detail_line("Application", &choice.app_description),
            detail_line("Account", &choice.account_header),
            detail_line("Profiles", &count.to_string()),
            Line::from(""),
            Line::from(Span::styled(
                "Press Enter to continue with this profile.",
                Style::default().fg(TURQUOISE),
            )),
        ]
    } else {
        vec![Line::from("No profiles available")]
    };

    Paragraph::new(lines)
        .block(Block::default().title(" Details ").borders(Borders::ALL))
        .wrap(Wrap { trim: true })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MenuAction {
    Contacts,
    Quit,
}

#[derive(Debug, Clone)]
pub struct MainMenuTui {
    account_name: String,
    app_description: String,
    state: ListState,
    status: String,
}

impl MainMenuTui {
    pub fn new(account_name: impl Into<String>, app_description: impl Into<String>) -> Self {
        let mut state = ListState::default();
        state.select(Some(0));

        Self {
            account_name: account_name.into(),
            app_description: app_description.into(),
            state,
            status: "Choose a section to open.".to_string(),
        }
    }

    #[cfg(test)]
    fn selected_index(&self) -> Option<usize> {
        self.state.selected()
    }

    pub fn next(&mut self) {
        let index = match self.state.selected() {
            Some(index) if index + 1 < menu_items().len() => index + 1,
            _ => 0,
        };
        self.state.select(Some(index));
    }

    pub fn previous(&mut self) {
        let index = match self.state.selected() {
            Some(0) | None => menu_items().len() - 1,
            Some(index) => index - 1,
        };
        self.state.select(Some(index));
    }

    pub fn activate(&mut self) -> Option<MenuAction> {
        match self.state.selected().unwrap_or(0) {
            0 => {
                self.status =
                    "Overview is not implemented yet. Choose Contacts to continue.".to_string();
                None
            }
            1 => Some(MenuAction::Contacts),
            2 => Some(MenuAction::Quit),
            _ => None,
        }
    }
}

pub fn run_main_menu_tui(account_name: &str, app_description: &str) -> Result<MenuAction> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let mut app = MainMenuTui::new(account_name, app_description);
    let result = run_main_menu_loop(&mut terminal, &mut app);

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    result
}

fn run_main_menu_loop(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    app: &mut MainMenuTui,
) -> Result<MenuAction> {
    loop {
        terminal.draw(|frame| draw_main_menu(frame, app))?;

        if event::poll(Duration::from_millis(250))? {
            let Event::Key(key) = event::read()? else {
                continue;
            };

            if key.kind == KeyEventKind::Release {
                continue;
            }

            match key.code {
                KeyCode::Char('q') | KeyCode::Esc => return Ok(MenuAction::Quit),
                KeyCode::Char('j') | KeyCode::Down | KeyCode::Tab => app.next(),
                KeyCode::Char('k') | KeyCode::Up | KeyCode::BackTab => app.previous(),
                KeyCode::Enter => {
                    if let Some(action) = app.activate() {
                        return Ok(action);
                    }
                }
                _ => {}
            }
        }
    }
}

fn draw_main_menu(frame: &mut Frame, app: &mut MainMenuTui) {
    let area = frame.area();
    let shell = Block::default()
        .title(Line::from(vec![
            Span::styled(" Gecko ", Style::default().fg(Color::White)),
            Span::styled(
                "enter open | up/down move | q/esc quit",
                Style::default().fg(Color::Gray),
            ),
        ]))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(TURQUOISE));
    frame.render_widget(shell, area);

    let inner = area.inner(Margin {
        horizontal: 2,
        vertical: 1,
    });
    let layout = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(36), Constraint::Percentage(64)])
        .split(inner);

    let items = menu_items()
        .iter()
        .map(|item| {
            ListItem::new(vec![
                Line::from(Span::styled(
                    item.title,
                    Style::default().add_modifier(Modifier::BOLD),
                )),
                Line::from(Span::styled(
                    item.subtitle,
                    Style::default().fg(Color::Gray),
                )),
            ])
        })
        .collect::<Vec<_>>();

    let menu = List::new(items)
        .block(Block::default().title(" Menu ").borders(Borders::ALL))
        .highlight_style(
            Style::default()
                .bg(TURQUOISE)
                .fg(Color::Black)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol(">> ");
    frame.render_stateful_widget(menu, layout[0], &mut app.state);

    let selected = app.state.selected().unwrap_or(0);
    let item = &menu_items()[selected];
    let details = vec![
        Line::from(Span::styled(
            &app.account_name,
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(Span::styled(
            &app.app_description,
            Style::default().fg(Color::Gray),
        )),
        Line::from(""),
        Line::from(Span::styled(
            item.title,
            Style::default().fg(TURQUOISE).add_modifier(Modifier::BOLD),
        )),
        Line::from(item.description),
        Line::from(""),
        Line::from(Span::styled(
            &app.status,
            Style::default().fg(Color::Yellow),
        )),
    ];

    frame.render_widget(
        Paragraph::new(details)
            .block(Block::default().title(" Details ").borders(Borders::ALL))
            .wrap(Wrap { trim: true }),
        layout[1],
    );
}

#[derive(Debug, Clone, Copy)]
struct MenuItem {
    title: &'static str,
    subtitle: &'static str,
    description: &'static str,
}

fn menu_items() -> &'static [MenuItem] {
    &[
        MenuItem {
            title: "Overview",
            subtitle: "Dashboard placeholder",
            description: "The overview screen is reserved for the next dashboard pass.",
        },
        MenuItem {
            title: "Contacts",
            subtitle: "Browse and select contacts",
            description: "Open the contacts table, move through rows, and select a contact.",
        },
        MenuItem {
            title: "Quit",
            subtitle: "Leave the CLI menu",
            description: "Return to the shell.",
        },
    ]
}

#[derive(Debug, Clone)]
pub struct ContactsTui {
    page: ContactsPage,
    state: TableState,
    selected: Option<ContactRow>,
}

impl ContactsTui {
    pub fn new(page: ContactsPage) -> Self {
        let mut state = TableState::default();
        if !page.contacts.is_empty() {
            state.select(Some(0));
        }

        Self {
            page,
            state,
            selected: None,
        }
    }

    pub fn selected_index(&self) -> Option<usize> {
        self.state.selected()
    }

    pub fn selected_contact(&self) -> Option<&ContactRow> {
        self.selected_index()
            .and_then(|index| self.page.contacts.get(index))
    }

    pub fn take_selected(self) -> Option<ContactRow> {
        self.selected
    }

    pub fn next(&mut self) {
        let len = self.page.contacts.len();
        if len == 0 {
            self.state.select(None);
            return;
        }

        let index = match self.state.selected() {
            Some(index) if index + 1 < len => index + 1,
            _ => 0,
        };
        self.state.select(Some(index));
    }

    pub fn previous(&mut self) {
        let len = self.page.contacts.len();
        if len == 0 {
            self.state.select(None);
            return;
        }

        let index = match self.state.selected() {
            Some(0) | None => len - 1,
            Some(index) => index - 1,
        };
        self.state.select(Some(index));
    }

    pub fn select_current(&mut self) {
        self.selected = self.selected_contact().cloned();
    }
}

pub fn run_contacts_tui(page: ContactsPage) -> Result<Option<ContactRow>> {
    if page.contacts.is_empty() {
        println!("{}", render_contacts_page(&page));
        return Ok(None);
    }

    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let mut app = ContactsTui::new(page);
    let result = run_loop(&mut terminal, &mut app);

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    result?;
    Ok(app.take_selected())
}

fn run_loop(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    app: &mut ContactsTui,
) -> Result<()> {
    loop {
        terminal.draw(|frame| draw_contacts(frame, app))?;

        if event::poll(Duration::from_millis(250))? {
            let Event::Key(key) = event::read()? else {
                continue;
            };

            if key.kind == KeyEventKind::Release {
                continue;
            }

            match key.code {
                KeyCode::Char('q') | KeyCode::Esc => return Ok(()),
                KeyCode::Char('j') | KeyCode::Down => app.next(),
                KeyCode::Char('k') | KeyCode::Up => app.previous(),
                KeyCode::Enter => {
                    app.select_current();
                    return Ok(());
                }
                _ => {}
            }
        }
    }
}

fn draw_contacts(frame: &mut Frame, app: &mut ContactsTui) {
    let area = frame.area();
    let shell = Block::default()
        .title(Line::from(vec![
            Span::styled(" Gecko Contacts ", Style::default().fg(Color::White)),
            Span::styled(
                "q/esc quit | enter select | j/k move",
                Style::default().fg(Color::Gray),
            ),
        ]))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(TURQUOISE));
    frame.render_widget(shell, area);

    let inner = area.inner(Margin {
        horizontal: 1,
        vertical: 1,
    });
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(68), Constraint::Percentage(32)])
        .split(inner);

    let rows = app.page.contacts.iter().map(|contact| {
        Row::new(vec![
            Cell::from(non_empty(&contact.full_name, "Unnamed contact")),
            Cell::from(contact.email.clone()),
            Cell::from(contact.created_at.clone()),
            Cell::from(contact.labels.join(", ")),
        ])
    });

    let table = Table::new(
        rows,
        [
            Constraint::Percentage(25),
            Constraint::Percentage(35),
            Constraint::Percentage(24),
            Constraint::Percentage(16),
        ],
    )
    .header(
        Row::new(vec!["Full name", "Email", "Created", "Labels"])
            .style(
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            )
            .bottom_margin(1),
    )
    .block(
        Block::default()
            .title(format!(
                " Contacts page {} ({} shown) ",
                app.page.page,
                app.page.contacts.len()
            ))
            .borders(Borders::ALL),
    )
    .row_highlight_style(
        Style::default()
            .bg(TURQUOISE)
            .fg(Color::White)
            .add_modifier(Modifier::BOLD),
    )
    .highlight_symbol(">> ");
    frame.render_stateful_widget(table, chunks[0], &mut app.state);

    let details = contact_details(app.selected_contact());
    frame.render_widget(details, chunks[1]);
}

fn contact_details(contact: Option<&ContactRow>) -> Paragraph<'static> {
    let lines = if let Some(contact) = contact {
        vec![
            Line::from(vec![Span::styled(
                non_empty(&contact.full_name, "Unnamed contact"),
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD),
            )]),
            Line::from(""),
            detail_line("ID", &contact.id),
            detail_line("Email", &contact.email),
            detail_line("Phone", &contact.phone),
            detail_line("Created", &contact.created_at),
            detail_line("Last chat", &contact.last_chat_message),
            detail_line("Labels", &contact.labels.join(", ")),
        ]
    } else {
        vec![Line::from("No contact selected")]
    };

    Paragraph::new(lines)
        .block(Block::default().title(" Details ").borders(Borders::ALL))
        .wrap(Wrap { trim: true })
}

fn detail_line(label: &'static str, value: &str) -> Line<'static> {
    Line::from(vec![
        Span::styled(
            format!("{label}: "),
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::from(non_empty(value, "-")),
    ])
}

fn non_empty(value: &str, fallback: &str) -> String {
    if value.trim().is_empty() {
        fallback.to_string()
    } else {
        value.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn contacts_page() -> ContactsPage {
        ContactsPage {
            page: 1,
            per_page: 15,
            contacts: vec![
                ContactRow {
                    id: "1".to_string(),
                    full_name: "A Contact".to_string(),
                    email: "a@example.com".to_string(),
                    last_chat_message: String::new(),
                    created_at: "Today".to_string(),
                    phone: String::new(),
                    labels: Vec::new(),
                },
                ContactRow {
                    id: "2".to_string(),
                    full_name: "B Contact".to_string(),
                    email: "b@example.com".to_string(),
                    last_chat_message: String::new(),
                    created_at: "Yesterday".to_string(),
                    phone: String::new(),
                    labels: vec!["bigbang".to_string()],
                },
            ],
        }
    }

    fn profile_choices() -> Vec<ProfileChoice> {
        vec![
            ProfileChoice {
                profile_id: serde_json::json!("p-1"),
                account_id: serde_json::json!(281),
                account_header: "281".to_string(),
                account_name: "Stage 281".to_string(),
                app_description: "Client Administration (Manage)".to_string(),
                is_closed: false,
            },
            ProfileChoice {
                profile_id: serde_json::json!("p-2"),
                account_id: serde_json::json!(281),
                account_header: "281".to_string(),
                account_name: "Stage 281".to_string(),
                app_description: "Forms, Events, Call Centre, Email & Text Campaigns".to_string(),
                is_closed: false,
            },
        ]
    }

    #[test]
    fn profile_selector_starts_on_first_profile() {
        let choices = profile_choices();
        let refs = choices.iter().collect::<Vec<_>>();
        let app = ProfileSelectorTui::new(&refs);

        assert_eq!(app.selected_index(), Some(0));
        assert_eq!(
            app.selected_choice().unwrap().profile_id,
            serde_json::json!("p-1")
        );
    }

    #[test]
    fn profile_selector_moves_with_wraparound() {
        let choices = profile_choices();
        let refs = choices.iter().collect::<Vec<_>>();
        let mut app = ProfileSelectorTui::new(&refs);

        app.next();
        assert_eq!(app.selected_index(), Some(1));
        app.next();
        assert_eq!(app.selected_index(), Some(0));
        app.previous();
        assert_eq!(app.selected_index(), Some(1));
    }

    #[test]
    fn menu_starts_on_overview_and_moves_with_wraparound() {
        let mut app = MainMenuTui::new("Stage 281", "Forms");

        assert_eq!(app.selected_index(), Some(0));
        app.next();
        assert_eq!(app.selected_index(), Some(1));
        app.previous();
        assert_eq!(app.selected_index(), Some(0));
        app.previous();
        assert_eq!(app.selected_index(), Some(2));
    }

    #[test]
    fn menu_activation_returns_actions_for_contacts_and_quit() {
        let mut app = MainMenuTui::new("Stage 281", "Forms");

        assert_eq!(app.activate(), None);
        app.next();
        assert_eq!(app.activate(), Some(MenuAction::Contacts));
        app.next();
        assert_eq!(app.activate(), Some(MenuAction::Quit));
    }

    #[test]
    fn starts_on_first_contact() {
        let app = ContactsTui::new(contacts_page());

        assert_eq!(app.selected_index(), Some(0));
        assert_eq!(app.selected_contact().unwrap().id, "1");
    }

    #[test]
    fn moves_selection_with_wraparound() {
        let mut app = ContactsTui::new(contacts_page());

        app.next();
        assert_eq!(app.selected_index(), Some(1));
        app.next();
        assert_eq!(app.selected_index(), Some(0));
        app.previous();
        assert_eq!(app.selected_index(), Some(1));
    }

    #[test]
    fn enter_selects_current_contact() {
        let mut app = ContactsTui::new(contacts_page());
        app.next();
        app.select_current();

        assert_eq!(app.take_selected().unwrap().id, "2");
    }
}
