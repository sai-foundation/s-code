use crate::{
    commands::{
        interactive::value_text,
        slash::{matching_slash_commands, slash_command_menu_visible},
    },
    state::{
        App, ArtifactActivity, CliTheme, InputMode, Keymap, NoticeActivity, PickerKind,
        PlanActivity, QuestionActivity, StatuslineMode, ToolActivity, ToolActivityState,
        ToolProgress, VimMode,
    },
};
use opencoding_protocol::{
    Id, Message, PermissionMode, QuestionStatus, SessionGoalStatus, TranscriptPlanStepStatus,
    TurnInputMode,
};
use ratatui::{
    layout::{Constraint, Direction, Layout},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Wrap},
};
use std::collections::HashSet;

mod markdown;
mod syntax;

use markdown::markdown_lines;

const ORANGE: Color = Color::Rgb(255, 107, 0);
const ASSISTANT_HEADING: Color = Color::Rgb(148, 148, 148);

fn theme_color(theme: CliTheme, color: Color) -> Color {
    match theme {
        CliTheme::Dark => color,
        CliTheme::Light => match color {
            Color::DarkGray => Color::Gray,
            Color::White => Color::Black,
            other => other,
        },
        CliTheme::NoColor => Color::Reset,
    }
}

fn apply_theme(lines: &mut [Line<'static>], theme: CliTheme) {
    if theme == CliTheme::Dark {
        return;
    }
    for line in lines {
        for span in &mut line.spans {
            if let Some(color) = span.style.fg {
                span.style.fg = Some(theme_color(theme, color));
            }
        }
    }
}

fn tool_activity_text(item: &ToolActivity) -> String {
    let base = match item.state {
        ToolActivityState::Preparing => format!("● Preparing {}", item.display),
        ToolActivityState::Running => format!("● Running {}", item.display),
        ToolActivityState::AwaitingApproval => {
            format!("! Approval required · {}", item.display)
        }
        ToolActivityState::Completed => format!("✓ {}", item.display),
        ToolActivityState::Failed => format!("× {} failed", item.display),
        ToolActivityState::Denied => format!("○ {} rejected", item.display),
    };
    match &item.progress {
        Some(progress) => format!("{base} · {}", tool_progress_text(progress)),
        None => base,
    }
}

fn tool_progress_text(progress: &ToolProgress) -> String {
    let amount = match progress.total {
        Some(total) if total > 0.0 => {
            format!(
                "{:.0}%",
                (progress.progress / total * 100.0).clamp(0.0, 100.0)
            )
        }
        _ => format!("{:.0}", progress.progress),
    };
    progress
        .message
        .as_deref()
        .filter(|message| !message.is_empty())
        .map_or(amount.clone(), |message| format!("{amount} · {message}"))
}

fn append_tool_activity_item(lines: &mut Vec<Line<'static>>, item: &ToolActivity) {
    let text = tool_activity_text(item);
    let color = match item.state {
        ToolActivityState::Failed => Color::Red,
        ToolActivityState::AwaitingApproval => Color::Yellow,
        ToolActivityState::Preparing | ToolActivityState::Running => ORANGE,
        ToolActivityState::Completed | ToolActivityState::Denied => Color::Green,
    };
    lines.push(Line::from(Span::styled(
        format!(" {text}"),
        Style::default().fg(color),
    )));
}

fn append_tool_activity(
    lines: &mut Vec<Line<'static>>,
    app: &App,
    turn_id: &Id,
    rendered_turns: &mut HashSet<Id>,
) {
    if !rendered_turns.insert(turn_id.clone()) {
        return;
    }
    for item in app
        .tool_activity
        .iter()
        .filter(|item| item.turn_id.as_ref() == Some(turn_id))
    {
        append_tool_activity_item(lines, item);
    }
}

fn append_plan_item(lines: &mut Vec<Line<'static>>, plan: &PlanActivity) {
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        format!(
            " Plan · {}",
            plan.title.as_deref().unwrap_or("Implementation")
        ),
        Style::default().fg(ORANGE).add_modifier(Modifier::BOLD),
    )));
    lines.extend(plan.steps.iter().map(|step| {
        let (marker, color) = match step.status {
            TranscriptPlanStepStatus::Pending => ("○", Color::DarkGray),
            TranscriptPlanStepStatus::InProgress => ("●", ORANGE),
            TranscriptPlanStepStatus::Completed => ("✓", Color::Green),
        };
        Line::from(vec![
            Span::styled(format!(" {marker} "), Style::default().fg(color)),
            Span::raw(step.text.clone()),
        ])
    }));
}

