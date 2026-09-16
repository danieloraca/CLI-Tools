use crate::contacts::{ContactDetail, ContactRow, ContactsPage};
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
use std::{io, thread, time::Duration};

const TURQUOISE: Color = Color::Rgb(64, 224, 208);

#[derive(Debug, Clone)]
pub struct ProfileSelectorTui<'a> {
    choices: &'a [&'a ProfileChoice],
    state: ListState,
    status: Option<String>,
    spinner_index: usize,
}

impl<'a> ProfileSelectorTui<'a> {
    pub fn new(choices: &'a [&'a ProfileChoice]) -> Self {
        let mut state = ListState::default();
        if !choices.is_empty() {
            state.select(Some(0));
        }

        Self {
            choices,
            state,
            status: None,
            spinner_index: 0,
        }
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

    pub fn set_loading_status(&mut self, message: &str) {
        let frame = SPINNER_FRAMES[self.spinner_index % SPINNER_FRAMES.len()];
        self.spinner_index = self.spinner_index.wrapping_add(1);
        self.status = Some(format!("{frame} {message}"));
    }
}

pub fn run_profile_selector_tui(choices: &[&ProfileChoice]) -> Result<usize> {
    run_profile_selector_tui_with_loader(choices, "Loading profile...", Ok)
}

pub fn run_profile_selector_tui_with_loader<T, F>(
    choices: &[&ProfileChoice],
    loading_message: &'static str,
    mut loader: F,
) -> Result<T>
where
    T: Send,
    F: FnMut(usize) -> Result<T> + Send,
{
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let mut app = ProfileSelectorTui::new(choices);
    let result = run_profile_selector_loop(&mut terminal, &mut app, loading_message, &mut loader);

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    result
}

fn run_profile_selector_loop<T, F>(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    app: &mut ProfileSelectorTui<'_>,
    loading_message: &'static str,
    loader: &mut F,
) -> Result<T>
where
    T: Send,
    F: FnMut(usize) -> Result<T> + Send,
{
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
                KeyCode::Enter => {
                    let selected = app.state.selected().unwrap_or(0);
                    return load_profile_selection(
                        terminal,
                        app,
                        selected,
                        loading_message,
                        loader,
                    );
                }
                KeyCode::Char('q') | KeyCode::Esc => {
                    anyhow::bail!("profile selection cancelled")
                }
                _ => {}
            }
        }
    }
}

fn load_profile_selection<T, F>(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    app: &mut ProfileSelectorTui<'_>,
    index: usize,
    loading_message: &'static str,
    loader: &mut F,
) -> Result<T>
where
    T: Send,
    F: FnMut(usize) -> Result<T> + Send,
{
    thread::scope(|scope| -> Result<T> {
        let handle = scope.spawn(|| loader(index));

        while !handle.is_finished() {
            app.set_loading_status(loading_message);
            terminal.draw(|frame| draw_profile_selector(frame, app))?;
            thread::sleep(Duration::from_millis(120));
        }

        handle
            .join()
            .map_err(|_| anyhow::anyhow!("profile loader panicked"))?
    })
}

fn draw_profile_selector(frame: &mut Frame, app: &mut ProfileSelectorTui<'_>) {
    let area = frame.area();
    let shell = Block::default()
        .title(Span::styled(
            " Select Profile ",
            Style::default().fg(Color::White),
        ))
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
    let side_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(8), Constraint::Length(8)])
        .split(layout[1]);

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
    frame.render_widget(details, side_chunks[0]);
    frame.render_widget(profile_legend(app.status.as_deref()), side_chunks[1]);
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

