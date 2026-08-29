use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeSet, VecDeque},
    fs,
    ops::Range,
    path::{Component, Path, PathBuf},
};
use thiserror::Error;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContextKind {
    System,
    ProjectInstructions,
    Skill,
    TeamKnowledge,
    Conversation,
    File,
    Selection,
    Diagnostic,
    ToolResult,
    Compaction,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Provenance {
    pub source_uri: String,
    pub owner_team_id: Option<String>,
    pub version: Option<String>,
    pub trust_level: String,
    pub valid_until: Option<DateTime<Utc>>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ContextItem {
    pub id: String,
    pub kind: ContextKind,
    pub content: String,
    pub priority: u16,
    pub pinned: bool,
    pub provenance: Provenance,
}

#[derive(Clone, Debug)]
pub struct ContextBudget {
    pub max_input_tokens: usize,
    pub reserved_output_tokens: usize,
    pub per_item_tokens: usize,
}

impl Default for ContextBudget {
    fn default() -> Self {
        Self {
            max_input_tokens: 64_000,
            reserved_output_tokens: 8_000,
            per_item_tokens: 16_000,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PackedContext {
    pub items: Vec<ContextItem>,
    pub estimated_tokens: usize,
    pub omitted_ids: Vec<String>,
    pub truncated_ids: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ConversationMessage {
    pub id: String,
    pub role: String,
    pub content: serde_json::Value,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PackedConversation {
    pub messages: Vec<ConversationMessage>,
    pub compaction: Option<ContextItem>,
    pub estimated_tokens: usize,
    pub omitted_ids: Vec<String>,
    pub truncated_ids: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkspacePathKind {
    File,
    Directory,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspacePathMatch {
    pub path: String,
    pub kind: WorkspacePathKind,
    pub score: u32,
}

#[derive(Debug, Error)]
pub enum ContextError {
    #[error("invalid workspace boundary: {0}")]
    Boundary(String),
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
}

pub fn pack(mut items: Vec<ContextItem>, budget: &ContextBudget) -> PackedContext {
    items.retain(|item| {
        item.provenance
            .valid_until
            .is_none_or(|expiry| expiry > Utc::now())
    });
    items.sort_by(|left, right| {
        right
            .pinned
            .cmp(&left.pinned)
            .then_with(|| right.priority.cmp(&left.priority))
            .then_with(|| left.id.cmp(&right.id))
    });
    let available = budget
        .max_input_tokens
        .saturating_sub(budget.reserved_output_tokens);
    let mut packed = Vec::new();
    let mut omitted = Vec::new();
    let mut truncated = Vec::new();
    let mut used = 0;
    for mut item in items {
        let remaining = available.saturating_sub(used);
        if remaining == 0 {
            omitted.push(item.id);
            continue;
        }
        let allowance = remaining.min(budget.per_item_tokens);
        let tokens = estimate_tokens(&item.content);
        if tokens > allowance {
            if item.pinned || allowance >= 64 {
                item.content = truncate_chars(&item.content, allowance.saturating_mul(4));
                truncated.push(item.id.clone());
            } else {
                omitted.push(item.id);
                continue;
            }
        }
        used += estimate_tokens(&item.content);
        packed.push(item);
    }
    PackedContext {
        items: packed,
        estimated_tokens: used,
        omitted_ids: omitted,
        truncated_ids: truncated,
    }
}

pub fn discover_instructions(
    workspace_uri: &str,
    active_relative_path: Option<&str>,
) -> Result<Vec<ContextItem>, ContextError> {
    let root = canonical_workspace(workspace_uri)?;
    let active = match active_relative_path {
        Some(relative) => {
            let path = Path::new(relative);
            if path.components().any(|component| {
                matches!(
                    component,
                    Component::ParentDir | Component::RootDir | Component::Prefix(_)
                )
            }) {
                return Err(ContextError::Boundary(relative.into()));
            }
            let joined = root.join(path);
            if joined.exists() {
                joined.canonicalize()?
            } else {
                joined
            }
        }
        None => root.clone(),
    };
    if !active.starts_with(&root) {
        return Err(ContextError::Boundary(active.display().to_string()));
    }
    let mut directories = Vec::new();
    let mut cursor = if active.is_dir() {
        active.as_path()
    } else {
        active.parent().unwrap_or(&root)
    };
    loop {
        directories.push(cursor.to_path_buf());
        if cursor == root {
            break;
        }
        cursor = cursor
            .parent()
            .ok_or_else(|| ContextError::Boundary(active.display().to_string()))?;
    }
    directories.reverse();
    let mut items = Vec::new();
    for directory in directories {
        let path = directory.join("AGENTS.md");
        if path.is_file() {
            let relative = path
                .strip_prefix(&root)
                .map_err(|_| ContextError::Boundary(path.display().to_string()))?;
            items.push(ContextItem {
                id: format!("instructions:{}", relative.display()),
                kind: ContextKind::ProjectInstructions,
                content: fs::read_to_string(&path)?,
                priority: 900,
                pinned: true,
                provenance: Provenance {
                    source_uri: url::Url::from_file_path(&path)
                        .map_err(|_| ContextError::Boundary(path.display().to_string()))?
                        .to_string(),
                    owner_team_id: None,
                    version: None,
                    trust_level: "repository".into(),
                    valid_until: None,
                },
            });
        }
    }
    Ok(items)
}

pub fn compact_conversation(
    messages: &[ContextItem],
    retain_recent: usize,
    max_summary_tokens: usize,
) -> (Option<ContextItem>, Vec<ContextItem>) {
    if messages.len() <= retain_recent {
        return (None, messages.to_vec());
    }
    let split = messages.len() - retain_recent;
    let mut summary = String::from("Earlier conversation summary (untrusted conversation data):\n");
    for item in &messages[..split] {
        let excerpt = truncate_chars(&item.content.replace('\n', " "), 400);
        summary.push_str(&format!("- [{}] {}\n", item.id, excerpt));
    }
    summary = truncate_chars(&summary, max_summary_tokens.saturating_mul(4));
    let compacted = ContextItem {
        id: format!("compaction:{split}"),
        kind: ContextKind::Compaction,
        content: summary,
        priority: 700,
        pinned: true,
        provenance: Provenance {
            source_uri: "internal://conversation-compaction".into(),
            owner_team_id: None,
            version: Some("1".into()),
            trust_level: "derived-untrusted".into(),
            valid_until: None,
        },
    };
    (Some(compacted), messages[split..].to_vec())
}

pub fn estimate_conversation_tokens(messages: &[ConversationMessage]) -> usize {
    messages
        .iter()
        .map(|message| {
            estimate_tokens(&message.role)
                .saturating_add(estimate_tokens(&message.content.to_string()))
                .saturating_add(4)
        })
        .sum()
}

pub fn pack_conversation_history(
    messages: Vec<ConversationMessage>,
    max_tokens: usize,
    per_message_tokens: usize,
    max_summary_tokens: usize,
) -> PackedConversation {
    if messages.is_empty() || max_tokens == 0 {
        return PackedConversation {
            omitted_ids: messages.into_iter().map(|message| message.id).collect(),
            messages: Vec::new(),
            compaction: None,
            estimated_tokens: 0,
            truncated_ids: Vec::new(),
        };
    }
    let mut normalized = Vec::with_capacity(messages.len());
    let mut truncated_ids = Vec::new();
    for mut message in messages {
        let content_tokens = estimate_tokens(&message.content.to_string());
        if content_tokens > per_message_tokens {
            if message.role == "tool" && message.content.is_object() {
                let encoded = truncate_chars(
                    &message.content["result"].to_string(),
                    per_message_tokens.saturating_mul(4),
                );
                message.content["result"] = serde_json::Value::String(format!(
                    "[tool result truncated to context budget] {encoded}"
                ));
                truncated_ids.push(message.id.clone());
            } else if tool_call_ids(&message).is_empty() {
                let encoded = truncate_chars(
                    &message.content.to_string(),
                    per_message_tokens.saturating_mul(4),
                );
                message.content = serde_json::Value::String(format!(
                    "[message truncated to context budget] {encoded}"
                ));
                truncated_ids.push(message.id.clone());
            }
        }
        normalized.push(message);
    }
    if estimate_conversation_tokens(&normalized) <= max_tokens {
        let estimated_tokens = estimate_conversation_tokens(&normalized);
        return PackedConversation {
            messages: normalized,
            compaction: None,
            estimated_tokens,
            omitted_ids: Vec::new(),
            truncated_ids,
        };
    }

    let summary_budget = max_summary_tokens.min(max_tokens / 4);
    let recent_budget = max_tokens.saturating_sub(summary_budget);
    let mut recent_tokens = 0_usize;
    let mut split = normalized.len();
    for group in conversation_atomic_groups(&normalized).iter().rev() {
        let tokens = estimate_conversation_tokens(&normalized[group.clone()]);
        if recent_tokens.saturating_add(tokens) > recent_budget {
            break;
        }
        recent_tokens += tokens;
        split = group.start;
    }
    let recent = normalized[split..].to_vec();
    let omitted = &normalized[..split];
    let omitted_ids = omitted.iter().map(|message| message.id.clone()).collect();
    let compaction = if omitted.is_empty() || summary_budget == 0 {
        None
    } else {
        let mut summary = String::from(
            "Earlier conversation summary (derived from untrusted conversation data):\n",
        );
        for message in omitted {
            let excerpt = truncate_chars(&message.content.to_string().replace('\n', " "), 240);
            summary.push_str(&format!("- [{}:{}] {excerpt}\n", message.id, message.role));
        }
        Some(ContextItem {
            id: format!("conversation-compaction:{split}"),
            kind: ContextKind::Compaction,
            content: truncate_chars(&summary, summary_budget.saturating_mul(4)),
            priority: 700,
            pinned: true,
            provenance: Provenance {
                source_uri: "internal://conversation-compaction".into(),
                owner_team_id: None,
                version: Some("1".into()),
                trust_level: "derived-untrusted".into(),
                valid_until: None,
            },
        })
    };
    let summary_tokens = compaction
        .as_ref()
        .map_or(0, |item| estimate_tokens(&item.content));
    PackedConversation {
        messages: recent,
        compaction,
        estimated_tokens: recent_tokens.saturating_add(summary_tokens),
        omitted_ids,
        truncated_ids,
    }
}

fn conversation_atomic_groups(messages: &[ConversationMessage]) -> Vec<Range<usize>> {
    let mut groups = Vec::new();
    let mut index = 0;
    while index < messages.len() {
        let start = index;
        index += 1;
        let call_ids = tool_call_ids(&messages[start]);
        if !call_ids.is_empty() {
            while index < messages.len()
                && messages[index].role == "tool"
                && messages[index]
                    .content
                    .get("tool_call_id")
                    .and_then(serde_json::Value::as_str)
                    .is_some_and(|id| call_ids.contains(id))
            {
                index += 1;
            }
        }
        groups.push(start..index);
    }
    groups
}

fn tool_call_ids(message: &ConversationMessage) -> BTreeSet<&str> {
    if message.role != "assistant" {
        return BTreeSet::new();
    }
    message
        .content
        .get("tool_calls")
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|call| call.get("id"))
        .filter_map(serde_json::Value::as_str)
        .collect()
}

pub fn estimate_tokens(content: &str) -> usize {
    content.chars().count().div_ceil(4).max(1)
}

pub fn fuzzy_workspace_paths(
    workspace_uri: &str,
    query: &str,
    limit: usize,
) -> Result<Vec<WorkspacePathMatch>, ContextError> {
    let root = canonical_workspace(workspace_uri)?;
    let query = query.trim().trim_start_matches('@').to_ascii_lowercase();
    let limit = limit.clamp(1, 100);
    let mut queue = VecDeque::from([root.clone()]);
    let mut matches = Vec::new();
    let mut scanned = 0usize;
    while let Some(directory) = queue.pop_front() {
        let Ok(read_dir) = fs::read_dir(&directory) else {
            continue;
        };
        let mut entries = read_dir.filter_map(Result::ok).collect::<Vec<_>>();
        entries.sort_by_key(|entry| entry.file_name());
        for entry in entries {
            scanned = scanned.saturating_add(1);
            if scanned > 50_000 {
                break;
            }
            let Ok(metadata) = fs::symlink_metadata(entry.path()) else {
                continue;
            };
            if metadata.file_type().is_symlink() {
                continue;
            }
            let name = entry.file_name().to_string_lossy().into_owned();
            if metadata.is_dir()
                && matches!(
                    name.as_str(),
                    ".git" | ".work" | "node_modules" | "target" | "dist"
                )
            {
                continue;
            }
            let path = entry.path();
            let Ok(relative) = path.strip_prefix(&root) else {
                continue;
            };
            let display = relative.to_string_lossy().replace('\\', "/");
            let lower = display.to_ascii_lowercase();
            let basename = name.to_ascii_lowercase();
            let Some(position) = lower.find(&query) else {
                if metadata.is_dir() {
                    queue.push_back(path);
                }
                continue;
            };
            let class = if basename.starts_with(&query) {
                0
            } else if lower.starts_with(&query) {
                1
            } else {
                2
            };
            let score =
                class * 1_000_000 + u32::try_from(position).unwrap_or(u32::MAX).min(999_999);
            matches.push(WorkspacePathMatch {
                path: if metadata.is_dir() {
                    format!("{display}/")
                } else {
                    display
                },
                kind: if metadata.is_dir() {
                    WorkspacePathKind::Directory
                } else {
                    WorkspacePathKind::File
                },
                score,
            });
            if metadata.is_dir() {
                queue.push_back(path);
            }
        }
        if scanned > 50_000 {
            break;
        }
    }
    matches.sort_by(|left, right| {
        left.score
            .cmp(&right.score)
            .then_with(|| left.path.len().cmp(&right.path.len()))
            .then_with(|| left.path.cmp(&right.path))
    });
    matches.truncate(limit);
    Ok(matches)
}

fn canonical_workspace(uri: &str) -> Result<PathBuf, ContextError> {
    let url = url::Url::parse(uri).map_err(|error| ContextError::Boundary(error.to_string()))?;
    if url.scheme() != "file" {
        return Err(ContextError::Boundary("only file:// is supported".into()));
    }
    url.to_file_path()
        .map_err(|_| ContextError::Boundary(uri.into()))?
        .canonicalize()
        .map_err(ContextError::Io)
}

fn truncate_chars(value: &str, max: usize) -> String {
    if value.chars().count() <= max {
        return value.into();
    }
    let mut output: String = value.chars().take(max.saturating_sub(1)).collect();
    output.push('…');
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(id: &str, priority: u16, pinned: bool, content: &str) -> ContextItem {
        ContextItem {
            id: id.into(),
            kind: ContextKind::File,
            content: content.into(),
            priority,
            pinned,
            provenance: Provenance {
                source_uri: format!("file:///{id}"),
                owner_team_id: None,
                version: None,
                trust_level: "repository".into(),
                valid_until: None,
            },
        }
    }

    #[test]
    fn packing_is_bounded_and_prefers_pinned_context() {
        let packed = pack(
            vec![
                item("low", 1, false, &"x".repeat(1000)),
                item("rules", 10, true, &"r".repeat(200)),
            ],
            &ContextBudget {
                max_input_tokens: 100,
                reserved_output_tokens: 20,
                per_item_tokens: 80,
            },
        );
        assert_eq!(packed.items[0].id, "rules");
        assert!(packed.estimated_tokens <= 80);
        assert!(packed.omitted_ids.contains(&"low".into()));
    }

    #[test]
    fn nested_agents_files_are_ordered_root_to_leaf() {
        let root = tempfile::tempdir().unwrap();
        fs::write(root.path().join("AGENTS.md"), "root").unwrap();
        fs::create_dir(root.path().join("src")).unwrap();
        fs::write(root.path().join("src/AGENTS.md"), "nested").unwrap();
        fs::write(root.path().join("src/lib.rs"), "").unwrap();
        let uri = url::Url::from_directory_path(root.path())
            .unwrap()
            .to_string();
        let items = discover_instructions(&uri, Some("src/lib.rs")).unwrap();
        assert_eq!(
            items
                .iter()
                .map(|item| item.content.as_str())
                .collect::<Vec<_>>(),
            vec!["root", "nested"]
        );
    }

    #[test]
    fn compaction_preserves_recent_messages() {
        let messages = (0..5)
            .map(|index| item(&index.to_string(), 1, false, "message"))
            .collect::<Vec<_>>();
        let (summary, recent) = compact_conversation(&messages, 2, 100);
        assert!(summary.unwrap().content.contains("Earlier conversation"));
        assert_eq!(
            recent
                .iter()
                .map(|item| item.id.as_str())
                .collect::<Vec<_>>(),
            vec!["3", "4"]
        );
    }

    #[test]
    fn full_history_pack_compacts_old_and_preserves_recent_within_budget() {
        let messages = (0..12)
            .map(|index| ConversationMessage {
                id: format!("msg_{index}"),
                role: if index % 2 == 0 { "user" } else { "assistant" }.into(),
                content: serde_json::Value::String(format!("message {index} {}", "x".repeat(180))),
            })
            .collect();
        let packed = pack_conversation_history(messages, 180, 80, 40);
        assert!(packed.estimated_tokens <= 180);
        assert!(!packed.omitted_ids.is_empty());
        assert_eq!(packed.messages.last().unwrap().id, "msg_11");
        let compaction = packed.compaction.unwrap();
        assert_eq!(compaction.kind, ContextKind::Compaction);
        assert_eq!(compaction.provenance.trust_level, "derived-untrusted");
    }

    #[test]
    fn oversized_recent_message_is_truncated_not_dropped() {
        let packed = pack_conversation_history(
            vec![ConversationMessage {
                id: "latest".into(),
                role: "user".into(),
                content: serde_json::Value::String("x".repeat(4_000)),
            }],
            200,
            100,
            20,
        );
        assert_eq!(packed.messages[0].id, "latest");
        assert_eq!(packed.truncated_ids, vec!["latest"]);
        assert!(packed.estimated_tokens <= 200);
    }

    #[test]
    fn compaction_never_splits_an_assistant_tool_call_from_its_results() {
        let messages = vec![
            ConversationMessage {
                id: "old".into(),
                role: "user".into(),
                content: serde_json::Value::String("old ".repeat(200)),
            },
            ConversationMessage {
                id: "assistant-call".into(),
                role: "assistant".into(),
                content: serde_json::json!({
                    "text": "",
                    "tool_calls": [{
                        "id": "call-1",
                        "type": "function",
                        "function": {"name": "read_file", "arguments": "x".repeat(160)}
                    }]
                }),
            },
            ConversationMessage {
                id: "tool-result".into(),
                role: "tool".into(),
                content: serde_json::json!({
                    "tool_call_id": "call-1",
                    "name": "read_file",
                    "result": {"content": "small"}
                }),
            },
            ConversationMessage {
                id: "latest".into(),
                role: "user".into(),
                content: serde_json::Value::String("latest".into()),
            },
        ];

        let packed = pack_conversation_history(messages, 80, 1_000, 20);
        let retained = packed
            .messages
            .iter()
            .map(|message| message.id.as_str())
            .collect::<BTreeSet<_>>();
        assert_eq!(
            retained.contains("assistant-call"),
            retained.contains("tool-result")
        );
        assert!(retained.contains("latest"));
    }

    #[test]
    fn truncating_a_tool_result_preserves_its_protocol_identity() {
        let packed = pack_conversation_history(
            vec![ConversationMessage {
                id: "tool-result".into(),
                role: "tool".into(),
                content: serde_json::json!({
                    "tool_call_id": "call-1",
                    "name": "read_file",
                    "result": "x".repeat(4_000)
                }),
            }],
            200,
            100,
            20,
        );

        assert_eq!(packed.messages[0].content["tool_call_id"], "call-1");
        assert!(
            packed.messages[0].content["result"]
                .as_str()
                .unwrap()
                .contains("tool result truncated")
        );
    }

    #[test]
    fn fuzzy_paths_rank_basename_matches_and_skip_build_directories_and_symlinks() {
        let root = tempfile::tempdir().unwrap();
        fs::create_dir_all(root.path().join("src/nested")).unwrap();
        fs::create_dir_all(root.path().join("target")).unwrap();
        fs::write(root.path().join("src/main.rs"), "").unwrap();
        fs::write(root.path().join("src/nested/domain.rs"), "").unwrap();
        fs::write(root.path().join("target/main.rs"), "").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(
            root.path().join("src/main.rs"),
            root.path().join("linked-main.rs"),
        )
        .unwrap();
        let uri = url::Url::from_directory_path(root.path())
            .unwrap()
            .to_string();

        let paths = fuzzy_workspace_paths(&uri, "main", 10).unwrap();
        assert_eq!(paths[0].path, "src/main.rs");
        assert!(!paths.iter().any(|item| item.path.starts_with("target/")));
        assert!(!paths.iter().any(|item| item.path == "linked-main.rs"));
    }
}
