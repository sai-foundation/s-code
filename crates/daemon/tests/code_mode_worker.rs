use serde_json::{Value, json};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    process::{Child, ChildStdin, ChildStdout, Command},
};

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
