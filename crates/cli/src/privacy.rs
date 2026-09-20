use crate::{Id, KeyCode, api::Api, state::App};
use ratatui::{
    layout::{Constraint, Layout},
    style::{Modifier, Style},
    widgets::{Block, Borders, Paragraph, Wrap},
};
use s_code_protocol::{PrivacyPage, PrivacyRequest};
use std::cell::Cell;

pub(crate) struct PrivacyView {
    pub session_id: Id,
    pub page: PrivacyPage,
    pub scroll: usize,
    pub max_scroll: Cell<usize>,
    pub notice: String,
}

impl PrivacyView {
    pub fn key(&mut self, key: KeyCode) {
        self.scroll = match key {
            KeyCode::Up | KeyCode::Char('k') => self.scroll.saturating_sub(1),
            KeyCode::Down | KeyCode::Char('j') => self.scroll.saturating_add(1),
            KeyCode::PageUp => self.scroll.saturating_sub(15),
            KeyCode::PageDown => self.scroll.saturating_add(15),
            KeyCode::Home => 0,
            KeyCode::End => self.max_scroll.get(),
            _ => self.scroll,
        }
        .min(self.max_scroll.get());
    }

    pub fn update(&mut self, mut request: PrivacyRequest, _sequence: u64) {
        if let Some(existing) = self
            .page
            .requests
            .iter_mut()
            .find(|old| old.id == request.id)
        {
            request.sequence = existing.sequence;
            *existing = request;
        } else {
            // Keep pagination stable. Refresh returns to the newest page.
            self.notice = "New model activity · press r to refresh".into();
        }
    }

    fn text(&self) -> String {
        let mut lines = vec![
            "FILES & MODEL REQUESTS".into(),
            "Recorded at the model HTTP boundary. Accepted means the endpoint returned success, not that generation finished or data was deleted.".into(),
            "Shell / MCP traffic is not monitored. Tool output, pasted text and summaries may include untraceable file content. Earlier versions have no records.".into(),
            String::new(),
        ];
        if self.page.requests.is_empty() {
            lines.push("No recorded model requests. This does not prove that no data was sent before recording was available.".into());
        }
        for request in &self.page.requests {
            lines.push(format!(
                "{} · {}",
                request.started_at.format("%Y-%m-%d %H:%M:%S UTC"),
                if request.purpose == "session_title" {
                    "Conversation title"
                } else {
                    "Agent request"
                }
            ));
            lines.push(format!(
                "{} → {} · {} bytes",
                request.model, request.destination, request.request_bytes
            ));
            lines.push(
                match request.status.as_str() {
                    "accepted" => "Accepted by endpoint",
                    "rejected" => "Rejected by endpoint · data may have been received",
                    "connection_error" => "Connection error · delivery unknown",
                    _ => "Request started · delivery not confirmed",
                }
                .into(),
            );
            if request.sources.is_empty() {
                lines.push(
                    "  No individually attributed files; other context may contain file data."
                        .into(),
                );
            }
            for source in &request.sources {
                lines.push(format!(
                    "  {} · {}{}",
                    source.source,
                    source.kind,
                    if source.partial {
                        " · excerpt / partial context"
                    } else {
                        ""
                    }
                ));
            }
            if !request.unattributed.is_empty() {
                lines.push(format!(
                    "Other context: {}",
                    request.unattributed.join(" · ")
                ));
            }
            lines.push(String::new());
        }
        lines.join("\n")
    }
}

pub(crate) async fn load(api: &Api, app: &mut App, older: bool) {
    let Some(session_id) = app.current().map(|session| session.id.clone()) else {
        app.status = "select a session before inspecting privacy".into();
        return;
    };
    let before = if older {
        let Some(cursor) = app.privacy.as_ref().and_then(|view| view.page.next_before) else {
            return;
        };
        Some(cursor)
    } else {
        None
    };
    match api.privacy(&session_id, before).await {
        Ok(page) => {
            app.privacy = Some(PrivacyView {
                session_id,
                page,
                scroll: 0,
                max_scroll: Cell::new(0),
                notice: String::new(),
            })
        }
        Err(error) => {
            if let Some(view) = &mut app.privacy {
                view.notice = format!("Could not load privacy records: {error}");
            } else {
                app.activity
                    .push_front(format!("× Could not load privacy records: {error}"));
            }
        }
    }
}

pub(crate) fn render(frame: &mut ratatui::Frame<'_>, view: &PrivacyView) {
    let area = frame.area();
    let sections = Layout::vertical([Constraint::Min(1), Constraint::Length(2)]).split(area);
    let block = Block::default()
        .title(" Privacy · current conversation ")
        .borders(Borders::ALL);
    let inner = block.inner(sections[0]);
    let paragraph = Paragraph::new(view.text()).wrap(Wrap { trim: false });
    let max_scroll = paragraph
        .line_count(inner.width)
        .saturating_sub(inner.height as usize)
        .min(u16::MAX as usize);
    view.max_scroll.set(max_scroll);
    frame.render_widget(block, sections[0]);
    frame.render_widget(
        paragraph.scroll((view.scroll.min(max_scroll) as u16, 0)),
        inner,
    );
    let footer = format!(
        "Esc close · ↑↓/Pg scroll · r refresh{}\n{}",
        if view.page.next_before.is_some() {
            " · n older requests"
        } else {
            ""
        },
        view.notice
    );
    frame.render_widget(
        Paragraph::new(footer).style(Style::default().add_modifier(Modifier::DIM)),
        sections[1],
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::{Terminal, backend::TestBackend};
    use s_code_protocol::PrivacySource;

    #[test]
    fn privacy_page_scrolls_to_last_source_and_does_not_claim_empty_means_private() {
        let mut view = PrivacyView {
            session_id: Id("session".into()),
            page: PrivacyPage {
                requests: vec![],
                next_before: Some(1),
            },
            scroll: 0,
            max_scroll: Cell::new(0),
            notice: String::new(),
        };
        assert!(view.text().contains("does not prove"));
        view.page.requests.push(PrivacyRequest {
            id: Id("request".into()),
            sequence: 1,
            turn_id: Id("turn".into()),
            started_at: chrono::Utc::now(),
            destination: "https://example.test".into(),
            model: "model".into(),
            purpose: "agent".into(),
            status: "connection_error".into(),
            request_bytes: 500,
            sources: (0..80)
                .map(|index| PrivacySource {
                    source: format!("src/very-long-file-name-{index}.rs"),
                    kind: "file excerpt".into(),
                    content_bytes: 10,
                    partial: true,
                })
                .collect(),
            unattributed: vec![],
        });
        assert!(view.text().contains("delivery unknown"));
        let mut terminal = Terminal::new(TestBackend::new(60, 18)).unwrap();
        terminal.draw(|frame| render(frame, &view)).unwrap();
        assert!(view.max_scroll.get() > 80);
        view.key(KeyCode::End);
        terminal.draw(|frame| render(frame, &view)).unwrap();
        let screen: String = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        assert!(screen.contains("file-name-79.rs"));
        assert!(screen.contains("n older requests"));
        view.key(KeyCode::Home);
        assert_eq!(view.scroll, 0);
    }
}
