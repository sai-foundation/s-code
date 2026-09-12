//! Editing wire formats share version checks, path checks and Turn undo records.
use super::{
    ApplyPatchArgs, ExecutionError, ExecutionService, FileEdit, expected_revision_matches,
};
use s_code_protocol::ToolCall;
use s_code_tool_runtime::{FileReplacement, FileSnapshot, ToolError, ToolRuntime, content_sha256};
use serde_json::Value;
use std::{
    collections::BTreeSet,
    path::{Component, Path},
};

pub(crate) const MAX_EDIT_BYTES: usize = 16 * 1024 * 1024;

fn invalid(message: impl Into<String>) -> ExecutionError {
    ExecutionError::Arguments(message.into())
}

/// Return every write target, including targets carried inside patch text.
/// Invalid input must use an exclusive scheduler claim, never an empty claim.
pub fn editing_paths(value: &Value) -> Result<Vec<String>, ExecutionError> {
    Ok(parse(value)?.into_iter().map(|file| file.path).collect())
}

pub(crate) fn parse(value: &Value) -> Result<Vec<ApplyPatchArgs>, ExecutionError> {
    if value.to_string().len() > MAX_EDIT_BYTES {
        return Err(invalid("editing request exceeds 16 MiB"));
    }
    let object = value
        .as_object()
        .ok_or_else(|| invalid("expected editing object"))?;
    let forms = ["path", "files", "patch"]
        .iter()
        .filter(|key| object.contains_key(**key))
        .count();
    if forms != 1 {
        return Err(invalid(
            "use exactly one editing form: path, files, or patch",
        ));
    }
    let mut files: Vec<ApplyPatchArgs> = if let Some(files) = object.get("files") {
        if object.len() != 1 {
            return Err(invalid("files cannot be combined with other fields"));
        }
        serde_json::from_value(files.clone()).map_err(|e| invalid(e.to_string()))?
    } else if let Some(patch) = object.get("patch") {
        if object.len() != 2 {
            return Err(invalid("patch requires only patch and revisions"));
        }
        parse_patch(
            patch
                .as_str()
                .ok_or_else(|| invalid("patch must be a string"))?,
            &value["revisions"],
        )?
    } else {
        vec![serde_json::from_value(value.clone()).map_err(|e| invalid(e.to_string()))?]
    };
    if files.is_empty() || files.len() > 32 {
        return Err(invalid("edit 1 to 32 files per call"));
    }
    let mut paths = BTreeSet::new();
    for file in &mut files {
        file.validate_shape()?;
        let mut components = Vec::new();
        for component in Path::new(&file.path).components() {
            match component {
                Component::Normal(part) => components.push(part.to_string_lossy().into_owned()),
                Component::CurDir => {}
                _ => {
                    return Err(invalid(
                        "editing paths must be relative without parent traversal",
                    ));
                }
            }
        }
        file.path = components.join("/");
        if file.path.is_empty() || !paths.insert(file.path.clone()) {
            return Err(invalid("editing paths must be nonempty and unique"));
        }
    }
    Ok(files)
}

fn prepare_file(
    runtime: &ToolRuntime,
    mut patch: ApplyPatchArgs,
) -> Result<(FileSnapshot, FileReplacement, String), ExecutionError> {
    let snapshot = runtime.snapshot_file(&patch.path)?;
    let expected_revision = patch
        .expected_revision
        .as_deref()
        .or(patch.expected_sha256.as_deref())
        .filter(|value| !value.eq_ignore_ascii_case("null"))
        .map(str::to_owned);
    match (&snapshot.sha256, expected_revision.as_deref()) {
        (Some(current), Some(expected)) if expected_revision_matches(current, expected) => {
            patch.expected_sha256 = Some(current.clone());
        }
        (Some(_), None) => return Err(ToolError::MissingExpectedHash.into()),
        (None, None) => {}
        (Some(current), Some(_)) => {
            return Err(ExecutionError::Arguments(format!(
                "expected_revision does not match the current file; read the file again and use its 16-character revision {}",
                &current[..16]
            )));
        }
        (None, Some(_)) => {
            return Err(ExecutionError::Arguments(
                "the file does not exist; use JSON null for expected_revision when creating it"
                    .into(),
            ));
        }
    }
    if snapshot.content.is_none() && patch.content.is_none() {
        return Err(ExecutionError::Arguments(
            "exact text edits require an existing file; use content to create a file".into(),
        ));
    }
    let replacement = patch.into_replacement(snapshot.content.as_deref().unwrap_or_default())?;
    let after_sha256 = content_sha256(replacement.content.as_bytes());
    Ok((snapshot, replacement, after_sha256))
}

