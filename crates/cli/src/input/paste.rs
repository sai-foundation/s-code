use crate::state::App;

pub(crate) const MAX_BRACKETED_PASTE_BYTES: usize = 256 * 1024;

/// Returns the largest UTF-8-safe prefix that keeps the complete composer
/// within its configured byte budget. The boolean records whether any input
/// was omitted.
pub(crate) fn bounded_prefix(
    value: &str,
    existing_bytes: usize,
    max_total_bytes: usize,
) -> (&str, bool) {
    let available = max_total_bytes.saturating_sub(existing_bytes);
    if value.len() <= available {
        return (value, false);
    }
    let mut end = available.min(value.len());
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    (&value[..end], true)
}

pub(crate) fn apply_bracketed_paste(app: &mut App, value: &str) {
    if let Some(picker) = app.picker.as_mut() {
        let normalized = value.replace(['\r', '\n'], " ");
        let (value, truncated) =
            bounded_prefix(&normalized, picker.query.len(), MAX_BRACKETED_PASTE_BYTES);
        picker.query.push_str(value);
        picker.selected = 0;
        app.status = paste_status(truncated);
    } else {
        let selected_bytes = app
            .input
            .selection_range()
            .map_or(0, |selection| selection.len());
        let retained_bytes = app.input.as_str().len().saturating_sub(selected_bytes);
        let (value, truncated) = bounded_prefix(value, retained_bytes, MAX_BRACKETED_PASTE_BYTES);
        app.input.insert_str(value);
        app.composer_input_changed();
        app.status = paste_status(truncated);
    }
}

fn paste_status(truncated: bool) -> String {
    if truncated {
        format!(
            "pasted input capped at {} KiB",
            MAX_BRACKETED_PASTE_BYTES / 1024
        )
    } else {
        "pasted input inserted without submitting".into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bounded_prefix_preserves_utf8_and_counts_existing_input() {
        let value = "界".repeat(8);
        let (prefix, truncated) = bounded_prefix(&value, 5, 16);
        assert!(truncated);
        assert_eq!(prefix, "界界界");
        assert_eq!(5 + prefix.len(), 14);

        let (empty, truncated) = bounded_prefix("more", 16, 16);
        assert!(truncated);
        assert!(empty.is_empty());
    }
}