fn append_plan(
    lines: &mut Vec<Line<'static>>,
    app: &App,
    turn_id: &Id,
    rendered_plans: &mut HashSet<Id>,
) {
    for plan in app.plans.iter().filter(|plan| &plan.turn_id == turn_id) {
        if !rendered_plans.insert(plan.id.clone()) {
            continue;
        }
        append_plan_item(lines, plan);
    }
}

fn append_question(lines: &mut Vec<Line<'static>>, question: &QuestionActivity) {
    lines.push(Line::from(""));
    let state = match question.status {
        QuestionStatus::Pending => "Answer needed",
        QuestionStatus::Answered => "Answered",
        QuestionStatus::Expired => "Expired",
        QuestionStatus::Cancelled => "Cancelled",
    };
    lines.push(Line::from(vec![
        Span::styled(
            format!(" {state}"),
            Style::default().fg(ORANGE).add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!(" · {}", question.id.0),
            Style::default().fg(Color::DarkGray),
        ),
    ]));
    for prompt in &question.questions {
        lines.push(Line::from(Span::styled(
            format!(" {} · {}", prompt.header, prompt.question),
            Style::default().add_modifier(Modifier::BOLD),
        )));
        for (index, option) in prompt.options.iter().enumerate() {
            lines.push(Line::from(vec![
                Span::styled(format!("   {}. ", index + 1), Style::default().fg(ORANGE)),
                Span::raw(option.label.clone()),
                Span::styled(
                    format!(" — {}", option.description),
                    Style::default().fg(Color::DarkGray),
                ),
            ]));
        }
        if let Some(answer) = question
            .answers
            .iter()
            .find(|answer| answer.question_id == prompt.id)
        {
            lines.push(Line::from(Span::styled(
                format!("   ✓ {}", answer.answer),
                Style::default().fg(Color::Green),
            )));
        }
    }
    if question.status == QuestionStatus::Pending {
        if let Some(expires_at) = question.expires_at {
            let seconds = (expires_at - chrono::Utc::now()).num_seconds().max(0);
            lines.push(Line::from(Span::styled(
                format!(" Defaults to the recommended options in {seconds}s"),
                Style::default().fg(Color::Yellow),
            )));
        }
        let usage = if question.questions.len() == 1 {
            format!("/answer {} <option or other text>", question.id.0)
        } else {
            format!(
                "/answer {} question_id=answer;question_id=answer",
                question.id.0
            )
        };
        lines.push(Line::from(Span::styled(
            format!(" {usage}"),
            Style::default().fg(Color::DarkGray),
        )));
    }
}

fn append_artifact(lines: &mut Vec<Line<'static>>, artifact: &ArtifactActivity) {
    lines.push(Line::from(""));
    lines.push(Line::from(vec![
        Span::styled(
            " Artifact · ",
            Style::default().fg(ORANGE).add_modifier(Modifier::BOLD),
        ),
        Span::raw(artifact.title.clone()),
        Span::styled(
            format!("  {}", artifact.media_type),
            Style::default().fg(Color::DarkGray),
        ),
    ]));
    lines.push(Line::from(Span::styled(
        format!(" /artifact {}", artifact.artifact_id.0),
        Style::default().fg(Color::DarkGray),
    )));
}

fn append_notices(
    lines: &mut Vec<Line<'static>>,
    app: &App,
    turn_id: &Id,
    rendered_notices: &mut HashSet<Id>,
) {
    for notice in app
        .notices
        .iter()
        .filter(|notice| &notice.turn_id == turn_id)
    {
        if !rendered_notices.insert(notice.item_id.clone()) {
            continue;
        }
        append_notice_item(lines, notice);
    }
}

