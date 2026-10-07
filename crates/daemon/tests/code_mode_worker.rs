#[cfg(any(target_os = "macos", target_os = "linux"))]
use s_code_platform_runtime::{NativeRuntime, PlatformRuntime, ProcessSpec};
use serde_json::{Value, json};
#[cfg(unix)]
use sha2::Digest;
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    process::{Child, ChildStdin, ChildStdout, Command},
};

#[cfg(unix)]
fn pdf_fixture() -> (tempfile::TempDir, std::path::PathBuf, String) {
    use lopdf::{Document, Object, Stream, content::Content, content::Operation, dictionary};

    let mut document = Document::with_version("1.5");
    let pages_id = document.new_object_id();
    let font_id = document.add_object(dictionary! {
        "Type" => "Font",
        "Subtype" => "Type1",
        "BaseFont" => "Helvetica",
    });
    let resources_id = document.add_object(dictionary! {
        "Font" => dictionary! { "F1" => font_id },
    });
    let content_id = document.add_object(Stream::new(
        dictionary! {},
        Content {
            operations: vec![
                Operation::new("BT", vec![]),
                Operation::new("Tf", vec![Object::Name(b"F1".to_vec()), 12.into()]),
                Operation::new("Tj", vec![Object::string_literal("worker fixture")]),
                Operation::new("ET", vec![]),
            ],
        }
        .encode()
        .unwrap(),
    ));
    let page_id = document.add_object(dictionary! {
        "Type" => "Page",
        "Parent" => pages_id,
        "Contents" => content_id,
        "Resources" => resources_id,
        "MediaBox" => vec![0.into(), 0.into(), 612.into(), 792.into()],
    });
    document.objects.insert(
        pages_id,
        Object::Dictionary(dictionary! {
            "Type" => "Pages",
            "Kids" => vec![Object::Reference(page_id)],
            "Count" => 1,
        }),
    );
    let catalog_id = document.add_object(dictionary! {
        "Type" => "Catalog",
        "Pages" => pages_id,
    });
    document.trailer.set("Root", catalog_id);
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("fixture.pdf");
    document.save(&path).unwrap();
    let digest = format!("{:x}", sha2::Sha256::digest(std::fs::read(&path).unwrap()));
    (directory, path, digest)
}

struct Worker {
    child: Child,
    input: ChildStdin,
    output: BufReader<ChildStdout>,
}
impl Worker {
    async fn new(code: &str) -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_s-code-daemon"))
            .arg("--code-mode-worker")
            .env_clear()
            .kill_on_drop(true)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        let input = child.stdin.take().unwrap();
        let output = BufReader::new(child.stdout.take().unwrap());
        let mut worker = Self {
            child,
            input,
            output,
        };
        worker.send(json!({"code":code})).await;
        worker
    }
    async fn send(&mut self, value: Value) {
        self.input
            .write_all(format!("{value}\n").as_bytes())
            .await
            .unwrap();
    }
    async fn read(&mut self) -> Value {
        let mut line = String::new();
        let count = tokio::time::timeout(
            std::time::Duration::from_secs(40),
            self.output.read_line(&mut line),
        )
        .await
        .unwrap()
        .unwrap();
        assert!(count > 0, "worker exited without protocol response");
        serde_json::from_str(&line).unwrap()
    }
    async fn done(mut self) {
        assert!(
            tokio::time::timeout(std::time::Duration::from_secs(5), self.child.wait())
                .await
                .unwrap()
                .unwrap()
                .success()
        );
    }
}

#[tokio::test]
async fn parallel_tools_accept_out_of_order_results_and_return_only_selected_output() {
    let mut worker = Worker::new("const r = await Promise.all([tools.read_file({path:'a'}),tools.read_file({path:'b'})]); text(r.map(x => x.version));").await;
    let a = worker.read().await;
    let b = worker.read().await;
    assert_eq!(a["arguments"]["path"], "a");
    assert_eq!(b["arguments"]["path"], "b");
    worker
        .send(json!({"id":b["id"],"value":{"version":"b2","content":"large raw result"}}))
        .await;
    worker
        .send(json!({"id":a["id"],"value":{"version":"a1","content":"another raw result"}}))
        .await;
    assert_eq!(
        worker.read().await,
        json!({"type":"done","output":[["a1","b2"]]})
    );
    worker.done().await;
}

