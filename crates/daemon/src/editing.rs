use s_code_config::{EditingMode, ModelConfig};
use s_code_model_gateway::{ModelMessage, ToolDefinition};
use serde_json::{Value, json};
use std::collections::BTreeMap;

const PROMPT_MARKER: &str = "S-Code editing format:\n";

#[derive(Clone, Default)]
pub(crate) struct EditingProfiles {
    default: EditingMode,
    overrides: BTreeMap<String, EditingMode>,
    aliases: BTreeMap<String, String>,
}

impl From<&ModelConfig> for EditingProfiles {
    fn from(config: &ModelConfig) -> Self {
        Self {
            default: config.editing_mode,
            overrides: config.editing_overrides.clone(),
            aliases: config
                .endpoints
                .iter()
                .map(|endpoint| (endpoint.id.clone(), endpoint.provider_model.clone()))
                .collect(),
        }
    }
}

impl EditingProfiles {
    pub(crate) fn resolve(&self, model: &str) -> EditingMode {
        let underlying = self.aliases.get(model).map(String::as_str).unwrap_or(model);
        let selected = self
            .overrides
            .get(model)
            .or_else(|| self.overrides.get(underlying))
            .copied()
            .unwrap_or(self.default);
        if selected != EditingMode::Auto {
            return selected;
        }
        let name = underlying
            .rsplit('/')
            .next()
            .unwrap_or(underlying)
            .to_ascii_lowercase();
        if name.starts_with("claude-") {
            EditingMode::Text
        } else if name.starts_with("gpt-") || name.starts_with("codex") {
            EditingMode::Patch
        } else {
            EditingMode::Lines
        }
    }
}

pub(crate) fn configure(
    messages: &mut Vec<ModelMessage>,
    tools: &mut [ToolDefinition],
    mode: EditingMode,
) {
    messages.retain(|message| {
        !(message.role == "system"
            && message
                .content
                .as_str()
                .is_some_and(|text| text.starts_with(PROMPT_MARKER)))
    });
    let Some(tool) = tools.iter_mut().find(|tool| tool.name == "apply_patch") else {
        return;
    };
    let instructions = match mode {
        EditingMode::Auto | EditingMode::Lines => {
            "Use files with per-file path, expected_revision and edits containing start_line/end_line/new_text. Ranges are 1-based, inclusive, non-overlapping and refer to the original numbered_content. Never copy line-number prefixes into new_text."
        }
        EditingMode::Text => {
            "Use files with per-file path, expected_revision and edits containing old_text/new_text. Copy old_text exactly, including whitespace, with enough context to match once. Replacements within a file run in array order; later edits see earlier results."
        }
        EditingMode::Patch => {
            "Use patch plus a revisions object keyed by every patch path. Supported syntax: *** Begin Patch, *** Add File: path or *** Update File: path, bare @@ before each update hunk, lines prefixed by space (context), - (remove), or + (add), and *** End Patch. Update hunks match exactly once in sequence, including trailing newlines; include existing context for insertions. No fuzzy matching, named/numbered @@ headers, Delete/Move operations or EOF markers. For files without trailing newlines or full replacements, use the files/content form instead."
        }
    };
    let common = "Read existing files first and copy each file's 16-character revision. Use JSON null only for a new file. A revision belongs to one file, not the batch. Batch known independent edits for up to 32 unique paths and 16 MiB total content. All edits are validated before any write; filesystem writes are sequential, not a cross-file transaction. On a partial failure inspect the reported applied paths and re-read before retrying; Turn undo records remain available. Use content for creation or deliberate whole-file replacement. The editing format does not change permissions, sandbox or approval requirements.";
    tool.description = "Edit up to 32 workspace files with per-file revision checks. All files are validated before sequential writes; failures report applied paths. Follow the editing-format system instructions and this input schema.".into();
    tool.parameters = schema(mode);
    let position = messages
        .iter()
        .position(|message| message.role != "system")
        .unwrap_or(messages.len());
    messages.insert(
        position,
        ModelMessage {
            role: "system".into(),
            content: Value::String(format!("{PROMPT_MARKER}{instructions}\n{common}")),
        },
    );
}