fn profile_legend(status: Option<&str>) -> Paragraph<'static> {
    let status = status.unwrap_or("Choose a profile to continue.");

    Paragraph::new(vec![
        legend_line("Move", "up/down or j/k"),
        legend_line("Choose", "enter"),
        legend_line("Cancel", "q or esc"),
        Line::from(""),
        Line::from(Span::styled(
            status.to_string(),
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        )),
    ])
    .block(Block::default().title(" Legend ").borders(Borders::ALL))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MenuAction {
    Contacts,
    Quit,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MainMenuOutcome {
    Contacts(ContactsPage),
    Quit,
}

#[derive(Debug, Clone)]
pub struct MainMenuTui {
    account_name: String,
    app_description: String,
    state: ListState,
    status: String,
    spinner_index: usize,
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
            spinner_index: 0,
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

    pub fn set_loading_status(&mut self, message: &str) {
        let frame = SPINNER_FRAMES[self.spinner_index % SPINNER_FRAMES.len()];
        self.spinner_index = self.spinner_index.wrapping_add(1);
        self.status = format!("{frame} {message}");
    }
}

pub fn run_main_menu_tui_with_contacts_loader<F>(
    account_name: &str,
    app_description: &str,
    mut load_contacts: F,
) -> Result<MainMenuOutcome>
where
    F: FnMut() -> Result<ContactsPage> + Send,
{
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let mut app = MainMenuTui::new(account_name, app_description);
    let result =
        run_main_menu_loop_with_contacts_loader(&mut terminal, &mut app, &mut load_contacts);

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    result
}

fn run_main_menu_loop_with_contacts_loader<F>(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    app: &mut MainMenuTui,
    load_contacts: &mut F,
) -> Result<MainMenuOutcome>
where
    F: FnMut() -> Result<ContactsPage> + Send,
{
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
                KeyCode::Char('q') | KeyCode::Esc => return Ok(MainMenuOutcome::Quit),
                KeyCode::Char('j') | KeyCode::Down | KeyCode::Tab => app.next(),
                KeyCode::Char('k') | KeyCode::Up | KeyCode::BackTab => app.previous(),
                KeyCode::Enter => {
                    if let Some(action) = app.activate() {
                        match action {
                            MenuAction::Contacts => {
                                return load_initial_contacts_page(
                                    terminal,
                                    app,
                                    "Loading contacts...",
                                    load_contacts,
                                )
                                .map(MainMenuOutcome::Contacts);
                            }
                            MenuAction::Quit => return Ok(MainMenuOutcome::Quit),
                        }
                    }
                }
                _ => {}
            }
        }
    }
}

fn load_initial_contacts_page<F>(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    app: &mut MainMenuTui,
    loading_message: &'static str,
    load_contacts: &mut F,
) -> Result<ContactsPage>
where
    F: FnMut() -> Result<ContactsPage> + Send,
{
    thread::scope(|scope| -> Result<ContactsPage> {
        let handle = scope.spawn(load_contacts);

        while !handle.is_finished() {
            app.set_loading_status(loading_message);
            terminal.draw(|frame| draw_main_menu(frame, app))?;
            thread::sleep(Duration::from_millis(120));
        }

        handle
            .join()
            .map_err(|_| anyhow::anyhow!("contacts loader panicked"))?
    })
}

fn draw_main_menu(frame: &mut Frame, app: &mut MainMenuTui) {
    let area = frame.area();
    let shell = Block::default()
        .title(Line::from(vec![
            Span::styled(" CLI Tools ", Style::default().fg(Color::White)),
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
    let side_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(8), Constraint::Length(8)])
        .split(layout[1]);

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
    ];

    frame.render_widget(
        Paragraph::new(details)
            .block(Block::default().title(" Details ").borders(Borders::ALL))
            .wrap(Wrap { trim: true }),
        side_chunks[0],
    );
    frame.render_widget(main_menu_legend(&app.status), side_chunks[1]);
}

fn main_menu_legend(status: &str) -> Paragraph<'static> {
    Paragraph::new(vec![
        legend_line("Move", "up/down or j/k"),
        legend_line("Open", "enter"),
        legend_line("Quit", "q or esc"),
        Line::from(""),
        Line::from(Span::styled(
            status.to_string(),
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        )),
    ])
    .block(Block::default().title(" Legend ").borders(Borders::ALL))
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
    status: Option<String>,
    spinner_index: usize,
    view: ContactsView,
}

const CONTACTS_PER_PAGE_OPTIONS: [u32; 3] = [15, 30, 50];
const SPINNER_FRAMES: [&str; 4] = ["|", "/", "-", "\\"];

