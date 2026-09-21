//! Local names for the privacy explorer, not model context or a file-read tool.
use super::*;

const DIRECTORY_LIMIT: usize = 3_000;

#[derive(Deserialize)]
pub(super) struct DirectoryQuery {
    organization_id: String,
    team_id: String,
    actor_id: String,
    #[serde(default)]
    path: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub(super) struct DirectoryEntry {
    path: String,
    kind: String,
    size: Option<u64>,
}

#[derive(Debug, Serialize, Deserialize)]
pub(super) struct DirectoryPage {
    entries: Vec<DirectoryEntry>,
    truncated: bool,
}

pub(super) async fn get_files(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Query(query): Query<DirectoryQuery>,
) -> Result<Json<DirectoryPage>, ApiError> {
    let scope = Scope {
        organization_id: Id(query.organization_id),
        team_id: Id(query.team_id),
        actor_id: Id(query.actor_id),
        goal_id: None,
        task_id: None,
    };
    authorize(&state, &headers)?.ensure_scope(&scope)?;
    let session = state.store.get_session(&Id(id)).await?;
    if session.scope.organization_id != scope.organization_id
        || session.scope.team_id != scope.team_id
        || session.scope.actor_id != scope.actor_id
    {
        return Err(ApiError::Forbidden);
    }
    if session.mode != s_code_protocol::SessionMode::Work {
        return Err(ApiError::BadRequest("Chat has no workspace files".into()));
    }
    let page = tokio::task::spawn_blocking(move || {
        list_directory(&session.workspace_uri, &query.path, DIRECTORY_LIMIT)
    })
    .await
    .map_err(|_| ApiError::Internal("directory listing task failed".into()))??;
    Ok(Json(page))
}

fn valid_relative(path: &str) -> bool {
    path.len() <= 4096
        && !path.contains('\0')
        && (path.is_empty()
            || path
                .split('/')
                .all(|part| !part.is_empty() && part != "." && part != ".."))
}

#[cfg(unix)]
fn list_directory(uri: &str, path: &str, limit: usize) -> Result<DirectoryPage, ApiError> {
    use rustix::fs::{AtFlags, Dir, FileType, Mode, OFlags, open, openat, statat};
    use std::{ffi::OsStr, os::unix::ffi::OsStrExt};
    if !valid_relative(path) {
        return Err(ApiError::BadRequest(
            "directory path must be workspace-relative".into(),
        ));
    }
    // Only the stored session chooses the workspace root. Every user-selected
    // descendant is opened relative to its parent's handle, without symlinks.
    let root = url::Url::parse(uri)
        .ok()
        .filter(|url| url.scheme() == "file")
        .and_then(|url| url.to_file_path().ok())
        .ok_or_else(|| ApiError::BadRequest("workspace is unavailable".into()))?;
    // A trailing slash would make the kernel dereference a final symlink
    // before applying O_NOFOLLOW. Rebuild components to remove that slash.
    let root: std::path::PathBuf = root.components().collect();
    let flags = OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC | OFlags::NOFOLLOW;
    let denied = |_| ApiError::BadRequest("directory is unavailable or is a symbolic link".into());
    let mut directory = open(&root, flags, Mode::empty()).map_err(denied)?;
    for component in path.split('/').filter(|part| !part.is_empty()) {
        directory = openat(&directory, component, flags, Mode::empty()).map_err(denied)?;
    }
    let iterator = Dir::read_from(&directory).map_err(denied)?;
    let mut page = DirectoryPage {
        entries: Vec::new(),
        truncated: false,
    };
    let mut scanned = 0usize;
    for entry in iterator {
        let entry = entry.map_err(denied)?;
        let name = OsStr::from_bytes(entry.file_name().to_bytes());
        if name == "." || name == ".." {
            continue;
        }
        if scanned == limit {
            page.truncated = true;
            break;
        }
        scanned += 1;
        let metadata = match statat(&directory, name, AtFlags::SYMLINK_NOFOLLOW) {
            Ok(metadata) => metadata,
            Err(error) if error == rustix::io::Errno::NOENT => continue,
            Err(error) => return Err(denied(error)),
        };
        let file_type = FileType::from_raw_mode(metadata.st_mode);
        let Some(display) = name.to_str() else {
            // Do not alias invalid UTF-8 names to another file via replacement characters.
            page.truncated = true;
            continue;
        };
        page.entries.push(DirectoryEntry {
            path: if path.is_empty() {
                display.to_owned()
            } else {
                format!("{path}/{display}")
            },
            kind: match file_type {
                FileType::Directory => "directory",
                FileType::RegularFile => "file",
                _ => "other",
            }
            .into(),
            size: (file_type == FileType::RegularFile)
                .then(|| u64::try_from(metadata.st_size).ok())
                .flatten(),
        });
    }
    page.entries
        .sort_by(|a, b| (a.kind != "directory", &a.path).cmp(&(b.kind != "directory", &b.path)));
    Ok(page)
}

#[cfg(not(unix))]
fn list_directory(_: &str, path: &str, _: usize) -> Result<DirectoryPage, ApiError> {
    if !valid_relative(path) {
        return Err(ApiError::BadRequest(
            "directory path must be workspace-relative".into(),
        ));
    }
    Err(ApiError::BadRequest(
        "privacy directory browsing is available on macOS and Linux".into(),
    ))
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use axum::{body::to_bytes, http::Request};
    use tower::ServiceExt;

    #[test]
    fn privacy_directory_is_bounded_shallow_and_never_follows_links() {
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::fs::create_dir(root.path().join("src")).unwrap();
        std::fs::write(root.path().join("src/inside.rs"), "inside-body").unwrap();
        std::fs::write(root.path().join(".env"), "secret-body-never-returned").unwrap();
        std::fs::write(outside.path().join("outside-marker"), "outside").unwrap();
        std::os::unix::fs::symlink(outside.path(), root.path().join("escape")).unwrap();
        let uri = url::Url::from_directory_path(root.path())
            .unwrap()
            .to_string();
        let page = list_directory(&uri, "", DIRECTORY_LIMIT).unwrap();
        assert_eq!(page.entries.len(), 3);
        assert!(
            page.entries
                .iter()
                .any(|entry| entry.path == "escape" && entry.kind == "other")
        );
        assert!(
            page.entries
                .iter()
                .all(|entry| !entry.path.contains("inside.rs"))
        );
        assert!(
            !serde_json::to_string(&page)
                .unwrap()
                .contains("secret-body")
        );
        assert_eq!(
            list_directory(&uri, "src", DIRECTORY_LIMIT)
                .unwrap()
                .entries[0]
                .path,
            "src/inside.rs"
        );
        assert!(list_directory(&uri, "escape", DIRECTORY_LIMIT).is_err());
        assert!(list_directory(&uri, "escape/sub", DIRECTORY_LIMIT).is_err());
        for path in [
            "../",
            "/tmp",
            "src/../",
            "src//child",
            "src/./child",
            "bad\0name",
        ] {
            assert!(list_directory(&uri, path, DIRECTORY_LIMIT).is_err());
        }
        let linked_root = url::Url::from_directory_path(root.path().join("escape"))
            .unwrap()
            .to_string();
        assert!(list_directory(&linked_root, "", DIRECTORY_LIMIT).is_err());
        let bounded = list_directory(&uri, "", 2).unwrap();
        assert!(bounded.truncated);
        assert_eq!(bounded.entries.len(), 2);
    }

    #[tokio::test]
    async fn privacy_directory_requires_owned_work_session_and_does_not_send_context() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("example.rs"), "body-not-sent").unwrap();
        let store = Store::in_memory().await.unwrap();
        let scope = Scope {
            organization_id: Id("org".into()),
            team_id: Id("team".into()),
            actor_id: Id("user".into()),
            goal_id: None,
            task_id: None,
        };
        let work = store
            .create_session(CreateSession {
                scope: scope.clone(),
                mode: s_code_protocol::SessionMode::Work,
                workspace_uri: url::Url::from_directory_path(root.path())
                    .unwrap()
                    .to_string(),
                title: "Privacy".into(),
                model: "test".into(),
            })
            .await
            .unwrap();
        let chat = store
            .create_session(CreateSession {
                scope: scope.clone(),
                mode: s_code_protocol::SessionMode::Chat,
                workspace_uri: String::new(),
                title: "Chat".into(),
                model: "test".into(),
            })
            .await
            .unwrap();
        let service = app(AppState::new("test-token", store.clone(), 0));
        for (id, actor, token, status) in [
            (&work.id.0, "user", "test-token", StatusCode::OK),
            (&work.id.0, "other", "test-token", StatusCode::FORBIDDEN),
            (&work.id.0, "user", "wrong", StatusCode::UNAUTHORIZED),
            (&chat.id.0, "user", "test-token", StatusCode::BAD_REQUEST),
        ] {
            let response = service.clone().oneshot(Request::builder().uri(format!("/v1/sessions/{id}/privacy/files?organization_id=org&team_id=team&actor_id={actor}"))
                .header(header::AUTHORIZATION, format!("Bearer {token}")).body(Body::empty()).unwrap()).await.unwrap();
            assert_eq!(response.status(), status);
            if status == StatusCode::OK {
                let data = to_bytes(response.into_body(), 64 * 1024).await.unwrap();
                let listing: DirectoryPage = serde_json::from_slice(&data).unwrap();
                assert_eq!(listing.entries[0].path, "example.rs");
                assert!(!String::from_utf8_lossy(&data).contains("body-not-sent"));
            }
        }
        assert!(
            store
                .privacy_requests(&scope, &work.id, None, 50)
                .await
                .unwrap()
                .requests
                .is_empty()
        );
    }
}