fn schema(mode: EditingMode) -> Value {
    let revision = json!({"type":["string","null"],"minLength":16,"maxLength":16});
    let edit = match mode {
        EditingMode::Text => {
            json!({"type":"object","additionalProperties":false,"properties":{"old_text":{"type":"string","minLength":1},"new_text":{"type":"string"}},"required":["old_text","new_text"]})
        }
        _ => {
            json!({"type":"object","additionalProperties":false,"properties":{"start_line":{"type":"integer","minimum":1},"end_line":{"type":"integer","minimum":1},"new_text":{"type":"string"}},"required":["start_line","end_line","new_text"]})
        }
    };
    let mut file = json!({"type":"object","additionalProperties":false,"properties":{"path":{"type":"string"},"expected_revision":revision,"content":{"type":"string"}},"required":["path","expected_revision"]});
    if mode == EditingMode::Patch {
        file["required"] = json!(["path", "expected_revision", "content"]);
    } else {
        file["properties"]["edits"] =
            json!({"type":"array","minItems":1,"maxItems":100,"items":edit});
        file["oneOf"] = json!([{"required":["content"]},{"required":["edits"]}]);
    }
    let mut root = json!({"type":"object","additionalProperties":false,"properties":{"files":{"type":"array","minItems":1,"maxItems":32,"items":file}},"required":["files"]});
    if mode == EditingMode::Patch {
        root.as_object_mut().unwrap().remove("required");
        root["properties"]["patch"] = json!({"type":"string"});
        root["properties"]["revisions"] = json!({"type":"object","additionalProperties":revision});
        root["oneOf"] = json!([{"required":["files"],"not":{"anyOf":[{"required":["patch"]},{"required":["revisions"]}]}},{"required":["patch","revisions"],"not":{"required":["files"]}}]);
    }
    root
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn selects_family_alias_and_explicit_override() {
        let mut profiles = EditingProfiles::default();
        profiles
            .aliases
            .insert("fast".into(), "anthropic/claude-sonnet-4".into());
        assert_eq!(profiles.resolve("fast"), EditingMode::Text);
        assert_eq!(profiles.resolve("gpt-5"), EditingMode::Patch);
        assert_eq!(profiles.resolve("unknown"), EditingMode::Lines);
        profiles.overrides.insert("fast".into(), EditingMode::Lines);
        assert_eq!(profiles.resolve("fast"), EditingMode::Lines);
        profiles.default = EditingMode::Text;
        assert_eq!(profiles.resolve("gpt-5"), EditingMode::Text);
    }
    #[test]
    fn scheduler_claims_every_target_and_approval_names_the_batch() {
        use super::super::{approval_projection, daemon_resource_claims};
        use s_code_agent_core::tool::{ResourceClaim, ResourceMode};
        use s_code_protocol::Id;
        for arguments in [
            json!({"files":[{"path":"a","content":"a"},{"path":"b","content":"b"}]}),
            json!({"patch":"*** Begin Patch\n*** Add File: a\n+a\n*** Add File: b\n+b\n*** End Patch","revisions":{"a":null,"b":null}}),
        ] {
            let claims = daemon_resource_claims("apply_patch", &arguments, &Id("session".into()));
            assert_eq!(claims.len(), 2);
            assert!(claims.iter().any(|claim| claim.conflicts_with(
                &ResourceClaim::workspace("b", ResourceMode::Read, false).unwrap()
            )));
            assert!(!claims.iter().any(|claim| claim.conflicts_with(
                &ResourceClaim::workspace("other", ResourceMode::Read, false).unwrap()
            )));
            assert_eq!(
                approval_projection("apply_patch", &arguments)
                    .target
                    .as_deref(),
                Some(r#"["a","b"]"#)
            );
        }
    }

    #[test]
    fn approval_preserves_all_targets_and_long_paths_in_both_batch_formats() {
        use super::super::approval_projection;
        let paths: Vec<_> = (0..32)
            .map(|i| format!("src/{}file-{i}.rs", "long-folder/".repeat(10)))
            .collect();
        let files = json!({"files":paths.iter().map(|path| json!({"path":path,"expected_revision":null,"content":"new"})).collect::<Vec<_>>()});
        let mut patch = String::from("*** Begin Patch\n");
        let mut revisions = serde_json::Map::new();
        for path in &paths {
            patch.push_str(&format!("*** Add File: {path}\n+new\n"));
            revisions.insert(path.clone(), Value::Null);
        }
        patch.push_str("*** End Patch");
        for args in [files, json!({"patch":patch,"revisions":revisions})] {
            let projection = approval_projection("apply_patch", &args);
            let actual: Vec<String> =
                serde_json::from_str(projection.target.as_deref().unwrap()).unwrap();
            assert_eq!(actual, paths);
            assert!(projection.summary.len() < 400);
        }
        let legacy = approval_projection("apply_patch", &json!({"path":paths[0],"content":"new"}));
        assert_eq!(legacy.target.as_deref(), Some(paths[0].as_str()));
    }

    #[test]
    fn approval_paths_keep_control_characters_separate_and_redact_secrets() {
        use super::super::approval_projection;
        let path = "name\nwith-newline.rs";
        let target = approval_projection(
            "apply_patch",
            &json!({"files":[{"path":path,"content":"new"},{"path":"normal.rs","content":"new"}]}),
        )
        .target
        .unwrap();
        assert!(!target.contains('\n'));
        assert_eq!(
            serde_json::from_str::<Vec<String>>(&target).unwrap(),
            vec![path, "normal.rs"]
        );
        let json_filename = r#"["literal-name.rs"]"#;
        let target = approval_projection(
            "apply_patch",
            &json!({"path":json_filename,"content":"new"}),
        )
        .target
        .unwrap();
        assert_eq!(
            serde_json::from_str::<Vec<String>>(&target).unwrap(),
            vec![json_filename]
        );
        let token_path = format!("sk-{}", "a".repeat(48));
        let target = approval_projection(
            "apply_patch",
            &json!({"files":[{"path":token_path,"content":"new"}]}),
        )
        .target
        .unwrap();
        assert!(!target.contains(&token_path));
    }

    #[test]
    fn resume_replaces_prompt_and_preserves_shared_safety_message() {
        let mut messages = vec![
            ModelMessage {
                role: "system".into(),
                content: json!("shared safety"),
            },
            ModelMessage {
                role: "user".into(),
                content: json!("task"),
            },
        ];
        let mut tools = super::super::builtin_tools();
        configure(&mut messages, &mut tools, EditingMode::Lines);
        configure(&mut messages, &mut tools, EditingMode::Text);
        assert_eq!(messages.len(), 3);
        assert_eq!(messages[0].content, "shared safety");
        assert!(
            messages[1]
                .content
                .as_str()
                .unwrap()
                .contains("old_text/new_text")
        );
        let tool = tools
            .iter()
            .find(|tool| tool.name == "apply_patch")
            .unwrap();
        assert!(tool.parameters["properties"]["files"]["items"]["properties"]["edits"]["items"]["properties"]["old_text"].is_object());
        configure(&mut messages, &mut [], EditingMode::Text);
        assert_eq!(messages.len(), 2); // Read-only profiles do not advertise edits.
    }
}
