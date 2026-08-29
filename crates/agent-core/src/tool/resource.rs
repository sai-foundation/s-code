use std::path::{Component, Path};
use thiserror::Error;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum ResourceMode {
    Read,
    Write,
    ReadWrite,
    Search,
}

impl ResourceMode {
    fn writes(self) -> bool {
        matches!(self, Self::Write | Self::ReadWrite)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum ResourceNamespace {
    Workspace,
    Process,
    Network,
    Mcp,
    Artifact,
    External,
    Global,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResourceClaim {
    pub namespace: ResourceNamespace,
    pub key: String,
    pub mode: ResourceMode,
    pub recursive: bool,
}

#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum ResourceClaimError {
    #[error("resource path must remain relative to the workspace")]
    OutsideWorkspace,
}

impl ResourceClaim {
    pub fn new(
        namespace: ResourceNamespace,
        key: impl Into<String>,
        mode: ResourceMode,
        recursive: bool,
    ) -> Self {
        Self {
            namespace,
            key: key.into(),
            mode,
            recursive,
        }
    }

    pub fn global_exclusive() -> Self {
        Self::new(
            ResourceNamespace::Global,
            "*",
            ResourceMode::ReadWrite,
            true,
        )
    }

    /// Builds a lexical workspace claim. Runtime executors must still resolve
    /// symlinks against their canonical workspace before execution.
    pub fn workspace(
        path: impl AsRef<Path>,
        mode: ResourceMode,
        recursive: bool,
    ) -> Result<Self, ResourceClaimError> {
        let mut parts = Vec::new();
        for component in path.as_ref().components() {
            match component {
                Component::CurDir => {}
                Component::Normal(part) => parts.push(part.to_string_lossy().into_owned()),
                Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                    return Err(ResourceClaimError::OutsideWorkspace);
                }
            }
        }
        let key = if parts.is_empty() {
            ".".to_owned()
        } else {
            parts.join("/")
        };
        Ok(Self::new(
            ResourceNamespace::Workspace,
            key,
            mode,
            recursive,
        ))
    }

    pub fn conflicts_with(&self, other: &Self) -> bool {
        if self.namespace == ResourceNamespace::Global
            || other.namespace == ResourceNamespace::Global
        {
            return true;
        }
        if self.namespace != other.namespace || !(self.mode.writes() || other.mode.writes()) {
            return false;
        }
        keys_overlap(self, other)
    }
}

fn keys_overlap(left: &ResourceClaim, right: &ResourceClaim) -> bool {
    left.key == right.key
        || (left.recursive && contains_key(&left.key, &right.key))
        || (right.recursive && contains_key(&right.key, &left.key))
}

fn contains_key(parent: &str, child: &str) -> bool {
    parent == "."
        || child
            .strip_prefix(parent)
            .is_some_and(|suffix| suffix.starts_with('/'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn workspace_claims_normalize_relative_paths_and_reject_escape() {
        assert_eq!(
            ResourceClaim::workspace("./src/lib.rs", ResourceMode::Read, false)
                .unwrap()
                .key,
            "src/lib.rs"
        );
        assert_eq!(
            ResourceClaim::workspace(".", ResourceMode::Search, true)
                .unwrap()
                .key,
            "."
        );
        assert_eq!(
            ResourceClaim::workspace("../secret", ResourceMode::Read, false),
            Err(ResourceClaimError::OutsideWorkspace)
        );
        assert_eq!(
            ResourceClaim::workspace("/etc/passwd", ResourceMode::Read, false),
            Err(ResourceClaimError::OutsideWorkspace)
        );
    }

    #[test]
    fn reads_share_but_writes_conflict_on_overlapping_paths() {
        let read = ResourceClaim::workspace("src/lib.rs", ResourceMode::Read, false).unwrap();
        let search = ResourceClaim::workspace("src", ResourceMode::Search, true).unwrap();
        let write = ResourceClaim::workspace("src/lib.rs", ResourceMode::Write, false).unwrap();
        let other = ResourceClaim::workspace("tests/test.rs", ResourceMode::Write, false).unwrap();

        assert!(!read.conflicts_with(&search));
        assert!(search.conflicts_with(&write));
        assert!(!write.conflicts_with(&other));
        assert!(ResourceClaim::global_exclusive().conflicts_with(&read));
    }
}
