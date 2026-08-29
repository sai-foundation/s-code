use opencoding_api_server::{ServerConfig, app};
use tracing::info;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "opencoding_api_server=info,tower_http=info".into()),
        )
        .init();

    let config = ServerConfig::from_env()?;
    let bind = config.bind;
    let listener = tokio::net::TcpListener::bind(bind).await?;
    eprintln!("OPENCODING_API_SERVER_ADDR={}", listener.local_addr()?);
    info!(
        address = %listener.local_addr()?,
        model = opencoding_api_server::DEFAULT_MODEL,
        "Opencoding API Server listening"
    );
    axum::serve(listener, app(config)?).await?;
    Ok(())
}
