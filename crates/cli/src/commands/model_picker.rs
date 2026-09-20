//! Bounded terminal viewport over a searchable provider catalog.
use anyhow::{Result, anyhow};
use crossterm::{
    event::{
        self, DisableBracketedPaste, EnableBracketedPaste, Event, KeyCode, KeyEventKind,
        KeyModifiers,
    },
    execute,
    terminal::{self, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    Terminal,
    backend::CrosstermBackend,
    layout::{Constraint, Layout},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
};
use s_code_model_gateway::onboarding::DiscoveredModel;
use std::io::{self, IsTerminal, Write};

const QUERY_LIMIT: usize = 256;
const PAGE_SIZE: usize = 10;

struct Picker<'a> {
    models: &'a [DiscoveredModel],
    search: Vec<String>,
    matches: Vec<usize>,
    query: String,
    selected: usize,
}
impl<'a> Picker<'a> {
    fn new(models: &'a [DiscoveredModel]) -> Self {
        Self {
            models,
            search: models
                .iter()
                .map(|m| format!("{} {}", m.id, m.name).to_lowercase())
                .collect(),
            matches: (0..models.len()).collect(),
            query: String::new(),
            selected: 0,
        }
    }
    fn filter(&mut self) {
        let query = self.query.to_lowercase();
        let words: Vec<_> = query
            .split_whitespace()
            .filter(|word| *word != "*")
            .collect();
        self.matches = self
            .search
            .iter()
            .enumerate()
            .filter(|(_, text)| words.iter().all(|word| text.contains(word)))
            .map(|(index, _)| index)
            .collect();
        self.selected = 0;
    }
    fn append(&mut self, text: &str) {
        let remaining = QUERY_LIMIT.saturating_sub(self.query.chars().count());
        self.query
            .extend(text.chars().filter(|c| !c.is_control()).take(remaining));
        self.filter();
    }
    fn move_by(&mut self, delta: isize) {
        self.selected = self
            .selected
            .saturating_add_signed(delta)
            .min(self.matches.len().saturating_sub(1));
    }
    fn chosen(&self) -> Option<&'a DiscoveredModel> {
        self.matches
            .get(self.selected)
            .map(|index| &self.models[*index])
    }
    fn range(&self, rows: usize) -> std::ops::Range<usize> {
        let start = self.selected / rows * rows;
        start..(start + rows).min(self.matches.len())
    }
}

struct Screen;
impl Drop for Screen {
    fn drop(&mut self) {
        let _ = execute!(io::stdout(), DisableBracketedPaste, LeaveAlternateScreen);
        let _ = terminal::disable_raw_mode();
    }
}

pub(super) fn choose(models: &[DiscoveredModel]) -> Result<Option<String>> {
    if models.is_empty() {
        return Err(anyhow!("No models are available"));
    }
    if !io::stdin().is_terminal()
        || !io::stdout().is_terminal()
        || std::env::var("TERM").as_deref() == Ok("dumb")
    {
        return choose_plain(models);
    }
    terminal::enable_raw_mode()?;
    let _screen = Screen;
    execute!(io::stdout(), EnterAlternateScreen, EnableBracketedPaste)?;
    let mut terminal = Terminal::new(CrosstermBackend::new(io::stdout()))?;
    let mut picker = Picker::new(models);
    let colorful = super::setup_style::enabled();
    let accent = if colorful {
        Style::default().fg(Color::Cyan)
    } else {
        Style::default()
    };
    loop {
        let mut rows = PAGE_SIZE;
        terminal.draw(|frame| {
            let [heading, search, body, help] = Layout::vertical([
                Constraint::Length(2), Constraint::Length(3), Constraint::Min(1), Constraint::Length(3),
            ]).areas(frame.area());
            rows = usize::from(body.height).clamp(1, PAGE_SIZE);
            frame.render_widget(Paragraph::new(if colorful { "🤖 03 / 03 · Meet your coding partner\nAPI access checked. Type to filter by name or ID." } else { "03 / 03 · Choose a model\nAPI access checked. Type to filter by name or ID." }).style(accent), heading);
            frame.render_widget(Paragraph::new(picker.query.as_str()).block(Block::default().borders(Borders::ALL).title("Search models").border_style(accent)), search);
            let range = picker.range(rows);
            let lines: Vec<Line<'_>> = if picker.matches.is_empty() {
                vec![Line::from("No matches. Backspace to edit; Ctrl+U to clear.")]
            } else {
                range.clone().map(|index| {
                    let model = &models[picker.matches[index]];
                    let label = if model.id == model.name { model.id.clone() } else { format!("{} · {}", model.id, model.name) };
                    if index == picker.selected {
                        Line::from(Span::styled(format!("> {label}"), accent.add_modifier(Modifier::BOLD)))
                    } else { Line::from(format!("  {label}")) }
                }).collect()
            };
            frame.render_widget(Paragraph::new(lines), body);
            frame.render_widget(Paragraph::new(format!(
                "{} matches / {} models · showing {}–{}\n↑/↓ move · PgUp/PgDn page · Home/End first/last\nEnter select · Ctrl+U clear · Esc cancel",
                picker.matches.len(), models.len(), if range.is_empty() { 0 } else { range.start + 1 }, range.end,
            )), help);
        })?;
        match event::read()? {
            Event::Paste(text) => picker.append(&text),
            Event::Key(key) if key.kind != KeyEventKind::Release => match key.code {
                KeyCode::Esc => return Ok(None),
                KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    return Ok(None);
                }
                KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    picker.query.clear();
                    picker.filter();
                }
                KeyCode::Char(c)
                    if !key
                        .modifiers
                        .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
                {
                    picker.append(&c.to_string())
                }
                KeyCode::Backspace => {
                    picker.query.pop();
                    picker.filter();
                }
                KeyCode::Up => picker.move_by(-1),
                KeyCode::Down => picker.move_by(1),
                KeyCode::PageUp => picker.move_by(-(rows as isize)),
                KeyCode::PageDown => picker.move_by(rows as isize),
                KeyCode::Home => picker.selected = 0,
                KeyCode::End => picker.selected = picker.matches.len().saturating_sub(1),
                KeyCode::Enter => {
                    if let Some(model) = picker.chosen() {
                        return Ok(Some(model.id.clone()));
                    }
                }
                _ => {}
            },
            _ => {}
        }
    }
}