#[tokio::test]
async fn tool_denial_can_be_caught_without_hiding_it_from_the_host() {
    let mut worker =
        Worker::new("try { await tools.read_file({path:'.env'}); } catch(e) { text('denied'); }")
            .await;
    let call = worker.read().await;
    worker
        .send(json!({"id":call["id"],"error":"Sensitive path denied"}))
        .await;
    assert_eq!(
        worker.read().await,
        json!({"type":"done","output":["denied"]})
    );
    worker.done().await;
}

#[tokio::test]
async fn no_host_apis_or_recursive_execute_are_exposed() {
    let mut worker = Worker::new("text([typeof process,typeof require,typeof fetch,typeof setTimeout,typeof tools.execute,typeof tools.run_command]); try { await import('node:fs'); } catch { text('no imports'); }").await;
    assert_eq!(
        worker.read().await,
        json!({"type":"done","output":[["undefined","undefined","undefined","undefined","undefined","undefined"],"no imports"]})
    );
    worker.done().await;
}

#[tokio::test]
async fn unfinished_calls_invalid_code_output_limits_and_dead_promises_fail_closed() {
    for code in [
        "tools.read_file({path:'a'});",
        "const = !;",
        "text('x'.repeat(33000));",
        "await new Promise(()=>{});",
        "await Promise.all(Array.from({length:33},()=>tools.read_file({path:'a'})));",
    ] {
        let mut worker = Worker::new(code).await;
        assert_eq!(worker.read().await["type"], "failed", "{code}");
        worker.done().await;
    }
}

#[tokio::test]
async fn prototype_changes_cannot_redirect_bridge_responses() {
    let mut worker = Worker::new("Map.prototype.get = () => {throw 'hijack'}; JSON.parse = () => {throw 'hijack'}; const r=await tools.read_file({path:'a'}); text(r.content);").await;
    let call = worker.read().await;
    worker
        .send(json!({"id":call["id"],"value":{"content":"ok"}}))
        .await;
    assert_eq!(worker.read().await, json!({"type":"done","output":["ok"]}));
    worker.done().await;
}

#[tokio::test]
async fn infinite_javascript_is_interrupted() {
    let mut worker = Worker::new("while (true) {}").await;
    assert_eq!(worker.read().await["type"], "failed");
    worker.done().await;
}

