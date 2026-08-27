use opencoding_config::{Component, ConfigLoader};
use opencoding_linux_runner::{LinuxRunner, RunnerConfig};
use std::time::Duration;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    if std::env::args().any(|arg| arg == "--version") {
        println!("opencoding-linux-runner {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }
    let loaded = ConfigLoader::from_process()
        .load(Component::Runner)?
        .config
        .runner;
    let config = RunnerConfig {
        daemon_url: loaded.daemon_url,
        token: loaded.token.expect("validated runner token"),
        worker_id: loaded
            .worker_id
            .unwrap_or_else(|| format!("linux-{}", std::process::id())),
        allow_network: loaded.allow_network,
        poll_interval: Duration::from_millis(loaded.poll_interval_millis),
        lease_seconds: loaded.lease_seconds,
    };
    LinuxRunner::new(config)?.run_forever().await?;
    Ok(())
}
