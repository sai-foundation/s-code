//! Selective recall uses concrete locations, not shared vocabulary.
use super::*;

pub(super) struct Focus {
    path: String,
    lines: Vec<(u32, String)>,
    sha256: Option<String>,
}

fn path_boundary(character: char) -> bool {
    character.is_whitespace()
        || matches!(
            character,
            '`' | '\'' | '"' | '(' | ')' | '[' | ']' | '{' | '}' | ',' | ';'
        )
}

fn explicit_path(prompt: &str, path: &str) -> bool {
    prompt.match_indices(path).any(|(index, _)| {
        let prefix = &prompt[..index];
        let prefix = prefix.strip_suffix("./").unwrap_or(prefix);
        let before = prefix.chars().next_back().is_none_or(path_boundary);
        let suffix = &prompt[index + path.len()..];
        let after = suffix.chars().next().is_none_or(path_boundary)
            || suffix
                .strip_prefix(['.', '!', '?', ':'])
                .is_some_and(|rest| rest.chars().next().is_none_or(char::is_whitespace));
        before && after
    })
}

fn identifier(text: &str) -> bool {
    let mut characters = text.chars();
    characters
        .next()
        .is_some_and(|c| c == '_' || c.is_alphabetic())
        && characters.all(|c| c == '_' || c.is_alphanumeric())
}

fn contains_identifier(text: &str, query: &str) -> bool {
    text.split(|c: char| c != '_' && !c.is_alphanumeric())
        .any(|word| word == query)
}

/// Only this turn's bounded stored tool calls are supplied by the caller.
/// A failed, incomplete or ambiguous latest lookup provides no focus.
pub(super) fn focus(calls: &[s_code_protocol::ToolCall]) -> Option<Focus> {
    let mut lookups = calls
        .iter()
        .filter(|call| matches!(call.request.tool.as_str(), "read_file" | "search_text"))
        .collect::<Vec<_>>();
    lookups.sort_by_key(|call| call.created_at);
    let call = lookups.pop()?;
    if lookups
        .last()
        .is_some_and(|previous| previous.created_at == call.created_at)
        || call.status != ToolCallStatus::Completed
        || call.error.is_some()
        || call.updated_at < call.created_at
        || calls.iter().any(|other| {
            !read_only(other)
                && (!matches!(
                    other.status,
                    ToolCallStatus::Completed
                        | ToolCallStatus::Failed
                        | ToolCallStatus::Denied
                        | ToolCallStatus::Cancelled
                ) || other.updated_at >= call.created_at)
        })
    {
        return None;
    }
    let result = call.result.as_ref()?;
    if call.request.tool == "read_file" {
        let path = relative_source_path(call.request.arguments["path"].as_str()?)?;
        let content = result["content"].as_str()?;
        let start = u32::try_from(result["start_line"].as_u64()?)
            .ok()
            .filter(|line| *line > 0)?;
        let end = u32::try_from(result["end_line"].as_u64()?).ok()?;
        if relative_source_path(result["path"].as_str()?)? != path
            || !complete_sha256(result["sha256"].as_str()?)
            || content.is_empty()
            || content.len() > 128 * 1024
            || u64::from(start) + content.lines().count() as u64 != u64::from(end) + 1
        {
            return None;
        }
        return Some(Focus {
            path,
            lines: content
                .lines()
                .enumerate()
                .map(|(offset, text)| (start + offset as u32, text.into()))
                .collect(),
            sha256: Some(result["sha256"].as_str()?.into()),
        });
    }
    let query = call.request.arguments["query"].as_str()?;
    if !identifier(query)
        || result["exit_code"].as_i64() != Some(0)
        || result["truncated"].as_bool() != Some(false)
    {
        return None;
    }
    let output = result["stdout"].as_str()?;
    if output.len() > 128 * 1024 || output.lines().count() > 1_000 {
        return None;
    }
    let mut selected_path = None;
    let mut lines = Vec::new();
    for record in output.lines() {
        let mut parts = record.splitn(3, ':');
        let path = relative_source_path(parts.next()?)?;
        let line = parts.next()?.parse::<u32>().ok().filter(|line| *line > 0)?;
        let text = parts.next()?;
        // search_text accepts regexes. A literal query can still return a
        // substring; only complete identifier occurrences establish a focus.
        if !contains_identifier(text, query) {
            continue;
        }
        if selected_path
            .as_ref()
            .is_some_and(|selected| selected != &path)
        {
            return None;
        }
        selected_path = Some(path);
        lines.push((line, text.into()));
    }
    Some(Focus {
        path: selected_path?,
        lines,
        sha256: None,
    })
}

pub(super) fn score(
    prompt: &str,
    observation: &ProjectSourceObservation,
    focus: Option<&Focus>,
) -> usize {
    if observation.change.is_none() {
        return 0;
    }
    if explicit_path(prompt, &observation.path) {
        return 2;
    }
    usize::from(focus.is_some_and(|focus| {
        focus.path == observation.path
            && focus
                .sha256
                .as_ref()
                .is_none_or(|hash| hash == &observation.sha256)
            && focus.lines.iter().any(|(line, text)| {
                observation.fragments.iter().any(|fragment| {
                    line.checked_sub(fragment.start_line).is_some_and(|offset| {
                        fragment.text.lines().nth(offset as usize) == Some(text.as_str())
                    })
                })
            })
    }))
}

/// Suppress a duplicate only when the current outgoing tool content contains
/// every saved fragment at its exact file position and full-file hash.
pub(super) fn already_visible(
    observation: &ProjectSourceObservation,
    messages: &[ModelMessage],
) -> bool {
    !observation.fragments.is_empty()
        && observation.fragments.iter().all(|fragment| {
            messages.iter().any(|message| {
                if message.role != "tool" || message.content["name"] != "read_file" {
                    return false;
                }
                let value = &message.content["result"];
                if value["path"]
                    .as_str()
                    .and_then(relative_source_path)
                    .as_deref()
                    != Some(&observation.path)
                    || value["sha256"].as_str() != Some(&observation.sha256)
                {
                    return false;
                }
                let (Some(start), Some(content)) =
                    (value["start_line"].as_u64(), value["content"].as_str())
                else {
                    return false;
                };
                if start == 0 || start > u64::from(fragment.start_line) {
                    return false;
                }
                let offset = content
                    .split_inclusive('\n')
                    .take((u64::from(fragment.start_line) - start) as usize)
                    .map(str::len)
                    .sum::<usize>();
                content
                    .get(offset..)
                    .is_some_and(|remaining| remaining.starts_with(&fragment.text))
            })
        })
}

#[cfg(test)]
#[path = "learning_recall_tests.rs"]
mod tests;