#[derive(Debug, Clone, PartialEq, Eq)]
enum ContactsView {
    List,
    Detail(Box<ContactOverview>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ContactOverview {
    contact: ContactRow,
    detail: ContactDetail,
    scroll: u16,
    max_scroll: u16,
    viewport_height: u16,
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
            status: None,
            spinner_index: 0,
            view: ContactsView::List,
        }
    }

    pub fn selected_index(&self) -> Option<usize> {
        self.state.selected()
    }

    pub fn selected_contact(&self) -> Option<&ContactRow> {
        self.selected_index()
            .and_then(|index| self.page.contacts.get(index))
    }

    pub fn can_go_previous(&self) -> bool {
        self.page.pagination.can_go_previous()
    }

    pub fn can_go_next(&self) -> bool {
        self.page.pagination.can_go_next()
    }

    pub fn next_per_page(&self) -> u32 {
        next_per_page_value(self.page.pagination.per_page)
    }

    pub fn previous_per_page(&self) -> u32 {
        previous_per_page_value(self.page.pagination.per_page)
    }

    pub fn set_status(&mut self, status: impl Into<String>) {
        self.status = Some(status.into());
    }

    pub fn set_loading_status(&mut self, page: u32) {
        let frame = SPINNER_FRAMES[self.spinner_index % SPINNER_FRAMES.len()];
        self.spinner_index = self.spinner_index.wrapping_add(1);
        self.set_status(format!("{frame} Loading page {page}..."));
    }

    pub fn set_contact_loading_status(&mut self, name: &str) {
        let frame = SPINNER_FRAMES[self.spinner_index % SPINNER_FRAMES.len()];
        self.spinner_index = self.spinner_index.wrapping_add(1);
        self.set_status(format!("{frame} Loading {}...", non_empty(name, "contact")));
    }

    pub fn clear_status(&mut self) {
        self.status = None;
    }

    pub fn replace_page(&mut self, page: ContactsPage) {
        self.page = page;
        if self.page.contacts.is_empty() {
            self.state.select(None);
        } else {
            self.state.select(Some(0));
        }
        self.clear_status();
    }

    pub fn show_detail(&mut self, contact: ContactRow, detail: ContactDetail) {
        self.view = ContactsView::Detail(Box::new(ContactOverview {
            contact,
            detail,
            scroll: 0,
            max_scroll: 0,
            viewport_height: 0,
        }));
        self.clear_status();
    }

    pub fn show_list(&mut self) {
        self.view = ContactsView::List;
        self.clear_status();
    }

    pub fn is_detail_view(&self) -> bool {
        matches!(self.view, ContactsView::Detail(_))
    }