impl ExecutionService {
    pub(super) async fn apply_edits(
        &self,
        call: &ToolCall,
        runtime: &ToolRuntime,
    ) -> Result<Value, ExecutionError> {
        let batch = call.request.arguments.get("path").is_none();
        let patches = parse(&call.request.arguments)?;
        if patches.len() > 1 {
            let mut identities = BTreeSet::new();
            for patch in &patches {
                let identity = runtime.file_target_identity(&patch.path)?;
                if !identities.insert(identity) {
                    return Err(invalid(format!(
                        "duplicate file target {}: paths must refer to distinct files; no files were written",
                        patch.path
                    )));
                }
            }
        }
        let mut prepared = Vec::new();
        let mut total_bytes = 0usize;
        let mut snapshot_bytes = 0usize;
        // Resolve every path, version and edit before the first mutation.
        for patch in patches {
            let path = patch.path.clone();
            let (snapshot, replacement, after_sha256) =
                prepare_file(runtime, patch).map_err(|error| {
                    invalid(format!(
                        "cannot prepare {path}: {error}; no files were written"
                    ))
                })?;
            snapshot_bytes =
                snapshot_bytes.saturating_add(snapshot.content.as_ref().map_or(0, Vec::len));
            if snapshot_bytes > MAX_EDIT_BYTES {
                return Err(invalid(
                    "input snapshots exceed 16 MiB per call; split the batch",
                ));
            }
            total_bytes = total_bytes.saturating_add(replacement.content.len());
            if total_bytes > MAX_EDIT_BYTES {
                return Err(ExecutionError::Arguments(
                    "edited content exceeds 16 MiB per call".into(),
                ));
            }
            prepared.push((snapshot, replacement, after_sha256));
        }
        let mut results = Vec::new();
        for (snapshot, replacement, after_sha256) in prepared {
            let path = replacement.path.clone();
            let outcome = self
                .write_prepared_edit(
                    call,
                    runtime,
                    (snapshot, replacement, after_sha256),
                    &mut results,
                )
                .await;
            if let Err(error) = outcome {
                if !batch {
                    return Err(error);
                }
                let applied: Vec<_> = results
                    .iter()
                    .map(|result| result["path"].clone())
                    .collect();
                return Err(ExecutionError::Arguments(format!(
                    "batch stopped at {path}: {error}; applied files: {}; later files were not attempted. Inspect the failed target too; its write may have completed before an I/O error. Re-read before retrying; Turn undo records are retained.",
                    serde_json::to_string(&applied).unwrap_or_default()
                )));
            }
        }
        if batch {
            Ok(serde_json::json!({"files": results}))
        } else {
            Ok(results.remove(0))
        }
    }

    pub(super) async fn write_prepared_edit(
        &self,
        call: &ToolCall,
        runtime: &ToolRuntime,
        prepared: (FileSnapshot, FileReplacement, String),
        results: &mut Vec<Value>,
    ) -> Result<(), ExecutionError> {
        self.write_prepared_edit_with(call, runtime, prepared, results, |replacement| {
            runtime.apply_replacement(replacement)
        })
        .await
    }