#[cfg(unix)]
#[test]
fn pdf_worker_returns_only_bounded_selected_page_text() {
    let (_directory, path, digest) = pdf_fixture();

    let output = std::process::Command::new(env!("CARGO_BIN_EXE_s-code-pdf-worker"))
        .args(["text", path.to_str().unwrap(), "1", "1", &digest])
        .env_clear()
        .output()
        .unwrap();
    assert!(output.status.success());
    assert!(output.stderr.is_empty());
    let response: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(response["status"], "completed");
    assert_eq!(response["page_count"], 1);
    assert_eq!(response["truncated"], false);
    assert_eq!(response["sha256"], digest);
    assert!(
        response["content"]
            .as_str()
            .unwrap()
            .contains("worker fixture")
    );

    let mismatch = std::process::Command::new(env!("CARGO_BIN_EXE_s-code-pdf-worker"))
        .args([
            "text",
            path.to_str().unwrap(),
            "1",
            "1",
            "0000000000000000000000000000000000000000000000000000000000000000",
        ])
        .env_clear()
        .output()
        .unwrap();
    assert!(mismatch.status.success());
    let mismatch: Value = serde_json::from_slice(&mismatch.stdout).unwrap();
    assert_eq!(mismatch["status"], "failed");
    assert!(mismatch["error"].as_str().unwrap().contains("changed"));
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
#[tokio::test]
async fn pdf_worker_runs_through_the_networkless_os_sandbox() {
    use std::{collections::BTreeMap, fs::File, sync::Arc, time::Duration};

    let (directory, path, digest) = pdf_fixture();
    let executable = std::path::PathBuf::from(env!("CARGO_BIN_EXE_s-code-pdf-worker"))
        .canonicalize()
        .unwrap();
    let executable_directory = executable.parent().unwrap();
    let directory_uri = url::Url::from_directory_path(directory.path())
        .unwrap()
        .to_string();
    let output = NativeRuntime
        .execute(ProcessSpec {
            program: executable.to_string_lossy().into_owned(),
            args: vec![
                "text".into(),
                path.to_string_lossy().into_owned(),
                "1".into(),
                "1".into(),
                digest.clone(),
            ],
            cwd_uri: directory_uri.clone(),
            environment_handles: BTreeMap::new(),
            timeout: Duration::from_secs(12),
            network_enabled: false,
            browser_compatible: false,
            readable_root_uris: vec![
                directory_uri.clone(),
                url::Url::from_directory_path(executable_directory)
                    .unwrap()
                    .to_string(),
            ],
            writable_root_uris: Vec::new(),
            denied_read_uris: Vec::new(),
            output_limit_bytes: 64 * 1024,
            pinned_cwd: Some(Arc::new(File::open(directory.path()).unwrap())),
        })
        .await
        .unwrap();
    assert_eq!(output.exit_code, Some(0));
    assert!(!output.truncated);
    let response: Value = serde_json::from_str(&output.stdout).unwrap();
    assert_eq!(response["status"], "completed");
    assert_eq!(response["sha256"], digest);
    assert!(
        response["content"]
            .as_str()
            .unwrap()
            .contains("worker fixture")
    );

    let rendered_directory = directory.path().join("rendered");
    std::fs::create_dir(&rendered_directory).unwrap();
    let rendered_uri = url::Url::from_directory_path(&rendered_directory)
        .unwrap()
        .to_string();
    let image_path = rendered_directory.join("page.png");
    let output = NativeRuntime
        .execute(ProcessSpec {
            program: executable.to_string_lossy().into_owned(),
            args: vec![
                "view".into(),
                path.to_string_lossy().into_owned(),
                "1".into(),
                digest.clone(),
                image_path.to_string_lossy().into_owned(),
            ],
            cwd_uri: rendered_uri.clone(),
            environment_handles: BTreeMap::new(),
            timeout: Duration::from_secs(12),
            network_enabled: false,
            browser_compatible: false,
            readable_root_uris: vec![
                directory_uri,
                url::Url::from_directory_path(executable_directory)
                    .unwrap()
                    .to_string(),
            ],
            writable_root_uris: vec![rendered_uri],
            denied_read_uris: Vec::new(),
            output_limit_bytes: 64 * 1024,
            pinned_cwd: Some(Arc::new(File::open(&rendered_directory).unwrap())),
        })
        .await
        .unwrap();
    assert_eq!(output.exit_code, Some(0));
    assert!(!output.truncated);
    let response: Value = serde_json::from_str(&output.stdout).unwrap();
    assert_eq!(response["status"], "rendered");
    assert_eq!(response["page"], 1);
    assert_eq!(response["page_count"], 1);
    assert_eq!(response["annotations_omitted"], 0);
    assert_eq!(response["sha256"], digest);
    let image = std::fs::read(image_path).unwrap();
    assert!(image.starts_with(b"\x89PNG\r\n\x1a\n"));
    assert_eq!(response["image_bytes"], image.len() as u64);
    assert_eq!(
        response["image_sha256"],
        format!("{:x}", sha2::Sha256::digest(&image))
    );
}

#[cfg(not(unix))]
#[test]
fn pdf_worker_fails_closed_when_isolation_is_unsupported() {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_s-code-pdf-worker"))
        .args([
            "text",
            "unavailable.pdf",
            "1",
            "1",
            "0000000000000000000000000000000000000000000000000000000000000000",
        ])
        .env_clear()
        .output()
        .unwrap();
    assert!(output.status.success());
    let response: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(response["status"], "failed");
    assert!(
        response["error"]
            .as_str()
            .unwrap()
            .contains("unavailable on this platform")
    );
}