    pub fn next(&mut self) {
        if let ContactsView::Detail(overview) = &mut self.view {
            overview.scroll = overview.scroll.saturating_add(1).min(overview.max_scroll);
            return;
        }

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
        if let ContactsView::Detail(overview) = &mut self.view {
            overview.scroll = overview.scroll.saturating_sub(1);
            return;
        }

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

    fn scroll_detail_page(&mut self, forward: bool) {
        if let ContactsView::Detail(overview) = &mut self.view {
            let amount = overview.viewport_height.max(1);
            overview.scroll = if forward {
                overview
                    .scroll
                    .saturating_add(amount)
                    .min(overview.max_scroll)
            } else {
                overview.scroll.saturating_sub(amount)
            };
        }
    }

    fn scroll_detail_to_end(&mut self, end: bool) {
        if let ContactsView::Detail(overview) = &mut self.view {
            overview.scroll = if end { overview.max_scroll } else { 0 };
        }
    }
}

pub fn run_contacts_browser_tui<F, G>(
    initial_page: ContactsPage,
    mut load_page: F,
    mut load_detail: G,
) -> Result<Option<ContactRow>>
where
    F: FnMut(u32, u32) -> Result<ContactsPage> + Send,
    G: FnMut(&str) -> Result<ContactDetail> + Send,
{
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let mut app = ContactsTui::new(initial_page);
    let result = run_browser_loop(&mut terminal, &mut app, &mut load_page, &mut load_detail);

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    result
}

fn run_browser_loop<F, G>(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    app: &mut ContactsTui,
    load_page: &mut F,
    load_detail: &mut G,
) -> Result<Option<ContactRow>>
where
    F: FnMut(u32, u32) -> Result<ContactsPage> + Send,
    G: FnMut(&str) -> Result<ContactDetail> + Send,
{
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
                KeyCode::Char('q') | KeyCode::Esc => return Ok(None),
                KeyCode::Backspace | KeyCode::Char('b') if app.is_detail_view() => {
                    app.show_list();
                }
                KeyCode::Char('j') | KeyCode::Down => app.next(),
                KeyCode::Char('k') | KeyCode::Up => app.previous(),
                KeyCode::PageDown => app.scroll_detail_page(true),
                KeyCode::PageUp => app.scroll_detail_page(false),
                KeyCode::Home => app.scroll_detail_to_end(false),
                KeyCode::End => app.scroll_detail_to_end(true),
                KeyCode::Char('h') | KeyCode::Char('p') | KeyCode::Left => {
                    if app.is_detail_view() {
                        app.show_list();
                    } else if app.can_go_previous() {
                        load_contacts_page(
                            terminal,
                            app,
                            app.page.pagination.page.saturating_sub(1).max(1),
                            app.page.pagination.per_page,
                            load_page,
                        )?;
                    }
                }
                KeyCode::Char('l') | KeyCode::Char('n') | KeyCode::Right
                    if !app.is_detail_view() && app.can_go_next() =>
                {
                    load_contacts_page(
                        terminal,
                        app,
                        app.page.pagination.page.saturating_add(1),
                        app.page.pagination.per_page,
                        load_page,
                    )?;
                }
                KeyCode::Char('+') | KeyCode::Char('=') | KeyCode::Char(']')
                    if !app.is_detail_view() =>
                {
                    load_contacts_page(terminal, app, 1, app.next_per_page(), load_page)?;
                }
                KeyCode::Char('-') | KeyCode::Char('_') | KeyCode::Char('[')
                    if !app.is_detail_view() =>
                {
                    load_contacts_page(terminal, app, 1, app.previous_per_page(), load_page)?;
                }
                KeyCode::Char('1') if !app.is_detail_view() => {
                    load_contacts_page(terminal, app, 1, 15, load_page)?
                }
                KeyCode::Char('2') if !app.is_detail_view() => {
                    load_contacts_page(terminal, app, 1, 30, load_page)?
                }
                KeyCode::Char('3') if !app.is_detail_view() => {
                    load_contacts_page(terminal, app, 1, 50, load_page)?
                }
                KeyCode::Enter if !app.is_detail_view() => {
                    if let Some(contact) = app.selected_contact().cloned() {
                        load_contact_detail(terminal, app, contact, load_detail)?;
                    }
                }
                _ => {}
            }
        }
    }
}

fn load_contact_detail<G>(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    app: &mut ContactsTui,
    contact: ContactRow,
    load_detail: &mut G,
) -> Result<()>
where
    G: FnMut(&str) -> Result<ContactDetail> + Send,
{
    let detail = thread::scope(|scope| -> Result<ContactDetail> {
        let contact_id = contact.id.clone();
        let handle = scope.spawn(move || load_detail(&contact_id));

        while !handle.is_finished() {
            app.set_contact_loading_status(&contact.full_name);
            terminal.draw(|frame| draw_contacts(frame, app))?;
            thread::sleep(Duration::from_millis(120));
        }

        handle
            .join()
            .map_err(|_| anyhow::anyhow!("contact detail loader panicked"))?
    })?;

    app.show_detail(contact, detail);
    Ok(())
}