    pub(super) async fn write_prepared_edit_with(
        &self,
        call: &ToolCall,
        runtime: &ToolRuntime,
        prepared: (FileSnapshot, FileReplacement, String),
        results: &mut Vec<Value>,
        write: impl FnOnce(FileReplacement) -> Result<s_code_tool_runtime::WriteResult, ToolError>,
    ) -> Result<(), ExecutionError> {
        let (snapshot, replacement, after_sha256) = prepared;
        let path = replacement.path.clone();

        let plan = self
            .store
            .plan_turn_file_change(
                &call.request.scope,
                &call.request.turn_id,
                &path,
                snapshot.content.as_deref(),
                snapshot.sha256.as_deref(),
                &after_sha256,
            )
            .await?;
        match write(replacement) {
            Ok(result) => {
                // If recording completion fails, the planned before-image
                // remains available to Turn undo/recovery.
                results.push(
                    serde_json::to_value(result)
                        .map_err(|error| ExecutionError::Arguments(error.to_string()))?,
                );
                self.store.complete_turn_file_change(&plan).await?;
                Ok(())
            }
            Err(error) => {
                if matches!(error, ToolError::Durability { .. }) {
                    results.push(serde_json::json!({"path":path,"sha256":after_sha256,"durability":"unconfirmed"}));
                    self.store.complete_turn_file_change(&plan).await?;
                    return Err(error.into());
                }
                if matches!(
                    error,
                    ToolError::ConcurrentModification | ToolError::MissingExpectedHash
                ) {
                    // These errors are returned before rename. A later
                    // hash match cannot make an external write our own.
                    self.store.abort_turn_file_change(&plan).await?;
                    return Err(error.into());
                }
                // A write can fail at directory fsync AFTER rename.
                // Discard undo data only when the file is known unchanged.
                match runtime.snapshot_file(&path) {
                    Ok(current) if current.sha256.as_deref() == Some(&after_sha256) => {
                        results.push(serde_json::json!({"path":path, "sha256":after_sha256}));
                        self.store.complete_turn_file_change(&plan).await?;
                    }
                    Ok(current) if current.sha256 == snapshot.sha256 => {
                        self.store.abort_turn_file_change(&plan).await?;
                    }
                    _ => {} // Preserve the plan for guarded recovery.
                }
                Err(ExecutionError::from(error))
            }
        }
    }
}