fn append_notice_item(lines: &mut Vec<Line<'static>>, notice: &NoticeActivity) {
    lines.push(Line::from(vec![
        Span::styled(format!(" {} · ", notice.label), Style::default().fg(ORANGE)),
        Span::styled(notice.detail.clone(), Style::default().fg(Color::DarkGray)),
    ]));
}

fn append_message(lines: &mut Vec<Line<'static>>, app: &App, message: &Message) {
    let (label, style) = if message.role == "user" {
        (
            " You".to_owned(),
            Style::default().fg(ORANGE).add_modifier(Modifier::BOLD),
        )
    } else {
        (
            format!(" {}", app.assistant_alias),
            Style::default()
                .fg(ASSISTANT_HEADING)
                .add_modifier(Modifier::BOLD),
        )
    };
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(label, style)));
    lines.extend(markdown_lines(&value_text(&message.content)));
    if let Some(attachments) = app.message_attachments.get(&message.id) {
        for attachment in attachments {
            lines.push(Line::from(vec![
                Span::styled(" Attached · ", Style::default().fg(ORANGE)),
                Span::raw(attachment.file_name.clone()),
                Span::styled(
                    format!(
                        "  {} · {} bytes",
                        attachment.media_type, attachment.byte_length
                    ),
                    Style::default().fg(Color::DarkGray),
                ),
            ]));
        }
    }
}

fn transcript_is_fully_ordered(app: &App) -> bool {
    if app.transcript_item_order.is_empty() {
        return false;
    }
    let ordered = app.transcript_item_order.iter().collect::<HashSet<_>>();
    app.messages
        .iter()
        .all(|message| ordered.contains(&message.id))
        && app
            .tool_activity
            .iter()
            .all(|tool| ordered.contains(&tool.item_id))
        && app.plans.iter().all(|plan| ordered.contains(&plan.id))
        && app
            .notices
            .iter()
            .all(|notice| ordered.contains(&notice.item_id))
        && app
            .questions
            .iter()
            .all(|question| ordered.contains(&question.item_id))
        && app
            .artifacts
            .iter()
            .all(|artifact| ordered.contains(&artifact.item_id))
}

fn append_ordered_transcript(lines: &mut Vec<Line<'static>>, app: &App) {
    for item_id in &app.transcript_item_order {
        if let Some(message) = app.messages.iter().find(|message| &message.id == item_id) {
            append_message(lines, app, message);
        } else if let Some(tool) = app
            .tool_activity
            .iter()
            .find(|tool| &tool.item_id == item_id)
        {
            append_tool_activity_item(lines, tool);
        } else if let Some(plan) = app.plans.iter().find(|plan| &plan.id == item_id) {
            append_plan_item(lines, plan);
        } else if let Some(notice) = app.notices.iter().find(|notice| &notice.item_id == item_id) {
            append_notice_item(lines, notice);
        } else if let Some(question) = app
            .questions
            .iter()
            .find(|question| &question.item_id == item_id)
        {
            append_question(lines, question);
        } else if let Some(artifact) = app
            .artifacts
            .iter()
            .find(|artifact| &artifact.item_id == item_id)
        {
            append_artifact(lines, artifact);
        }
    }
}