fn load_contacts_page<F>(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    app: &mut ContactsTui,
    page: u32,
    per_page: u32,
    load_page: &mut F,
) -> Result<()>
where
    F: FnMut(u32, u32) -> Result<ContactsPage> + Send,
{
    let next_page = thread::scope(|scope| -> Result<ContactsPage> {
        let handle = scope.spawn(|| load_page(page, per_page));

        while !handle.is_finished() {
            app.set_loading_status(page);
            terminal.draw(|frame| draw_contacts(frame, app))?;
            thread::sleep(Duration::from_millis(120));
        }

        handle
            .join()
            .map_err(|_| anyhow::anyhow!("contacts page loader panicked"))?
    })?;

    app.replace_page(next_page);
    Ok(())
}

fn draw_contacts(frame: &mut Frame, app: &mut ContactsTui) {
    if let ContactsView::Detail(overview) = &mut app.view {
        draw_contact_overview(frame, overview, app.status.as_deref());
        return;
    }

    let area = frame.area();
    let shell = Block::default()
        .title(Span::styled(
            " Contacts ",
            Style::default().fg(Color::White),
        ))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(TURQUOISE));
    frame.render_widget(shell, area);

    let inner = area.inner(Margin {
        horizontal: 1,
        vertical: 1,
    });
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(8), Constraint::Length(3)])
        .split(inner);
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(68), Constraint::Percentage(32)])
        .split(vertical[0]);
    let side_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(8), Constraint::Length(9)])
        .split(chunks[1]);

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
        Row::new(vec!["Full name", "Email", "Created (UTC)", "Labels"])
            .style(
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            )
            .bottom_margin(1),
    )
    .block(
        Block::default()
            .title(format!(" Contacts ({} shown) ", app.page.contacts.len()))
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
    frame.render_widget(details, side_chunks[0]);
    frame.render_widget(contacts_legend(), side_chunks[1]);

    frame.render_widget(
        contacts_footer(&app.page, app.status.as_deref()),
        vertical[1],
    );
}

fn draw_contact_overview(frame: &mut Frame, overview: &mut ContactOverview, status: Option<&str>) {
    let area = frame.area();
    let compact = area.width < 100 || area.height < 24;
    let mut shell = Block::default()
        .title(Span::styled(
            " Contact Overview ",
            Style::default().fg(Color::White),
        ))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(TURQUOISE));
    if compact {
        shell = shell.title_bottom(" ↑/↓ j/k scroll | PgUp/PgDn | b back | q/esc quit ");
    }
    frame.render_widget(shell, area);

    let inner = area.inner(Margin {
        horizontal: 1,
        vertical: 1,
    });
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(if inner.height >= 12 { 4 } else { 0 }),
            Constraint::Length(if compact { 0 } else { 3 }),
            Constraint::Min(1),
        ])
        .split(inner);
    frame.render_widget(contact_header(&overview.contact), vertical[0]);
    frame.render_widget(contact_tabs(), vertical[1]);

    if compact {
        draw_contact_profile_fields(frame, overview, vertical[2]);
        return;
    }

    let main = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(66), Constraint::Percentage(34)])
        .split(vertical[2]);
    let right = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(3), Constraint::Length(10)])
        .split(main[1]);

    draw_contact_profile_fields(frame, overview, main[0]);
    frame.render_widget(contact_activity_placeholder(), right[0]);
    frame.render_widget(contact_detail_legend(status), right[1]);
}

fn draw_contact_profile_fields(
    frame: &mut Frame,
    overview: &mut ContactOverview,
    area: ratatui::layout::Rect,
) {
    let block = Block::default()
        .title(format!(" Contact #{} ", overview.detail.id))
        .borders(Borders::ALL);
    let inner = block.inner(area);
    let fields = contact_profile_fields(&overview.detail);
    let line_count = fields.line_count(inner.width);
    overview.viewport_height = inner.height;
    overview.max_scroll = if inner.is_empty() {
        0
    } else {
        line_count
            .saturating_sub(usize::from(inner.height))
            .try_into()
            .unwrap_or(u16::MAX)
    };
    overview.scroll = overview.scroll.min(overview.max_scroll);

    let block = block.title_bottom(format!(
        " Lines {}–{}/{} ",
        usize::from(overview.scroll)
            .saturating_add(1)
            .min(line_count),
        (usize::from(overview.scroll) + usize::from(inner.height)).min(line_count),
        line_count,
    ));
    frame.render_widget(fields.block(block).scroll((overview.scroll, 0)), area);
}

