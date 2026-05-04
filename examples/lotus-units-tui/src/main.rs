//! Interactive terminal spreadsheet with numbat-backed unit cells.
//!
//! Arrow keys navigate. Enter starts editing the current cell; Esc
//! cancels, Enter again commits. Starts with a preseeded example so
//! there's something to play with.

use std::io;
use std::sync::Arc;

use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use crossterm::{
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use lotus_core::{cell_id, index_to_col, CellValue, Registry, Sheet};
use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Cell as TuiCell, Paragraph, Row, Table};

const ROWS: u32 = 12;
const COLS: u32 = 6;

struct App {
    sheet: Sheet,
    cursor: (u32, u32), // (row, col), 1-based
    mode: Mode,
    message: String,
}

enum Mode {
    Nav,
    Edit { buffer: String, target: String },
}

impl App {
    fn new() -> Self {
        let mut reg = Registry::default();
        lotus_numbat::install(&mut reg).expect("install numbat handler + CONVERT");
        let mut sheet = Sheet::new_with_registry(Arc::new(reg));
        sheet
            .set_cells(&[
                // col A — arithmetic across mixed units
                ("A1".into(), "3 km".into()),
                ("A2".into(), "2 mi".into()),
                ("A3".into(), "=A1 + A2".into()),
                // col B — derived unit from division
                ("B1".into(), "100 km".into()),
                ("B2".into(), "2 hours".into()),
                ("B3".into(), "=B1 / B2".into()),
                // col C — CONVERT showcase
                ("C1".into(), "100 m/s".into()),
                ("C2".into(), "=CONVERT(C1, \"mph\")".into()),
                ("C3".into(), "=CONVERT(C1, \"km/h\")".into()),
                // col D — unit equality
                ("D1".into(), "1000 m".into()),
                ("D2".into(), "1 km".into()),
                ("D3".into(), "=D1 = D2".into()),
                // col E — chain CONVERT with CONCAT
                ("E1".into(), "5 km".into()),
                ("E2".into(), "=CONVERT(E1, \"feet\")".into()),
                ("E3".into(), "=CONCAT(E1, \" = \", E2)".into()),
                // col F — pure numeric arithmetic still works
                ("F1".into(), "42".into()),
                ("F2".into(), "=F1 * 2".into()),
                ("F3".into(), "=SUM(F1, F2)".into()),
            ])
            .ok();
        App {
            sheet,
            cursor: (1, 1),
            mode: Mode::Nav,
            message: "numbat units + CONVERT loaded · arrows navigate · Enter edit · q quit"
                .into(),
        }
    }

    fn current_id(&self) -> String {
        cell_id(self.cursor.0 - 1, self.cursor.1 - 1)
    }

    fn begin_edit(&mut self) {
        let target = self.current_id();
        let buffer = self
            .sheet
            .get_raw(&target)
            .map(|s| s.to_string())
            .unwrap_or_default();
        self.mode = Mode::Edit { buffer, target };
    }

    fn commit_edit(&mut self) {
        if let Mode::Edit { buffer, target } = std::mem::replace(&mut self.mode, Mode::Nav) {
            let id = target.clone();
            match self.sheet.set_cells(&[(target, buffer)]) {
                Ok(()) => self.message = format!("{id} committed"),
                Err(e) => self.message = format!("{id}: {e}"),
            }
        }
    }

    fn cancel_edit(&mut self) {
        self.mode = Mode::Nav;
        self.message = "edit cancelled".into();
    }

    fn move_cursor(&mut self, dr: i32, dc: i32) {
        let r = (self.cursor.0 as i32 + dr).clamp(1, ROWS as i32) as u32;
        let c = (self.cursor.1 as i32 + dc).clamp(1, COLS as i32) as u32;
        self.cursor = (r, c);
    }
}