pub(crate) fn transcript_lines(app: &App) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    if app.messages.is_empty() {
        lines.extend([
            Line::from(""),
            Line::from(Span::styled(
                " What are we building?",
                Style::default().fg(ORANGE).add_modifier(Modifier::BOLD),
            )),
            Line::from(""),
            Line::from(Span::styled(
                " Describe an outcome. Use / for commands, @ for files, and ! for shell mode.",
                Style::default().fg(Color::DarkGray),
            )),
        ]);
    }
    if transcript_is_fully_ordered(app) {
        append_ordered_transcript(&mut lines, app);
    } else {
        let mut rendered_turns = HashSet::new();
        let mut rendered_plans = HashSet::new();
        let mut rendered_notices = HashSet::new();
        let mut rendered_questions = HashSet::new();
        let mut rendered_artifacts = HashSet::new();
        for message in &app.messages {
            if message.role == "assistant" {
                append_plan(&mut lines, app, &message.turn_id, &mut rendered_plans);
                append_tool_activity(&mut lines, app, &message.turn_id, &mut rendered_turns);
                append_notices(&mut lines, app, &message.turn_id, &mut rendered_notices);
                for question in app
                    .questions
                    .iter()
                    .filter(|question| question.turn_id == message.turn_id)
                {
                    if rendered_questions.insert(question.item_id.clone()) {
                        append_question(&mut lines, question);
                    }
                }
                for artifact in app
                    .artifacts
                    .iter()
                    .filter(|artifact| artifact.turn_id == message.turn_id)
                {
                    if rendered_artifacts.insert(artifact.item_id.clone()) {
                        append_artifact(&mut lines, artifact);
                    }
                }
            }
            append_message(&mut lines, app, message);
        }
        if let Some(turn_id) = app.current_turn.as_ref() {
            append_plan(&mut lines, app, turn_id, &mut rendered_plans);
            append_tool_activity(&mut lines, app, turn_id, &mut rendered_turns);
            append_notices(&mut lines, app, turn_id, &mut rendered_notices);
            for question in app
                .questions
                .iter()
                .filter(|question| &question.turn_id == turn_id)
            {
                if rendered_questions.insert(question.item_id.clone()) {
                    append_question(&mut lines, question);
                }
            }
            for artifact in app
                .artifacts
                .iter()
                .filter(|artifact| &artifact.turn_id == turn_id)
            {
                if rendered_artifacts.insert(artifact.item_id.clone()) {
                    append_artifact(&mut lines, artifact);
                }
            }
        }
    }
    for input in &app.pending_inputs {
        let mode = if input.mode == TurnInputMode::Steer {
            "Steering"
        } else {
            "Queued"
        };
        let content = input.content.as_str().unwrap_or("Structured input");
        let mut preview = content.chars().take(96).collect::<String>();
        if content.chars().count() > 96 {
            preview.push('…');
        }
        lines.push(Line::from(vec![
            Span::styled(
                format!(" {mode} · "),
                Style::default().fg(ORANGE).add_modifier(Modifier::BOLD),
            ),
            Span::raw(preview),
            Span::styled(
                format!("  /dequeue {}", input.id.0),
                Style::default().fg(Color::DarkGray),
            ),
        ]));
    }
    for (index, attachment) in app.pending_attachments.iter().enumerate() {
        lines.push(Line::from(vec![
            Span::styled(
                " Pending attachment · ",
                Style::default().fg(ORANGE).add_modifier(Modifier::BOLD),
            ),
            Span::raw(attachment.file_name.clone()),
            Span::styled(
                format!("  {} · /detach {}", attachment.media_type, index + 1),
                Style::default().fg(Color::DarkGray),
            ),
        ]));
    }
    for activity in app.activity.iter().take(6).rev() {
        let color = if activity.starts_with('×') {
            Color::Red
        } else if activity.starts_with('!') {
            Color::Yellow
        } else if activity.starts_with('●') {
            ORANGE
        } else {
            Color::Green
        };
        lines.push(Line::from(Span::styled(
            format!(" {activity}"),
            Style::default().fg(color),
        )));
    }
    if !app.tool_result.is_empty() && app.tool_result != "Press d to load the current Git diff." {
        let result_lines = app.tool_result.lines().collect::<Vec<_>>();
        let visible_lines = if app.tool_result_expanded {
            result_lines.len()
        } else {
            result_lines.len().min(12)
        };
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            " Changes / result",
            Style::default().add_modifier(Modifier::BOLD),
        )));
        lines.extend(
            result_lines
                .iter()
                .take(visible_lines)
                .map(|line| Line::from(format!(" {line}"))),
        );
        if result_lines.len() > 12 {
            let detail = if app.tool_result_expanded {
                format!(" {} lines · /output collapse", result_lines.len())
            } else {
                format!(
                    " … {} more lines · /output expand",
                    result_lines.len() - visible_lines
                )
            };
            lines.push(Line::from(Span::styled(
                detail,
                Style::default().fg(Color::DarkGray),
            )));
        }
    }
    if let Some(picker) = &app.picker {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            format!(
                " {} · {}",
                match picker.kind {
                    PickerKind::History => "History search",
                    PickerKind::Link => "Link picker",
                    PickerKind::Model => "Model picker",
                    PickerKind::Permission => "Permission picker",
                },
                picker.title
            ),
            Style::default().fg(ORANGE).add_modifier(Modifier::BOLD),
        )));
        lines.push(Line::from(Span::styled(
            format!(
                " Search: {}",
                if picker.query.is_empty() {
                    "type to filter".into()
                } else {
                    picker.query.clone()
                }
            ),
            Style::default().fg(Color::DarkGray),
        )));
        for (visible_index, option_index) in
            picker.visible_indices().into_iter().take(8).enumerate()
        {
            let option = &picker.options[option_index];
            let selected = visible_index == picker.selected;
            let color = if option.disabled_reason.is_some() {
                Color::DarkGray
            } else if selected {
                ORANGE
            } else {
                Color::White
            };
            lines.push(Line::from(vec![
                Span::styled(
                    if selected { " › " } else { "   " },
                    Style::default().fg(color),
                ),
                Span::styled(
                    option.label.clone(),
                    Style::default().fg(color).add_modifier(if selected {
                        Modifier::BOLD
                    } else {
                        Modifier::empty()
                    }),
                ),
                Span::styled(
                    format!(
                        " · {}",
                        option.disabled_reason.as_deref().unwrap_or(&option.detail)
                    ),
                    Style::default().fg(Color::DarkGray),
                ),
            ]));
        }
        lines.push(Line::from(Span::styled(
            " ↑↓ navigate · Enter choose · type to filter · Esc close",
            Style::default().fg(Color::DarkGray),
        )));
    }
    apply_theme(&mut lines, app.theme);
    lines
}

