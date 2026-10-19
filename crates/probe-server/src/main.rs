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

    let args: Vec<String> = std::env::args().skip(1).collect();
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    match args.as_slice() {
        [] => {}
        // For Docker HEALTHCHECK, since the runtime image has no shell or curl.
        ["healthcheck"] => return healthcheck(),
        [cmd @ ("create-user" | "set-password" | "delete-user"), name] => {
            return user_command(cmd, name);
        }
        ["help" | "--help" | "-h"] => {
            eprintln!("{USAGE}");
            return ExitCode::SUCCESS;
        }
        _ => {
            eprintln!("{USAGE}");
            return ExitCode::from(2);
        }
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

const USAGE: &str = "usage:
  probe-server                      run the server (settings from the environment)
  probe-server healthcheck          exit 0 if GET /probe/health answers 200
  probe-server create-user <name>   add a login; the password is read from stdin
  probe-server set-password <name>  new password from stdin; logs the user out everywhere
  probe-server delete-user <name>   remove a login and all its sessions

The user commands need only DATABASE_URL. On the VPS, for example:
  docker compose exec -T probe-app /app/probe-server create-user probe < password-file
On a terminal the password is asked twice and not shown.";

/// `create-user`, `set-password` and `delete-user`: there is no sign-up page,
/// so logins are managed here, on the server, by whoever can run commands in
/// the container. The password comes from stdin, never from the command line
/// (which would end up in the shell history and the process list).
fn user_command(cmd: &str, name: &str) -> ExitCode {
    let run = || -> anyhow::Result<()> {
        let password = if cmd == "delete-user" {
            String::new()
        } else {
            read_new_password()?
        };
        let database_url = std::env::var("DATABASE_URL")
            .map_err(|_| anyhow::anyhow!("DATABASE_URL is not set"))?;
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        runtime.block_on(async {
            let pool = probe_server::connect_for_admin(&database_url).await?;
            let result = match cmd {
                "create-user" => probe_server::auth::create_user(&pool, name, &password).await,
                "set-password" => probe_server::auth::set_password(&pool, name, &password).await,
                _ => probe_server::auth::delete_user(&pool, name).await,
            };
            pool.close().await;
            result
        })
    };
    match run() {
        Ok(()) => {
            eprintln!("{cmd} {name}: done");
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("{cmd} {name}: {e:#}");
            ExitCode::FAILURE
        }
    }
}

/// On a terminal: asks twice without echo. Otherwise: the first line of stdin.
fn read_new_password() -> anyhow::Result<String> {
    let password = if std::io::stdin().is_terminal() {
        let first = rpassword::prompt_password("New password: ")?;
        let again = rpassword::prompt_password("Again: ")?;
        anyhow::ensure!(first == again, "the passwords don't match");
        first
    } else {
        let mut line = String::new();
        std::io::stdin().read_line(&mut line)?;
        line.trim_end_matches(['\r', '\n']).to_owned()
    };
    probe_server::auth::check_password(&password).map_err(anyhow::Error::msg)?;
    Ok(password)
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