fn contact_header(contact: &ContactRow) -> Paragraph<'static> {
    let labels = if contact.labels.is_empty() {
        String::new()
    } else {
        format!("  [{}]", contact.labels.join(", "))
    };

    Paragraph::new(vec![
        Line::from(vec![
            Span::styled(
                format!(" {} ", initials(&contact.full_name)),
                Style::default()
                    .fg(Color::Black)
                    .bg(Color::Rgb(230, 170, 90))
                    .add_modifier(Modifier::BOLD),
            ),
            Span::from("  "),
            Span::styled(
                non_empty(&contact.full_name, "Unnamed contact"),
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(labels, Style::default().fg(Color::Gray)),
        ]),
        Line::from(format!("    {}", non_empty(&contact.email, "-"))),
    ])
    .block(Block::default().borders(Borders::ALL))
}

fn contact_tabs() -> Paragraph<'static> {
    Paragraph::new(Line::from(vec![
        Span::styled(
            " Overview ",
            Style::default().fg(TURQUOISE).add_modifier(Modifier::BOLD),
        ),
        Span::from(" Organisations  Notes  Responses  Calls  Conversations  Messages  Campaigns "),
    ]))
    .block(Block::default().borders(Borders::BOTTOM))
}

fn contact_profile_fields(detail: &ContactDetail) -> Paragraph<'static> {
    let mut lines = vec![Line::from(Span::styled(
        "Profile",
        Style::default()
            .fg(Color::White)
            .add_modifier(Modifier::BOLD),
    ))];
    lines.push(Line::from(""));

    for field in &detail.fields {
        lines.push(Line::from(Span::styled(
            field.label.clone(),
            Style::default().fg(TURQUOISE).add_modifier(Modifier::BOLD),
        )));
        lines.extend(
            non_empty(&field.value, "-")
                .lines()
                .map(|line| Line::from(line.to_string())),
        );
        lines.push(Line::from(""));
    }

    Paragraph::new(lines).wrap(Wrap { trim: false })
}

fn contact_activity_placeholder() -> Paragraph<'static> {
    Paragraph::new(vec![
        Line::from(Span::styled(
            "Activity",
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from("Activity timeline is not loaded yet."),
        Line::from("The profile values on the left come from current values."),
    ])
    .block(Block::default().title(" Activity ").borders(Borders::ALL))
    .wrap(Wrap { trim: true })
}

fn contact_detail_legend(status: Option<&str>) -> Paragraph<'static> {
    let status = status.unwrap_or("Viewing contact overview.");

    Paragraph::new(vec![
        legend_line("Scroll", "up/down or j/k"),
        legend_line("Page", "PageUp/PageDown"),
        legend_line("Jump", "Home/End"),
        legend_line("Back", "b, left, or backspace"),
        legend_line("Quit", "q or esc"),
        Line::from(""),
        Line::from(Span::styled(
            status.to_string(),
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        )),
    ])
    .block(Block::default().title(" Legend ").borders(Borders::ALL))
}

fn initials(name: &str) -> String {
    let mut initials = name
        .split_whitespace()
        .filter_map(|part| part.chars().next())
        .take(2)
        .collect::<String>()
        .to_uppercase();

    if initials.is_empty() {
        initials.push('?');
    }

    initials
}