/// Accessible line-oriented fallback for dumb terminals and redirected input.
fn choose_plain(models: &[DiscoveredModel]) -> Result<Option<String>> {
    let mut picker = Picker::new(models);
    loop {
        let range = picker.range(PAGE_SIZE);
        println!(
            "\n{} matches / {} models",
            picker.matches.len(),
            models.len()
        );
        for index in range.clone() {
            let model = &models[picker.matches[index]];
            println!(
                "  {}  {} · {}",
                index - range.start + 1,
                model.id,
                model.name
            );
        }
        if range.is_empty() {
            println!("No matches. Enter / to clear the search.");
        }
        print!("Model number or exact ID; /text search; n/p page; q cancel: ");
        io::stdout().flush()?;
        let mut input = String::new();
        if io::stdin().read_line(&mut input)? == 0 {
            return Ok(None);
        }
        let input = input.trim();
        match input {
            "q" => return Ok(None),
            "n" => picker.move_by(PAGE_SIZE as isize),
            "p" => picker.move_by(-(PAGE_SIZE as isize)),
            value if value.starts_with('/') => {
                picker.query.clear();
                picker.append(&value[1..]);
            }
            value => {
                let chosen = value
                    .parse::<usize>()
                    .ok()
                    .and_then(|n| n.checked_sub(1))
                    .filter(|n| *n < range.len())
                    .map(|n| &models[picker.matches[range.start + n]])
                    .or_else(|| models.iter().find(|model| model.id == value));
                if let Some(model) = chosen {
                    return Ok(Some(model.id.clone()));
                }
                println!("Choose a listed model, or type / followed by a search.");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn models() -> Vec<DiscoveredModel> {
        (0..5000)
            .map(|i| DiscoveredModel {
                id: format!("provider/model-{i:04}"),
                name: format!("Coding 模型 {i}"),
            })
            .collect()
    }
    #[test]
    fn large_catalog_supports_last_page_and_name_search() {
        let models = models();
        let mut picker = Picker::new(&models);
        picker.move_by(4999);
        assert_eq!(picker.range(10), 4990..5000);
        assert_eq!(picker.chosen().unwrap().id, "provider/model-4999");
        picker.append("CODING 模型 4999");
        assert_eq!(picker.matches, vec![4999]);
        assert_eq!(picker.range(10), 0..1);
    }
    #[test]
    fn empty_results_are_not_selectable_and_clear_recovers() {
        let models = models();
        let mut picker = Picker::new(&models);
        picker.append("not-a-model");
        picker.move_by(10);
        assert!(picker.chosen().is_none());
        assert!(picker.range(10).is_empty());
        picker.query.clear();
        picker.filter();
        picker.move_by(-10);
        assert_eq!(picker.chosen().unwrap().id, "provider/model-0000");
        picker.move_by(10);
        assert_eq!(picker.range(10), 10..20);
    }
    #[test]
    fn paste_is_bounded_and_drops_control_characters() {
        let models = models();
        let mut picker = Picker::new(&models);
        picker.append(&format!("\x1b\n{}", "界".repeat(5000)));
        assert_eq!(picker.query.chars().count(), QUERY_LIMIT);
        assert!(!picker.query.chars().any(char::is_control));
    }
}
