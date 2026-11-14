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
//! access-log target at `info`. `RUST_LOG`, when set, replaces the level mapping. ANSI colors
//! (`pretty` formats on stdout) follow Verdaccio (`log.colors` + `colorette`): only when stdout is
//! a terminal, `NO_COLOR` disables and `FORCE_COLOR` forces them, `colors: false` turns them off;
//! never in a log file. So logs collected from a container have no escape codes.
//!
//! Memory: the budgets of the `fastopm:` section are resolved against the container memory
//! limit ([`MemoryPlan::resolve`]) and logged once at startup.

use std::fs::OpenOptions;
use std::io::IsTerminal;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::{Arc, Mutex};

use clap::{ArgAction, Parser};
use fastopm_config::memory::{detect_memory_limit, format_size};
use fastopm_config::{
    Config, ListenAddress, LogConfig, LogFormat, LogLevel, LogTarget, MemoryPlan, Scheme,
};
use fastopm_server::{ACCESS_LOG_TARGET, AppState, ServerOptions, router_with};
use fastopm_storage::FsStorage;
use fastopm_store::{MemoryBudget, Store, StoreOptions};
use fastopm_uplink::build_uplinks;
use tracing_subscriber::EnvFilter;

/// mimalloc instead of the system allocator (feature `mimalloc`, on by default): with glibc
/// malloc, many threads allocating and freeing large JSON trees left RSS at ~2× the live heap.
#[cfg(feature = "mimalloc")]
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

/// Size of Tokio's blocking pool.
const BLOCKING_THREADS: usize = 32;

/// `tracing` target of the periodic memory report (`RUST_LOG=...,fastopm::memory=debug`).
const MEMORY_LOG_TARGET: &str = "fastopm::memory";

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

    // The blocking pool only runs small file-system calls (large manifests go to the
    // `fastopm_core::heavy` pool): 32 threads are plenty, and each idle thread keeps a stack
    // and allocator caches (512 by default: ~60 MiB more RSS under a cold-cache install).
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .max_blocking_threads(BLOCKING_THREADS)
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
    let plan = MemoryPlan::resolve(&config.fastopm.memory, detect_memory_limit());
    tracing::warn!(
        memory_limit = %format_size(plan.memory_limit),
        source = %plan.source,
        manifest_cache = %format_size(plan.manifest_cache),
        render_cache = %format_size(plan.render_cache),
        work_memory = %if plan.work_memory == 0 { "unlimited".to_owned() } else { format_size(plan.work_memory) },
        "memory budgets"
    );
    let budget = match usize::try_from(plan.work_memory) {
        Ok(0) | Err(_) => MemoryBudget::unlimited(),
        Ok(bytes) => MemoryBudget::new(bytes),
    };
    let storage = FsStorage::new(Arc::clone(&config));
    let uplinks = build_uplinks(&config).map_err(|e| e.to_string())?;
    let options = StoreOptions { manifest_cache_bytes: plan.manifest_cache, memory: budget };
    let store = Arc::new(Store::new(Arc::clone(&config), storage, uplinks, options));
    let server_options = ServerOptions {
        render_cache_bytes: usize::try_from(plan.render_cache).unwrap_or(usize::MAX),
        ..ServerOptions::default()
    };
    if tracing::enabled!(target: MEMORY_LOG_TARGET, tracing::Level::DEBUG) {
        let store = Arc::clone(&store);
        let render_cache = Arc::clone(&server_options.render_cache_used);
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                let work = store.memory();
                let status = std::fs::read_to_string("/proc/self/status").unwrap_or_default();
                let field = |name: &str| {
                    status
                        .lines()
                        .find_map(|l| l.strip_prefix(name))
                        .and_then(|v| v.split_whitespace().next()?.parse::<u64>().ok())
                        .unwrap_or(0)
                };
                let (rss, threads) = (field("VmRSS:") * 1024, field("Threads:"));
                tracing::debug!(
                    target: MEMORY_LOG_TARGET,
                    rss,
                    threads,
                    work_used = work.used(),
                    work_peak = work.peak(),
                    manifest_cache = store.manifest_cache_used(),
                    render_cache = render_cache.load(std::sync::atomic::Ordering::Relaxed),
                    "memory"
                );
            }
        });
    }
    let app = router_with(AppState { config: Arc::clone(&config), store }, server_options);

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
        LogTarget::Stdout => {
            let ansi = ansi_colors(
                log.colors,
                &|k| std::env::var_os(k).is_some(),
                &|k| std::env::var(k).ok(),
                std::io::stdout().is_terminal(),
            );
            let builder = builder.with_ansi(ansi);
            match log.format {
                LogFormat::Pretty => builder.without_time().try_init(),
                LogFormat::PrettyTimestamped => builder.try_init(),
                LogFormat::Json => builder.json().try_init(),
            }
        }
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

/// Whether stdout logs get ANSI colors: Verdaccio's `hasColors(log.colors)` with `colorette`'s
/// `isColorSupported` (`NO_COLOR` set → no; `FORCE_COLOR` set → yes; else a terminal whose
/// `TERM` is set and not `dumb`, or a known CI). `colors: false` never colors.
fn ansi_colors(
    colors: Option<bool>,
    is_set: &dyn Fn(&str) -> bool,
    var: &dyn Fn(&str) -> Option<String>,
    stdout_tty: bool,
) -> bool {
    if colors == Some(false) || is_set("NO_COLOR") {
        return false;
    }
    let term = var("TERM");
    let dumb = term.as_deref() == Some("dumb");
    let terminal = stdout_tty && term.as_deref().is_some_and(|t| !t.is_empty()) && !dumb;
    let ci = is_set("CI") && ["GITHUB_ACTIONS", "GITLAB_CI", "CIRCLECI"].iter().any(|k| is_set(k));
    is_set("FORCE_COLOR") || (cfg!(windows) && !dumb) || terminal || ci
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
    fn colors_only_on_a_terminal() {
        let env = |vars: &'static [(&'static str, &'static str)]| {
            (
                move |k: &str| vars.iter().any(|(n, _)| *n == k),
                move |k: &str| vars.iter().find(|(n, _)| *n == k).map(|(_, v)| (*v).to_owned()),
            )
        };
        let (set, var) = env(&[("TERM", "xterm-256color")]);
        assert!(ansi_colors(None, &set, &var, true));
        assert!(ansi_colors(Some(true), &set, &var, true));
        assert!(!ansi_colors(Some(false), &set, &var, true));
        // a container: stdout is a pipe
        assert!(!ansi_colors(None, &set, &var, false));
        assert!(!ansi_colors(Some(true), &set, &var, false));
        let (set, var) = env(&[("TERM", "dumb")]);
        assert!(!ansi_colors(None, &set, &var, true));
        let (set, var) = env(&[]);
        assert!(!ansi_colors(None, &set, &var, true), "no TERM");
        let (set, var) = env(&[("FORCE_COLOR", "1")]);
        assert!(ansi_colors(None, &set, &var, false));
        assert!(!ansi_colors(Some(false), &set, &var, false));
        let (set, var) = env(&[("FORCE_COLOR", "1"), ("NO_COLOR", "")]);
        assert!(!ansi_colors(None, &set, &var, false));
        let (set, var) = env(&[("CI", "true"), ("GITHUB_ACTIONS", "true")]);
        assert!(ansi_colors(None, &set, &var, false));
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