// Deliberately strict Add/Update subset: bare @@ hunks with unique exact context.
// No fuzzy whitespace matching, rename, deletion, or line-number hunk headers.
fn parse_patch(patch: &str, revisions: &Value) -> Result<Vec<ApplyPatchArgs>, ExecutionError> {
    let revisions = revisions
        .as_object()
        .ok_or_else(|| invalid("revisions must map every patch path to a revision or null"))?;
    let lines: Vec<_> = patch.lines().collect();
    if lines.first() != Some(&"*** Begin Patch") || lines.last() != Some(&"*** End Patch") {
        return Err(invalid("patch must have Begin Patch and End Patch markers"));
    }
    let mut result = Vec::new();
    let mut index = 1;
    while index < lines.len() - 1 {
        let (path, add) = if let Some(path) = lines[index].strip_prefix("*** Add File: ") {
            (path, true)
        } else if let Some(path) = lines[index].strip_prefix("*** Update File: ") {
            (path, false)
        } else {
            return Err(invalid(
                "supported patch operations are Add File and Update File",
            ));
        };
        let revision = revisions
            .get(path)
            .ok_or_else(|| invalid(format!("missing revision for {path}")))?;
        let expected_revision = match revision {
            Value::Null => None,
            Value::String(value) => Some(value.clone()),
            _ => return Err(invalid("revision must be a string or null")),
        };
        if add && expected_revision.is_some() {
            return Err(invalid("Add File requires a null revision"));
        }
        if !add && expected_revision.is_none() {
            return Err(invalid("Update File requires an existing revision"));
        }
        index += 1;
        let mut content = String::new();
        let mut edits = Vec::new();
        while index < lines.len() - 1 && !lines[index].starts_with("*** ") {
            if add {
                let line = lines[index]
                    .strip_prefix('+')
                    .ok_or_else(|| invalid("Add File lines must start with +"))?;
                content.push_str(line);
                content.push('\n');
                index += 1;
            } else {
                if lines[index] != "@@" {
                    return Err(invalid("Update File requires bare @@ before each hunk"));
                }
                index += 1;
                let mut old_text = String::new();
                let mut new_text = String::new();
                let mut changed = false;
                while index < lines.len() - 1
                    && lines[index] != "@@"
                    && !lines[index].starts_with("*** ")
                {
                    let line = lines[index];
                    let (prefix, text) = line
                        .split_at_checked(1)
                        .ok_or_else(|| invalid("patch lines require a space, + or - prefix"))?;
                    match prefix {
                        " " => {
                            old_text.push_str(text);
                            old_text.push('\n');
                            new_text.push_str(text);
                            new_text.push('\n');
                        }
                        "-" => {
                            old_text.push_str(text);
                            old_text.push('\n');
                            changed = true;
                        }
                        "+" => {
                            new_text.push_str(text);
                            new_text.push('\n');
                            changed = true;
                        }
                        _ => return Err(invalid("patch lines require a space, + or - prefix")),
                    }
                    index += 1;
                }
                if !changed || old_text.is_empty() {
                    return Err(invalid(
                        "each update hunk needs a change and existing context or removed lines",
                    ));
                }
                edits.push(FileEdit {
                    patch_hunk: true,
                    old_text: Some(old_text),
                    new_text,
                    start_line: None,
                    end_line: None,
                });
            }
        }
        result.push(ApplyPatchArgs {
            path: path.into(),
            expected_sha256: None,
            expected_revision,
            content: add.then_some(content),
            edits,
        });
    }
    if revisions.len() != result.len() {
        return Err(invalid("revisions must contain exactly the patch targets"));
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn overlapping_patch_context_is_ambiguous() {
        let files = parse(&json!({"patch":"*** Begin Patch\n*** Update File: a\n@@\n-foo\n-foo\n+replacement\n*** End Patch", "revisions":{"a":"0123456789abcdef"}})).unwrap();
        let error = files[0]
            .clone()
            .into_replacement(b"foo\nfoo\nfoo\n")
            .unwrap_err();
        assert!(error.to_string().contains("matched 2 times"));
    }

    #[test]
    fn patch_finds_a_line_match_overlapping_a_non_line_match() {
        let files = parse(&json!({"patch":"*** Begin Patch\n*** Update File: a\n@@\n-foo\n-foo\n+replacement\n*** End Patch", "revisions":{"a":"0123456789abcdef"}})).unwrap();
        let result = files[0]
            .clone()
            .into_replacement(b"xfoo\nfoo\nfoo\n")
            .unwrap();
        assert_eq!(result.content, "xfoo\nreplacement\n");
    }

    #[test]
    fn patch_applies_multiple_hunks_and_creates_another_file() {
        let files = parse(&json!({"patch":"*** Begin Patch\n*** Update File: a\n@@\n-one\n+ONE\n@@\n-three\n+THREE\n*** Add File: b\n+new\n*** End Patch", "revisions":{"a":"0123456789abcdef","b":null}})).unwrap();
        assert_eq!(
            files[0]
                .clone()
                .into_replacement(b"one\ntwo\nthree\n")
                .unwrap()
                .content,
            "ONE\ntwo\nTHREE\n"
        );
        assert_eq!(
            files[1].clone().into_replacement(b"").unwrap().content,
            "new\n"
        );
    }

    #[test]
    fn patch_context_does_not_match_a_partial_line_or_ambiguous_context() {
        let files = parse(&json!({"patch":"*** Begin Patch\n*** Update File: a\n@@\n-hello\n+world\n*** End Patch", "revisions":{"a":"0123456789abcdef"}})).unwrap();
        assert!(files[0].clone().into_replacement(b"say hello\n").is_err());
        assert!(
            files[0]
                .clone()
                .into_replacement(b"hello\nhello\n")
                .is_err()
        );
        assert_eq!(
            files[0]
                .clone()
                .into_replacement(b"say hello\nhello\n")
                .unwrap()
                .content,
            "say hello\nworld\n"
        );
    }

    #[test]
    fn rejects_duplicate_aliases_mixed_forms_and_unsupported_patch_operations() {
        for value in [
            json!({"files":[{"path":"a","content":"a"},{"path":"./a","content":"b"}]}),
            json!({"path":"a","content":"a","files":[]}),
            json!({"files":[{"path":"../a","content":"a"}]}),
            json!({"patch":"*** Begin Patch\n*** Delete File: a\n*** End Patch","revisions":{"a":"0123456789abcdef"}}),
            json!({"patch":"*** Begin Patch\n*** Add File: a\n+a\n*** End Patch","revisions":{}}),
        ] {
            assert!(parse(&value).is_err(), "{value}");
        }
    }
}
