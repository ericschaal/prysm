use anyhow::Result;
use clap::Parser;
use std::net::SocketAddr;

#[derive(Parser)]
#[command(about = "Browser controller for the Prysm REST API")]
struct Args {
    #[arg(long, default_value = "127.0.0.1:8788")]
    listen: SocketAddr,
    #[arg(long, default_value = "http://127.0.0.1:8787")]
    backend: String,
    /// Accepted browser Host headers. Defaults to the listen address and localhost.
    #[arg(long = "allowed-host")]
    allowed_hosts: Vec<String>,
}

#[tokio::main]
async fn main() -> Result<()> {
    let mut args = Args::parse();
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .init();
    if args.allowed_hosts.is_empty() {
        args.allowed_hosts = vec![
            args.listen.to_string(),
            format!("localhost:{}", args.listen.port()),
        ];
    }
    let router = prysm_web::router(&args.backend, args.allowed_hosts)?;
    let listener = tokio::net::TcpListener::bind(args.listen).await?;
    tracing::info!(address = %args.listen, backend = %args.backend, "Prysm web controller listening");
    axum::serve(listener, router)
        .with_graceful_shutdown(async {
            #[cfg(unix)]
            {
                let mut terminated =
                    tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                        .expect("termination signal");
                tokio::select! { _ = tokio::signal::ctrl_c() => {}, _ = terminated.recv() => {} }
            }
            #[cfg(not(unix))]
            let _ = tokio::signal::ctrl_c().await;
        })
        .await?;
    Ok(())
}
