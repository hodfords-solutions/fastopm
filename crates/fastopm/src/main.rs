//! `fastopm` binary.
//!
//! ```text
//! fastopm [--config <path>] [--listen <addr>]
//! fastopm --version
//! ```
//!
//! Same flags as `verdaccio` (`-c/--config`, `-l/--listen`, `-v/--version`). Startup: load the
//! config ([`Config::load`], Verdaccio's search order, default config written when none
//! exists), set up logging from `log:`, log the config warnings, build storage, uplinks and
//! store, bind [`Config::listen_address`] (`--listen` wins; TCP or unix socket; `https`
//! addresses are refused because fastopm reads no TLS settings), serve
//! [`fastopm_server::router`], stop gracefully on SIGINT/SIGTERM.
//!
//! Logging: `log.type` stdout or file (appended), `log.format` `pretty` (no timestamp),
//! `pretty-timestamped` or `json`, `log.level` with Verdaccio's names. Verdaccio's `http` level
//! sits between `warn` and `info` and only adds the access log, so it maps to `warn` plus the
//! access-log target at `info`. `RUST_LOG`, when set, replaces the level mapping.

use std::fs::OpenOptions;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::{Arc, Mutex};

use clap::{ArgAction, Parser};
use fastopm_config::{Config, ListenAddress, LogConfig, LogFormat, LogLevel, LogTarget, Scheme};
use fastopm_server::{ACCESS_LOG_TARGET, AppState, router};
use fastopm_storage::FsStorage;
use fastopm_store::{Store, StoreOptions};
use fastopm_uplink::build_uplinks;
use tracing_subscriber::EnvFilter;

/// Command line.
#[derive(Debug, Parser)]
#[command(name = "fastopm", about = "npm proxy cache compatible with Verdaccio", version)]
#[command(disable_version_flag = true)]
struct Cli {
    /// Path to config.yaml (default: Verdaccio's search order).
    #[arg(short = 'c', long = "config", value_name = "PATH")]
    config: Option<PathBuf>,

    /// Listen address, overrides `listen` in the config: `4873`, `host:port`,
    /// `http://host:port/`, `unix:/path.sock`.
    #[arg(short = 'l', long = "listen", value_name = "ADDR")]
    listen: Option<String>,

    /// Print the version and exit.
    #[arg(short = 'v', long = "version", action = ArgAction::Version)]
    version: Option<bool>,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(cli) {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            if tracing::dispatcher::has_been_set() {
                tracing::error!("{message}");
            }
            eprintln!("fastopm: {message}");
            ExitCode::FAILURE
        }
    }
}

fn run(cli: Cli) -> Result<(), String> {
    let config = Config::load(cli.config.as_deref()).map_err(|e| e.to_string())?;
    init_logging(&config.log)?;
    tracing::warn!(config = %config.config_path.display(), "config file");
    for warning in &config.warnings {
        tracing::warn!("{warning}");
    }
    let address = config.listen_address(cli.listen.as_deref()).map_err(|e| e.to_string())?;
    check_address(&address)?;

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|e| format!("cannot start the runtime: {e}"))?;
    runtime.block_on(serve(Arc::new(config), address))
}

/// fastopm has no TLS settings: an `https` listen address cannot be honoured.
fn check_address(address: &ListenAddress) -> Result<(), String> {
    match address {
        ListenAddress::Tcp { scheme: Scheme::Https, .. } => Err(format!(
            "cannot listen on {address}: fastopm does not terminate TLS; listen on http and put \
             a TLS reverse proxy in front"
        )),
        _ => Ok(()),
    }
}

async fn serve(config: Arc<Config>, address: ListenAddress) -> Result<(), String> {
    let storage = FsStorage::new(Arc::clone(&config));
    let uplinks = build_uplinks(&config).map_err(|e| e.to_string())?;
    let store =
        Arc::new(Store::new(Arc::clone(&config), storage, uplinks, StoreOptions::default()));
    let app = router(AppState { config: Arc::clone(&config), store });

    match &address {
        ListenAddress::Tcp { host, port, .. } => {
            let listener = tokio::net::TcpListener::bind((host.as_str(), *port))
                .await
                .map_err(|e| format!("cannot listen on {address}: {e}"))?;
            let local = listener.local_addr().map_err(|e| e.to_string())?;
            tracing::warn!(address = %address, bound = %local, version = env!("CARGO_PKG_VERSION"), "http address");
            axum::serve(listener, app.into_make_service_with_connect_info::<SocketAddr>())
                .with_graceful_shutdown(shutdown_signal())
                .await
                .map_err(|e| format!("server error: {e}"))
        }
        #[cfg(unix)]
        ListenAddress::Unix { path } => {
            let listener = tokio::net::UnixListener::bind(path)
                .map_err(|e| format!("cannot listen on {address}: {e}"))?;
            tracing::warn!(address = %address, version = env!("CARGO_PKG_VERSION"), "http address");
            let served = axum::serve(listener, app.into_make_service())
                .with_graceful_shutdown(shutdown_signal())
                .await
                .map_err(|e| format!("server error: {e}"));
            // Node.js removes the socket file when the server closes.
            let _ = std::fs::remove_file(path);
            served
        }
        #[cfg(not(unix))]
        ListenAddress::Unix { .. } => {
            Err(format!("cannot listen on {address}: unix sockets are not supported here"))
        }
    }
}