pub(crate) fn render(frame: &mut ratatui::Frame<'_>, app: &App) {
    let area = frame.area();
    let approval_height = if app.approvals.is_empty() { 0 } else { 6 };
    let goal_height = if app.goal.is_some() { 2 } else { 0 };
    let command_menu_visible = slash_command_menu_visible(app);
    let command_matches = matching_slash_commands(app.input.as_str());
    let command_menu_height = if command_menu_visible {
        u16::try_from(command_matches.len().min(8))
            .unwrap_or(8)
            .saturating_add(2)
            .max(3)
    } else {
        0
    };
    let statusline_height = u16::from(app.statusline != StatuslineMode::Off);
    let sections = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(8),
            Constraint::Length(approval_height),
            Constraint::Length(goal_height),
            Constraint::Length(command_menu_height),
            Constraint::Length(5),
            Constraint::Length(statusline_height),
        ])
        .split(area);

    let session = app.current();
    let title = session
        .map(|value| value.title.as_str())
        .unwrap_or("New task");
    let detail = session
        .map(|value| {
            format!(
                "{} · {}",
                value.model,
                header_workspace_label(&value.workspace_uri)
            )
        })
        .unwrap_or_else(|| "Ready when you are".into());
    frame.render_widget(
        Paragraph::new(vec![
            Line::from(vec![
                Span::styled(
                    " opencoding ",
                    Style::default()
                        .fg(theme_color(app.theme, ORANGE))
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(title, Style::default().add_modifier(Modifier::BOLD)),
            ]),
            Line::from(Span::styled(
                format!(" {detail}"),
                Style::default().fg(theme_color(app.theme, Color::DarkGray)),
            )),
        ]),
        sections[0],
    );

    let lines = transcript_lines(app);
    let max_transcript_scroll = lines.len().saturating_sub(usize::from(sections[1].height));
    let transcript_scroll = u16::try_from(
        max_transcript_scroll.saturating_sub(app.transcript_scroll.min(max_transcript_scroll)),
    )
    .unwrap_or(u16::MAX);
    frame.render_widget(
        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .scroll((transcript_scroll, 0)),
        sections[1],
    );

    if let Some(approval) = app.approvals.front() {
        let choices = [("[1] Allow once", ORANGE), ("[2] Reject", Color::Red)];
        let choice_spans = choices
            .iter()
            .enumerate()
            .flat_map(|(index, (label, color))| {
                let selected = index == app.approval_selected.min(choices.len() - 1);
                let style = if selected {
                    Style::default()
                        .fg(theme_color(app.theme, *color))
                        .add_modifier(Modifier::BOLD | Modifier::REVERSED)
                } else {
                    Style::default().fg(theme_color(app.theme, *color))
                };
                [
                    Span::styled(if selected { " › " } else { "   " }, style),
                    Span::styled(*label, style),
                    Span::raw("  "),
                ]
            })
            .collect::<Vec<_>>();
        frame.render_widget(
            Paragraph::new(vec![
                Line::from(Span::styled(
                    format!(" Approval required · {}", approval.display),
                    Style::default()
                        .fg(theme_color(app.theme, Color::Yellow))
                        .add_modifier(Modifier::BOLD),
                )),
                Line::from(" Review the requested operation and choose its scope."),
                Line::from(choice_spans),
                Line::from(Span::styled(
                    " ←/→ select · Enter confirm · 1/2 choose directly",
                    Style::default().fg(theme_color(app.theme, Color::DarkGray)),
                )),
            ])
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(theme_color(app.theme, Color::Yellow))),
            ),
            sections[2],
        );
    }

    if let Some(goal) = app.goal.as_ref() {
        let (status, color) = match goal.status {
            SessionGoalStatus::Active => ("active", Color::Green),
            SessionGoalStatus::Paused => ("paused", Color::Yellow),
            SessionGoalStatus::Completed => ("completed", Color::Green),
            SessionGoalStatus::Blocked => ("blocked", Color::Red),
        };
        let usage = goal.token_budget.map_or_else(
            || format!("{} turns", goal.continuation_count),
            |budget| {
                format!(
                    "{} / {} tokens",
                    goal.input_tokens.saturating_add(goal.output_tokens),
                    budget
                )
            },
        );
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(
                    " Goal ",
                    Style::default()
                        .fg(theme_color(app.theme, ORANGE))
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    format!("{status} "),
                    Style::default().fg(theme_color(app.theme, color)),
                ),
                Span::raw(format!("· {} · {usage}", goal.objective)),
            ]))
            .wrap(Wrap { trim: true }),
            sections[3],
        );
    }

    if command_menu_visible {
        let selected = app
            .slash_command_selected
            .min(command_matches.len().saturating_sub(1));
        let start = selected.saturating_add(1).saturating_sub(8);
        let lines = if command_matches.is_empty() {
            vec![Line::from(Span::styled(
                " No matching command",
                Style::default().fg(theme_color(app.theme, Color::DarkGray)),
            ))]
        } else {
            command_matches
                .iter()
                .enumerate()
                .skip(start)
                .take(8)
                .map(|(index, command)| {
                    let is_selected = index == selected;
                    let style = if is_selected {
                        Style::default()
                            .fg(theme_color(app.theme, ORANGE))
                            .add_modifier(Modifier::BOLD | Modifier::REVERSED)
                    } else {
                        Style::default().fg(theme_color(app.theme, Color::White))
                    };
                    Line::from(vec![
                        Span::styled(if is_selected { " › " } else { "   " }, style),
                        Span::styled(command.name, style),
                        Span::styled(
                            format!("  {}", command.detail),
                            Style::default().fg(theme_color(app.theme, Color::DarkGray)),
                        ),
                    ])
                })
                .collect()
        };
        frame.render_widget(
            Paragraph::new(lines).block(
                Block::default()
                    .title(" Commands · ↑↓ select · Enter/Tab complete · Esc close ")
                    .title_style(
                        Style::default()
                            .fg(theme_color(app.theme, ORANGE))
                            .add_modifier(Modifier::BOLD),
                    )
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(theme_color(app.theme, Color::DarkGray))),
            ),
            sections[4],
        );
    }

    let composer_title = match app.input_mode {
        InputMode::Prompt => format!(" Message {} ", app.assistant_alias),
        InputMode::NewWorkspace => " New session · workspace file:// URI ".into(),
        InputMode::NewModel => " New session · model ID ".into(),
    };
    frame.render_widget(
        Paragraph::new(app.input.as_str()).block(
            Block::default()
                .title(composer_title)
                .title_style(Style::default().fg(theme_color(app.theme, ORANGE)))
                .borders(Borders::ALL)
                .border_style(Style::default().fg(theme_color(app.theme, Color::DarkGray))),
        ),
        sections[5],
    );
    let prompt_inner_width = sections[5].width.saturating_sub(2);
    let (cursor_x, cursor_y) = app.input.cursor_position(prompt_inner_width);
    frame.set_cursor_position((
        sections[5].x.saturating_add(1).saturating_add(cursor_x),
        sections[5]
            .y
            .saturating_add(1)
            .saturating_add(cursor_y.min(2)),
    ));

    let mode_hint = if command_menu_visible {
        "slash command · ↑↓ select · Enter/Tab complete".into()
    } else if app.input.as_str().starts_with('/') {
        app.status.clone()
    } else if app.input.as_str().starts_with('@') {
        "file mention · type a repository path and press Tab".into()
    } else if app.input.as_str().starts_with('!') {
        "shell mode · explicit commands still follow Opencoding policy".into()
    } else {
        app.status.clone()
    };
    let pending = if app.pending_inputs.is_empty() {
        String::new()
    } else {
        format!(" · {} queued", app.pending_inputs.len())
    };
    let attachments = if app.pending_attachments.is_empty() {
        String::new()
    } else {
        format!(" · {} attached", app.pending_attachments.len())
    };
    if app.statusline != StatuslineMode::Off {
        let permission = match app.permission_mode {
            PermissionMode::Manual => "manual",
            PermissionMode::AcceptEdits => "accept edits",
            PermissionMode::Workspace => "workspace",
            PermissionMode::Plan => "plan",
        };
        let editor_mode = match (app.keymap, app.vim_mode) {
            (Keymap::Vim, VimMode::Normal) => " · NORMAL",
            (Keymap::Vim, VimMode::Insert) => " · INSERT",
            (Keymap::Emacs, _) => "",
        };
        let scroll = if app.transcript_scroll == 0 {
            String::new()
        } else {
            format!(" · history -{}", app.transcript_scroll)
        };
        let history = if app.transcript_item_count > app.transcript_loaded_items {
            format!(
                " · {}/{} items",
                app.transcript_loaded_items, app.transcript_item_count
            )
        } else {
            String::new()
        };
        let footer = match app.statusline {
            StatuslineMode::Full => format!(
                " {mode_hint}{pending}{attachments} · {permission}{editor_mode}{scroll}{history} · Ctrl-D exit · ? help"
            ),
            StatuslineMode::Compact => {
                format!(
                    " {mode_hint}{pending}{attachments} · {permission}{editor_mode}{scroll}{history}"
                )
            }
            StatuslineMode::Off => unreachable!(),
        };
        frame.render_widget(
            Paragraph::new(footer)
                .style(Style::default().fg(theme_color(app.theme, Color::DarkGray))),
            sections[6],
        );
    }
}

