use crate::state::{App, PickerKind, PickerOption, PickerState};
use std::collections::HashSet;

pub(crate) fn recall_history(app: &mut App, older: bool) {
    if app.prompt_history.is_empty() {
        return;
    }
    let last = app.prompt_history.len() - 1;
    let index = match (app.history_cursor, older) {
        (None, true) => last,
        (Some(index), true) => index.saturating_sub(1),
        (Some(index), false) if index < last => index + 1,
        (Some(_), false) | (None, false) => {
            app.history_cursor = None;
            app.input.clear();
            app.composer_input_changed();
            return;
        }
    };
    app.history_cursor = Some(index);
    app.input.replace(&app.prompt_history[index]);
    app.composer_input_changed();
}

pub(crate) fn open_history_search(app: &mut App) {
    let mut seen = HashSet::new();
    let options = app
        .prompt_history
        .iter()
        .rev()
        .filter(|prompt| seen.insert((*prompt).clone()))
        .map(|prompt| {
            let first_line = prompt.lines().next().unwrap_or_default();
            let mut label = first_line.chars().take(72).collect::<String>();
            if first_line.chars().count() > 72 || prompt.contains('\n') {
                label.push('…');
            }
            let line_count = prompt.lines().count().max(1);
            PickerOption {
                id: prompt.clone(),
                label,
                detail: format!(
                    "{line_count} line{} · {} characters",
                    if line_count == 1 { "" } else { "s" },
                    prompt.chars().count()
                ),
                disabled_reason: None,
            }
        })
        .collect::<Vec<_>>();
    if options.is_empty() {
        app.status = "prompt history is empty".into();
        return;
    }
    app.picker = Some(PickerState {
        kind: PickerKind::History,
        title: "Ctrl-R · type to filter previous prompts".into(),
        options,
        query: String::new(),
        selected: 0,
    });
    app.status = "searching prompt history".into();
}