/// Resolves on SIGINT or SIGTERM.
async fn shutdown_signal() {
    let interrupt = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut signal) => {
                signal.recv().await;
            }
            Err(_) => std::future::pending::<()>().await,
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();
    tokio::select! {
        () = interrupt => {}
        () = terminate => {}
    }
    tracing::warn!("received shutdown signal, closing connections");
}

/// `EnvFilter` directives for a Verdaccio log level.
fn level_directives(level: LogLevel) -> String {
    match level {
        LogLevel::Fatal | LogLevel::Error => "error".to_owned(),
        LogLevel::Warn => "warn".to_owned(),
        LogLevel::Http => format!("warn,{ACCESS_LOG_TARGET}=info"),
        LogLevel::Info => "info".to_owned(),
        LogLevel::Debug => "debug".to_owned(),
        LogLevel::Trace => "trace".to_owned(),
    }
}

fn init_logging(log: &LogConfig) -> Result<(), String> {
    let filter = match std::env::var("RUST_LOG") {
        Ok(directives) if !directives.is_empty() => EnvFilter::new(directives),
        _ => EnvFilter::new(level_directives(log.level)),
    };
    let builder = tracing_subscriber::fmt().with_env_filter(filter).with_target(false);
    let result = match &log.target {
        LogTarget::Stdout => match log.format {
            LogFormat::Pretty => builder.without_time().try_init(),
            LogFormat::PrettyTimestamped => builder.try_init(),
            LogFormat::Json => builder.json().try_init(),
        },
        LogTarget::File(path) => {
            let file = OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)
                .map_err(|e| format!("cannot open log file {}: {e}", path.display()))?;
            let builder = builder.with_ansi(false).with_writer(Mutex::new(file));
            match log.format {
                LogFormat::Pretty => builder.without_time().try_init(),
                LogFormat::PrettyTimestamped => builder.try_init(),
                LogFormat::Json => builder.json().try_init(),
            }
        }
    };
    result.map_err(|e| format!("cannot set up logging: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::{CommandFactory, Parser};

    #[test]
    fn cli_definition_is_valid() {
        Cli::command().debug_assert();
    }

    #[test]
    fn parses_flags() {
        let cli = Cli::try_parse_from(["fastopm", "-c", "/etc/c.yaml", "--listen", "0.0.0.0:4873"])
            .unwrap();
        assert_eq!(cli.config.as_deref(), Some(std::path::Path::new("/etc/c.yaml")));
        assert_eq!(cli.listen.as_deref(), Some("0.0.0.0:4873"));
    }

    #[test]
    fn levels_map_to_filters() {
        assert_eq!(level_directives(LogLevel::Fatal), "error");
        assert_eq!(level_directives(LogLevel::Http), "warn,fastopm_server::access=info");
        assert_eq!(level_directives(LogLevel::Info), "info");
        assert_eq!(level_directives(LogLevel::Trace), "trace");
        for level in [LogLevel::Warn, LogLevel::Http, LogLevel::Debug] {
            EnvFilter::try_new(level_directives(level)).unwrap();
        }
    }

    #[test]
    fn https_is_refused() {
        let https = ListenAddress::Tcp { scheme: Scheme::Https, host: "localhost".into(), port: 1 };
        assert!(check_address(&https).unwrap_err().contains("TLS"));
        let http = ListenAddress::Tcp { scheme: Scheme::Http, host: "localhost".into(), port: 1 };
        assert!(check_address(&http).is_ok());
        assert!(check_address(&ListenAddress::Unix { path: "/tmp/x.sock".into() }).is_ok());
    }
}
