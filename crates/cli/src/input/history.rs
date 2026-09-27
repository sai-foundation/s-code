use crate::state::{App, PickerKind, PickerOption, PickerState};
use std::collections::HashSet;

pub(crate) fn recall_history(app: &mut App, older: bool) {
    if app.prompt_history.is_empty() {
        return;
    }
    let last = app.prompt_history.len() - 1;
    let index = match (app.history_cursor, older) {
        (None, true) => {
            app.history_draft = Some(app.input.clone());
            last
        }
        (Some(0), true) => return,
        (Some(index), true) => index - 1,
        (Some(index), false) if index < last => index + 1,
        (Some(_), false) => {
            app.history_cursor = None;
            if let Some(draft) = app.history_draft.take() {
                app.input = draft;
            }
            app.composer_input_replaced();
            return;
        }
        (None, false) => return,
    };
    app.history_cursor = Some(index);
    app.input.replace(&app.prompt_history[index]);
    if !older {
        app.input.move_to_start();
    }
    app.composer_input_replaced();
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn down_outside_history_never_clears_a_draft() {
        let mut app = App::new(vec![], true, true);
        app.prompt_history.push("previous prompt".into());
        app.input.replace("live draft");

        recall_history(&mut app, false);

        assert_eq!(app.input.as_str(), "live draft");
        assert_eq!(app.history_cursor, None);
        assert!(app.history_draft.is_none());
    }

    #[test]
    fn history_round_trip_restores_the_exact_draft_and_cursor() {
        let mut app = App::new(vec![], true, true);
        app.prompt_history = vec!["older\nentry".into(), "newer entry".into()];
        app.input.replace("live draft");
        app.input.move_left();
        app.input.move_left();
        let draft_cursor = app.input.cursor();

        recall_history(&mut app, true);
        assert_eq!(app.input.as_str(), "newer entry");
        assert_eq!(app.input.cursor(), app.input.as_str().len());

        recall_history(&mut app, true);
        assert_eq!(app.input.as_str(), "older\nentry");
        assert_eq!(app.input.cursor(), app.input.as_str().len());

        recall_history(&mut app, false);
        assert_eq!(app.input.as_str(), "newer entry");
        assert_eq!(app.input.cursor(), 0);

        recall_history(&mut app, false);
        assert_eq!(app.input.as_str(), "live draft");
        assert_eq!(app.input.cursor(), draft_cursor);
        assert_eq!(app.history_cursor, None);
        assert!(app.history_draft.is_none());
    }

    #[test]
    fn editing_a_recalled_entry_detaches_from_history_without_losing_the_edit() {
        let mut app = App::new(vec![], true, true);
        app.prompt_history.push("previous prompt".into());
        app.input.replace("live draft");
        recall_history(&mut app, true);

        app.input.insert('!');
        app.composer_input_changed();
        recall_history(&mut app, false);

        assert_eq!(app.input.as_str(), "previous prompt!");
        assert_eq!(app.history_cursor, None);
        assert!(app.history_draft.is_none());
    }

    #[test]
    fn a_no_op_edit_does_not_discard_the_saved_draft() {
        let mut app = App::new(vec![], true, true);
        app.prompt_history.push("previous prompt".into());
        app.input.replace("live draft");
        app.input.move_left();
        let draft_cursor = app.input.cursor();
        recall_history(&mut app, true);

        app.input.delete();
        app.composer_input_changed();
        assert_eq!(app.history_cursor, Some(0));

        recall_history(&mut app, false);
        assert_eq!(app.input.as_str(), "live draft");
        assert_eq!(app.input.cursor(), draft_cursor);
    }
}