fn header_workspace_label(workspace_uri: &str) -> &str {
    let Some(path) = workspace_uri.strip_prefix("file://") else {
        return workspace_uri;
    };
    let trimmed = path.trim_end_matches('/');
    if trimmed.is_empty() {
        return "/";
    }
    trimmed
        .rsplit('/')
        .find(|segment| !segment.is_empty())
        .unwrap_or(trimmed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use opencoding_protocol::Message;
    use serde_json::json;

    #[test]
    fn markdown_renderer_handles_structure_inline_content_and_unicode() {
        let lines = markdown_lines(
            "# 结果\n- **完成** `cargo test`\n> 中文说明\n```rust\nfn main() {}\n```\n[docs](https://example.com)",
        );
        let rendered = lines
            .iter()
            .flat_map(|line| line.spans.iter())
            .map(|span| span.content.as_ref())
            .collect::<String>();
        assert!(rendered.contains("结果"));
        assert!(rendered.contains("• 完成 cargo test"));
        assert!(rendered.contains("│ 中文说明"));
        assert!(rendered.contains("fn main() {}"));
        assert!(rendered.contains("docs (https://example.com)"));
    }

    #[test]
    fn markdown_renderer_distinguishes_diff_additions_and_deletions() {
        let lines = markdown_lines("diff --git a/a b/a\n-old\n+new");
        assert_eq!(lines[1].spans[0].style.fg, Some(Color::Red));
        assert_eq!(lines[2].spans[0].style.fg, Some(Color::Green));
    }

    #[test]
    fn assistant_alias_heading_uses_a_distinct_gray_from_the_response_body() {
        let mut app = App::new(Vec::new(), true, true);
        app.assistant_alias = "橙子".into();
        app.messages.push(Message {
            id: Id("message-1".into()),
            session_id: Id("session-1".into()),
            turn_id: Id("turn-1".into()),
            role: "assistant".into(),
            content: json!("Response body"),
            created_at: Utc::now(),
        });

        let lines = transcript_lines(&app);
        let heading = lines
            .iter()
            .find(|line| line.spans.iter().any(|span| span.content == " 橙子"))
            .expect("assistant heading should be rendered");
        let body = lines
            .iter()
            .find(|line| {
                line.spans
                    .iter()
                    .any(|span| span.content == " Response body")
            })
            .expect("assistant body should be rendered");

        assert_eq!(heading.spans[0].style.fg, Some(ASSISTANT_HEADING));
        assert!(heading.spans[0].style.add_modifier.contains(Modifier::BOLD));
        assert_ne!(heading.spans[0].style.fg, body.spans[0].style.fg);
    }

    #[test]
    fn no_color_theme_removes_transcript_foreground_colors() {
        let mut app = App::new(Vec::new(), true, true);
        app.theme = CliTheme::NoColor;
        app.messages.push(Message {
            id: Id("message-1".into()),
            session_id: Id("session-1".into()),
            turn_id: Id("turn-1".into()),
            role: "assistant".into(),
            content: json!("# Result\n```rust\nfn main() {}\n```"),
            created_at: Utc::now(),
        });

        let lines = transcript_lines(&app);
        assert!(lines.iter().flat_map(|line| &line.spans).all(|span| {
            span.style
                .fg
                .is_none_or(|foreground| foreground == Color::Reset)
        }));
    }

    #[test]
    fn long_results_are_folded_and_explicitly_expandable() {
        let mut app = App::new(Vec::new(), true, true);
        app.tool_result = (1..=20)
            .map(|line| format!("line {line}"))
            .collect::<Vec<_>>()
            .join("\n");
        let folded = transcript_lines(&app)
            .iter()
            .flat_map(|line| line.spans.iter())
            .map(|span| span.content.as_ref())
            .collect::<String>();
        assert!(folded.contains("line 12"));
        assert!(!folded.contains("line 13"));
        assert!(folded.contains("8 more lines · /output expand"));

        app.tool_result_expanded = true;
        let expanded = transcript_lines(&app)
            .iter()
            .flat_map(|line| line.spans.iter())
            .map(|span| span.content.as_ref())
            .collect::<String>();
        assert!(expanded.contains("line 20"));
        assert!(expanded.contains("20 lines · /output collapse"));
    }

    #[test]
    fn interface_modes_parse_without_ambiguous_fallbacks() {
        assert_eq!(CliTheme::parse("light"), Some(CliTheme::Light));
        assert_eq!(CliTheme::parse("sepia"), None);
        assert_eq!(Keymap::parse("vim"), Some(Keymap::Vim));
        assert_eq!(Keymap::parse("default"), None);
        assert_eq!(
            StatuslineMode::parse("compact"),
            Some(StatuslineMode::Compact)
        );
        assert_eq!(StatuslineMode::parse("hidden"), None);
    }

    #[test]
    fn header_uses_a_compact_local_workspace_label() {
        assert_eq!(
            header_workspace_label("file:///Users/example/code/opencoding"),
            "opencoding"
        );
        assert_eq!(header_workspace_label("file:///"), "/");
        assert_eq!(
            header_workspace_label("ssh://host.example/workspace"),
            "ssh://host.example/workspace"
        );
    }
}
