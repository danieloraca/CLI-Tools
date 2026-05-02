use crate::contacts::{ContactRow, ContactsPage, render_contacts_page};
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
    widgets::{Block, Borders, Cell, Paragraph, Row, Table, TableState, Wrap},
};
use std::{io, time::Duration};

const TURQUOISE: Color = Color::Rgb(64, 224, 208);

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