fn main() -> io::Result<()> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut term = Terminal::new(backend)?;

    let mut app = App::new();

    loop {
        term.draw(|f| draw(f, &app))?;

        if let Event::Key(key) = event::read()? {
            if key.kind != KeyEventKind::Press {
                continue;
            }
            let quit = match &mut app.mode {
                Mode::Nav => match key.code {
                    KeyCode::Char('q')
                    | KeyCode::Char('c')
                        if key.modifiers.contains(KeyModifiers::CONTROL)
                            || key.code == KeyCode::Char('q') =>
                    {
                        true
                    }
                    KeyCode::Up => {
                        app.move_cursor(-1, 0);
                        false
                    }
                    KeyCode::Down => {
                        app.move_cursor(1, 0);
                        false
                    }
                    KeyCode::Left => {
                        app.move_cursor(0, -1);
                        false
                    }
                    KeyCode::Right => {
                        app.move_cursor(0, 1);
                        false
                    }
                    KeyCode::Enter => {
                        app.begin_edit();
                        false
                    }
                    KeyCode::Delete | KeyCode::Backspace => {
                        let id = app.current_id();
                        let _ = app.sheet.set_cells(&[(id.clone(), "".into())]);
                        app.message = format!("{id} cleared");
                        false
                    }
                    _ => false,
                },
                Mode::Edit { buffer, .. } => match key.code {
                    KeyCode::Enter => {
                        app.commit_edit();
                        false
                    }
                    KeyCode::Esc => {
                        app.cancel_edit();
                        false
                    }
                    KeyCode::Backspace => {
                        buffer.pop();
                        false
                    }
                    KeyCode::Char(c) => {
                        buffer.push(c);
                        false
                    }
                    _ => false,
                },
            };
            if quit {
                break;
            }
        }
    }

    disable_raw_mode()?;
    execute!(term.backend_mut(), LeaveAlternateScreen)?;
    Ok(())
}

fn draw(f: &mut Frame<'_>, app: &App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(2),
            Constraint::Min(1),
            Constraint::Length(3),
            Constraint::Length(1),
        ])
        .split(f.area());

    // Title
    f.render_widget(
        Paragraph::new("  lotus-units-tui — numbat-backed unit spreadsheet").bold(),
        chunks[0],
    );

    // Formula bar
    let cur = app.current_id();
    let raw = app.sheet.get_raw(&cur).unwrap_or("").to_string();
    let formula_line = match &app.mode {
        Mode::Nav => format!("  {cur}:  {raw}"),
        Mode::Edit { buffer, .. } => format!("  {cur} (editing):  {buffer}_"),
    };
    f.render_widget(
        Paragraph::new(formula_line).block(Block::default().borders(Borders::BOTTOM)),
        chunks[1],
    );

    // Grid
    let header_cells = std::iter::once(TuiCell::from("")).chain(
        (1..=COLS).map(|c| TuiCell::from(index_to_col(c - 1)).style(Style::default().bold())),
    );
    let header = Row::new(header_cells).style(Style::default().fg(Color::DarkGray));

    let rows: Vec<Row> = (1..=ROWS)
        .map(|r| {
            let mut cells = Vec::with_capacity(COLS as usize + 1);
            cells.push(TuiCell::from(r.to_string()).style(Style::default().fg(Color::DarkGray)));
            for c in 1..=COLS {
                let id = cell_id(r - 1, c - 1);
                let v = app.sheet.get(&id);
                let text = format_cell(&v);
                let style = cell_style(&v, (r, c) == app.cursor);
                cells.push(TuiCell::from(text).style(style));
            }
            Row::new(cells)
        })
        .collect();

    let widths = std::iter::once(Constraint::Length(3))
        .chain((0..COLS).map(|_| Constraint::Length(18)))
        .collect::<Vec<_>>();

    let table = Table::new(rows, widths)
        .header(header)
        .block(Block::default().borders(Borders::ALL).title(" grid "));
    f.render_widget(table, chunks[2]);

    // Help panel
    let help = Paragraph::new(
        "  ↑↓←→ navigate · Enter edit · Esc cancel · Del clear · q quit\n  \
         Try:  3 km   =A1+A2   =CONVERT(A1, \"mph\")   =B1/B2   =D1=D2",
    )
    .block(Block::default().borders(Borders::ALL).title(" help "));
    f.render_widget(help, chunks[3]);

    // Status
    f.render_widget(
        Paragraph::new(format!("  {}", app.message)).style(Style::default().fg(Color::DarkGray)),
        chunks[4],
    );
}

fn format_cell(v: &CellValue) -> String {
    match v {
        CellValue::Empty => String::new(),
        CellValue::Number(n) => format!("{n}"),
        CellValue::String(s) => s.clone(),
        CellValue::Boolean(b) => if *b { "TRUE" } else { "FALSE" }.to_string(),
        CellValue::Error(e) => e.to_string(),
        CellValue::Custom(cv) => cv.data.clone(),
    }
}

fn cell_style(v: &CellValue, is_cursor: bool) -> Style {
    let base = match v {
        CellValue::Custom(_) => Style::default().fg(Color::Cyan),
        CellValue::Number(_) => Style::default().fg(Color::Yellow),
        CellValue::Boolean(_) => Style::default().fg(Color::Magenta),
        CellValue::String(s) if s.starts_with('#') && s.ends_with('!') => {
            Style::default().fg(Color::Red)
        }
        _ => Style::default(),
    };
    if is_cursor {
        base.bg(Color::DarkGray).bold()
    } else {
        base
    }
}