fn contacts_footer(page: &ContactsPage, status: Option<&str>) -> Paragraph<'static> {
    let total_results = page
        .pagination
        .total_results
        .map(|value| value.to_string())
        .unwrap_or_else(|| "unknown".to_string());
    let total_pages = page
        .pagination
        .total_pages
        .map(|value| value.to_string())
        .unwrap_or_else(|| "?".to_string());
    let per_page_options = CONTACTS_PER_PAGE_OPTIONS
        .iter()
        .map(|option| {
            if *option == page.pagination.per_page {
                format!("[{option}]")
            } else {
                option.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("/");

    let mut spans = vec![
        Span::styled(
            format!("Found {total_results} results. "),
            Style::default().fg(Color::Gray),
        ),
        Span::styled(
            format!("Showing {per_page_options} per page. "),
            Style::default().fg(Color::Gray),
        ),
        Span::styled(
            format!("Page {} of {total_pages}. ", page.pagination.page),
            Style::default().fg(TURQUOISE).add_modifier(Modifier::BOLD),
        ),
    ];

    if let Some(status) = status {
        spans.push(Span::styled(
            status.to_string(),
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        ));
    }

    Paragraph::new(Line::from(spans)).block(Block::default().borders(Borders::ALL))
}

fn contacts_legend() -> Paragraph<'static> {
    Paragraph::new(vec![
        legend_line("Move", "up/down or j/k"),
        legend_line("Page", "left/right or h/l"),
        legend_line("Per page", "+/- or [/], 1/2/3"),
        legend_line("Select", "enter"),
        legend_line("Quit", "q or esc"),
    ])
    .block(Block::default().title(" Legend ").borders(Borders::ALL))
}

fn legend_line(label: &'static str, value: &'static str) -> Line<'static> {
    Line::from(vec![
        Span::styled(
            format!("{label}: "),
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::from(value),
    ])
}

fn next_per_page_value(current: u32) -> u32 {
    CONTACTS_PER_PAGE_OPTIONS
        .iter()
        .copied()
        .find(|option| *option > current)
        .unwrap_or(CONTACTS_PER_PAGE_OPTIONS[0])
}

fn previous_per_page_value(current: u32) -> u32 {
    CONTACTS_PER_PAGE_OPTIONS
        .iter()
        .rev()
        .copied()
        .find(|option| *option < current)
        .unwrap_or(*CONTACTS_PER_PAGE_OPTIONS.last().unwrap_or(&current))
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
            detail_line("Created (UTC)", &contact.created_at),
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
    use crate::contacts::{ContactFieldValue, ContactsPagination};
    use ratatui::backend::TestBackend;

    fn render_contacts(app: &mut ContactsTui, width: u16, height: u16) -> String {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal.draw(|frame| draw_contacts(frame, app)).unwrap();
        let buffer = terminal.backend().buffer();
        (0..height)
            .map(|y| {
                (0..width)
                    .map(|x| buffer[(x, y)].symbol())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn overview(app: &ContactsTui) -> &ContactOverview {
        let ContactsView::Detail(overview) = &app.view else {
            panic!("expected contact detail view");
        };
        overview
    }

    fn contacts_page() -> ContactsPage {
        ContactsPage {
            pagination: ContactsPagination {
                page: 1,
                per_page: 15,
                total_results: Some(2),
                total_pages: Some(3),
            },
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

        assert_eq!(app.selected_contact().unwrap().id, "2");
    }

    #[test]
    fn contacts_page_reports_pagination_availability() {
        let app = ContactsTui::new(contacts_page());

        assert!(!app.can_go_previous());
        assert!(app.can_go_next());
    }

    #[test]
    fn contacts_page_cycles_page_size_options() {
        let app = ContactsTui::new(contacts_page());

        assert_eq!(app.next_per_page(), 30);
        assert_eq!(app.previous_per_page(), 50);
        assert_eq!(next_per_page_value(30), 50);
        assert_eq!(next_per_page_value(50), 15);
        assert_eq!(previous_per_page_value(30), 15);
    }

    #[test]
    fn contacts_loading_status_uses_spinner_frames() {
        let mut app = ContactsTui::new(contacts_page());

        app.set_loading_status(2);
        assert_eq!(app.status.as_deref(), Some("| Loading page 2..."));
        app.set_loading_status(2);
        assert_eq!(app.status.as_deref(), Some("/ Loading page 2..."));
        app.clear_status();
        assert_eq!(app.status, None);
    }

    #[test]
    fn detail_scroll_reaches_fields_after_twenty_four_without_moving_list_selection() {
        let mut app = ContactsTui::new(contacts_page());
        app.next();
        let contact = app.selected_contact().unwrap().clone();
        let detail = ContactDetail {
            id: contact.id.clone(),
            fields: (1..=30)
                .map(|index| ContactFieldValue {
                    label: format!("Field {index:02}"),
                    value: format!("value-{index:02}"),
                })
                .collect(),
        };
        app.show_detail(contact.clone(), detail.clone());

        let initial = render_contacts(&mut app, 60, 12);
        assert!(initial.contains("Field 01"));
        assert!(!initial.contains("Field 30"));
        assert!(initial.contains("j/k scroll"));

        app.next();
        assert_eq!(overview(&app).scroll, 1);
        app.previous();
        assert_eq!(overview(&app).scroll, 0);
        app.scroll_detail_page(true);
        assert_eq!(overview(&app).scroll, overview(&app).viewport_height);
        app.scroll_detail_page(false);
        assert_eq!(overview(&app).scroll, 0);
        app.scroll_detail_to_end(true);

        let last = render_contacts(&mut app, 60, 12);
        assert!(last.contains("Field 30"), "{last}");
        assert!(last.contains("value-30"), "{last}");
        assert_eq!(app.selected_index(), Some(1));
        let end = overview(&app).scroll;
        app.next();
        app.scroll_detail_page(true);
        assert_eq!(overview(&app).scroll, end);

        app.show_list();
        assert!(!app.is_detail_view());
        assert_eq!(app.selected_index(), Some(1));
        app.show_detail(contact, detail);
        assert_eq!(overview(&app).scroll, 0);
    }

    #[test]
    fn detail_scroll_reaches_wrapped_and_multiline_values_on_short_terminals() {
        let mut app = ContactsTui::new(contacts_page());
        let contact = app.selected_contact().unwrap().clone();
        app.show_detail(
            contact,
            ContactDetail {
                id: "1".to_string(),
                fields: vec![ContactFieldValue {
                    label: "Notes".to_string(),
                    value: format!(
                        "FIRST LINE\n{} WRAPPED END\nLAST EXPLICIT LINE",
                        "a long value with spaces ".repeat(30),
                    ),
                }],
            },
        );

        let initial = render_contacts(&mut app, 40, 8);
        assert!(initial.contains("FIRST LINE"), "{initial}");
        assert!(!initial.contains("LAST EXPLICIT LINE"));
        let max_scroll = overview(&app).max_scroll;
        assert!(
            max_scroll > 20,
            "long values must include their wrapped rows"
        );

        let mut rendered = initial;
        for _ in 0..max_scroll {
            app.next();
            rendered.push_str(&render_contacts(&mut app, 40, 8));
        }
        assert!(rendered.contains("WRAPPED END"));
        assert!(rendered.contains("LAST EXPLICIT LINE"));
        assert_eq!(overview(&app).scroll, max_scroll);
        assert_eq!(app.selected_index(), Some(0));

        app.scroll_detail_to_end(false);
        app.previous();
        app.scroll_detail_page(false);
        assert_eq!(overview(&app).scroll, 0);
    }

    #[test]
    fn detail_scroll_clamps_after_terminal_resize() {
        let mut app = ContactsTui::new(contacts_page());
        let contact = app.selected_contact().unwrap().clone();
        app.show_detail(
            contact,
            ContactDetail {
                id: "1".to_string(),
                fields: vec![ContactFieldValue {
                    label: "Notes".to_string(),
                    value: "a long value ".repeat(25),
                }],
            },
        );

        render_contacts(&mut app, 30, 8);
        app.scroll_detail_to_end(true);
        assert!(overview(&app).scroll > 0);

        let resized = render_contacts(&mut app, 120, 50);
        assert!(resized.contains("Notes"));
        assert_eq!(overview(&app).max_scroll, 0);
        assert_eq!(overview(&app).scroll, 0);
        app.next();
        assert_eq!(overview(&app).scroll, 0);

        render_contacts(&mut app, 2, 2);
        assert_eq!(overview(&app).max_scroll, 0);
        assert_eq!(overview(&app).scroll, 0);
    }
}
