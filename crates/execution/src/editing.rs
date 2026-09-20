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
        self.write_prepared_edit_with(call, prepared, results, |replacement| {
            runtime.apply_replacement(replacement)
        })
        .await
    }

    pub(super) async fn write_prepared_edit_with(
        &self,
        call: &ToolCall,
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
                // apply_replacement reports every post-write failure as Durability.
                // All other errors leave the target untouched by this write. A
                // matching hash may belong to an external editor, not this turn.
                self.store.abort_turn_file_change(&plan).await?;
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
                // A hunk holding only context lines marks a position instead of a
                // change, and the reference `apply_patch` accepts it while still
                // requiring those lines to be present. Such a hunk already carries
                // identical `old_text` and `new_text`, because a context line is
                // pushed to both, so letting it through as an ordinary edit keeps that
                // verification -- it must match exactly once, as the documented
                // contract says every update hunk must -- while replacing its text
                // with itself cannot change a byte. Dropping it unverified instead
                // would apply the remaining hunks against a file the patch does not
                // describe.
                if old_text.is_empty() {
                    return Err(invalid(if changed {
                        // Changes something, but carries nothing to match it against.
                        // This had shared one message with the case below, which made
                        // the two indistinguishable in a failure report.
                        "an update hunk that only adds lines needs context or removed lines to locate it"
                    } else {
                        // No lines at all: nothing to apply and nothing to verify.
                        // The reference rejects this shape too.
                        "an update hunk must contain at least one line"
                    }));
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

    #[test]
    fn a_context_only_hunk_locates_without_blocking_the_rest_of_the_patch() {
        // Shape observed from real model output: a first hunk that only marks where
        // to look, then a second that makes the change. The reference apply_patch
        // applies this patch; rejecting the locator failed the entire call, taking
        // the well-formed hunk with it.
        let files = parse(&json!({"patch":"*** Begin Patch\n*** Update File: a\n@@\n class C:\n@@\n-one\n+ONE\n*** End Patch", "revisions":{"a":"0123456789abcdef"}})).unwrap();
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].edits.len(), 2, "the locator is kept and verified");
        assert_eq!(
            files[0]
                .clone()
                .into_replacement(b"class C:\none\n")
                .unwrap()
                .content,
            "class C:\nONE\n"
        );
    }

    #[test]
    fn a_patch_of_only_locators_verifies_them_and_changes_nothing() {
        // The shape is accepted, as the reference accepts it, and because a locator
        // is applied as a replacement of its own text the file is byte identical
        // afterwards. Verification still happens: an absent locator fails, which the
        // next test covers.
        let files = parse(&json!({"patch":"*** Begin Patch\n*** Update File: a\n@@\n class C:\n*** End Patch", "revisions":{"a":"0123456789abcdef"}})).unwrap();
        assert_eq!(files[0].edits.len(), 1);
        assert_eq!(
            files[0]
                .clone()
                .into_replacement(b"class C:\nbody\n")
                .unwrap()
                .content,
            "class C:\nbody\n"
        );
    }

    #[test]
    fn a_locator_whose_text_is_absent_fails_instead_of_applying_the_rest() {
        // The reference verifies a locator's lines are present and fails when they
        // are not. Keeping the locator as a self-replacing edit preserves that:
        // dropping it unverified would apply the later hunk against a file the patch
        // does not actually describe.
        let files = parse(&json!({"patch":"*** Begin Patch\n*** Update File: a\n@@\n missing marker\n@@\n-one\n+ONE\n*** End Patch", "revisions":{"a":"0123456789abcdef"}})).unwrap();
        let error = files[0]
            .clone()
            .into_replacement(b"class C:\none\n")
            .unwrap_err();
        assert!(error.to_string().contains("matched 0 times"), "{error}");
    }

    #[test]
    fn an_empty_hunk_is_rejected_alone_and_beside_a_real_hunk() {
        // A hunk with no lines at all carries nothing to apply and nothing to
        // verify, so it cannot be treated as a locator. The reference rejects this
        // shape as well.
        for patch in [
            "*** Begin Patch\n*** Update File: a\n@@\n*** End Patch",
            "*** Begin Patch\n*** Update File: a\n@@\n-one\n+ONE\n@@\n*** End Patch",
            "*** Begin Patch\n*** Update File: a\n@@\n@@\n-one\n+ONE\n*** End Patch",
        ] {
            let error =
                parse(&json!({"patch": patch, "revisions":{"a":"0123456789abcdef"}})).unwrap_err();
            assert!(
                error.to_string().contains("must contain at least one line"),
                "{patch}: {error}"
            );
        }
    }

    #[test]
    fn consecutive_and_trailing_locators_are_verified_without_changing_bytes() {
        let files = parse(&json!({"patch":"*** Begin Patch\n*** Update File: a\n@@\n class C:\n@@\n def f():\n@@\n-one\n+ONE\n@@\n tail\n*** End Patch", "revisions":{"a":"0123456789abcdef"}})).unwrap();
        assert_eq!(files[0].edits.len(), 4);
        assert_eq!(
            files[0]
                .clone()
                .into_replacement(b"class C:\ndef f():\none\ntail\n")
                .unwrap()
                .content,
            "class C:\ndef f():\nONE\ntail\n"
        );
    }

    #[test]
    fn an_add_only_hunk_without_context_is_rejected_with_its_own_message() {
        // This hunk does change something but carries nothing to match it against,
        // which stays an error and no longer shares the locator's wording.
        let error = parse(&json!({"patch":"*** Begin Patch\n*** Update File: a\n@@\n+added\n*** End Patch", "revisions":{"a":"0123456789abcdef"}})).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("only adds lines needs context or removed lines"),
            "{error}"
        );
    }

    #[test]
    fn a_locator_occurring_twice_is_rejected_unlike_the_reference() {
        // The reference matches a locator relative to a moving cursor, so a repeated
        // locator is acceptable there. Here it is an ordinary edit and the shared
        // matcher demands exactly one occurrence, as it does of every hunk. Pinned as
        // a deliberate difference rather than left to be rediscovered.
        let files = parse(&json!({"patch":"*** Begin Patch\n*** Update File: a\n@@\n marker\n*** End Patch", "revisions":{"a":"0123456789abcdef"}})).unwrap();
        let error = files[0]
            .clone()
            .into_replacement(b"marker\nmarker\n")
            .unwrap_err();
        assert!(error.to_string().contains("matched 2 times"), "{error}");
    }

    #[test]
    fn a_locator_followed_by_an_add_only_hunk_is_still_rejected() {
        // The reference accepts this and appends the added lines. Here the add-only
        // hunk still has nothing to match against, so it keeps erroring: also a
        // deliberate difference, pinned so a later change notices it.
        let error = parse(&json!({"patch":"*** Begin Patch\n*** Update File: a\n@@\n marker\n@@\n+added\n*** End Patch", "revisions":{"a":"0123456789abcdef"}})).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("only adds lines needs context or removed lines"),
            "{error}"
        );
    }

    #[test]
    fn a_locator_removed_by_the_next_hunk_is_accepted_unlike_the_reference() {
        // The reference resumes matching after the locator, so a following hunk that
        // removes the locator's own lines fails there. Here every hunk matches
        // globally and sequentially, so this succeeds: the one case where this
        // implementation is looser than the reference.
        let files = parse(&json!({"patch":"*** Begin Patch\n*** Update File: a\n@@\n marker\n@@\n-marker\n-old\n+replacement\n*** End Patch", "revisions":{"a":"0123456789abcdef"}})).unwrap();
        assert_eq!(
            files[0]
                .clone()
                .into_replacement(b"before\nmarker\nold\nafter\n")
                .unwrap()
                .content,
            "before\nreplacement\nafter\n"
        );
    }

    #[test]
    fn add_file_rejects_a_bare_hunk_header_and_keeps_a_prefixed_one_as_content() {
        // Locators are an Update File notion; inside Add File every line must carry
        // the + prefix, so a bare @@ is an error and a prefixed one is literal text.
        let error = parse(&json!({"patch":"*** Begin Patch\n*** Add File: b\n@@\n*** End Patch", "revisions":{"b":null}})).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("Add File lines must start with +"),
            "{error}"
        );
        let files = parse(&json!({"patch":"*** Begin Patch\n*** Add File: b\n+@@\n*** End Patch", "revisions":{"b":null}})).unwrap();
        assert_eq!(
            files[0].clone().into_replacement(b"").unwrap().content,
            "@@\n"
        );
    }
}
