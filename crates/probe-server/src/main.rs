use std::{
    io::{IsTerminal, Read, Write},
    net::{SocketAddr, TcpStream},
    process::ExitCode,
    time::Duration,
};

use probe_server::Config;
use tokio_util::sync::CancellationToken;
use tracing_subscriber::EnvFilter;

fn main() -> ExitCode {
    // Local development: read .env if there is one. In a container all
    // settings come from the environment and there is no .env file.
    let _ = dotenvy::dotenv();

    // `probe-server healthcheck`: for Docker HEALTHCHECK, since the runtime
    // image has no shell or curl.
    if std::env::args().nth(1).as_deref() == Some("healthcheck") {
        return healthcheck();
    }

    tracing_subscriber::fmt()
        // Colors only on a terminal, not in `docker logs`.
        .with_ansi(std::io::stderr().is_terminal())
        .with_writer(std::io::stderr)
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("info,tower_http=info")),
        )
        .init();

    let runtime = tokio::runtime::Builder::new_multi_thread()
        // Two workers are plenty here and keep memory low on the small VPS.
        .worker_threads(2)
        .enable_all()
        .build()
        .expect("tokio runtime");

    match runtime.block_on(serve()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            tracing::error!("{e:#}");
            ExitCode::FAILURE
        }
    }
}

async fn serve() -> anyhow::Result<()> {
    let config = Config::from_env()?;
    let listener = tokio::net::TcpListener::bind(config.bind_addr).await?;
    let shutdown = CancellationToken::new();
    tokio::spawn({
        let shutdown = shutdown.clone();
        async move {
            shutdown_signal().await;
            tracing::info!("shutdown requested");
            shutdown.cancel();
        }
    });
    probe_server::run(config, listener, shutdown).await
}

/// Resolves on Ctrl+C, or on SIGTERM (what `docker stop` sends).
async fn shutdown_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    #[cfg(unix)]
    let term = async {
        use tokio::signal::unix::{SignalKind, signal};
        match signal(SignalKind::terminate()) {
            Ok(mut s) => {
                s.recv().await;
            }
            Err(_) => std::future::pending().await,
        }
    };
    #[cfg(not(unix))]
    let term = std::future::pending::<()>();
    tokio::select! {
        () = ctrl_c => {}
        () = term => {}
    }
}

/// GET /probe/health on our own port; exit code 0 only on HTTP 200.
fn healthcheck() -> ExitCode {
    let port = std::env::var("BIND_ADDR")
        .ok()
        .and_then(|a| a.parse::<SocketAddr>().ok())
        .map_or(8080, |a| a.port());
    let check = || -> std::io::Result<bool> {
        let addr = SocketAddr::from(([127, 0, 0, 1], port));
        let mut stream = TcpStream::connect_timeout(&addr, Duration::from_secs(2))?;
        stream.set_read_timeout(Some(Duration::from_secs(3)))?;
        stream.write_all(
            b"GET /probe/health HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
        )?;
        let mut head = [0u8; 12];
        stream.read_exact(&mut head)?;
        Ok(head.starts_with(b"HTTP/1.1 200"))
    };
    match check() {
        Ok(true) => ExitCode::SUCCESS,
        _ => ExitCode::FAILURE,
    }
}
