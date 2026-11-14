//! Loads a Verdaccio `config.yaml` into a typed [`Config`].
//!
//! Only the keys fastopm uses are typed (`storage`, `listen`, `uplinks`, `packages`,
//! `url_prefix`, `log`, `http_proxy`, `https_proxy`, `no_proxy`, `user_agent`, `server_id`).
//! Every other top-level key is ignored and produces one entry in [`Config::warnings`]; so does
//! an `access` rule under `packages` (fastopm does not authenticate clients). A config file that
//! Verdaccio accepts must load without error.
//!
//! The top-level `fastopm:` section holds fastopm-only options ([`FastopmConfig`]): behavior
//! that differs from Verdaccio, every option off by default. Verdaccio ignores unknown top-level
//! keys, so the same file still works for both.
//!
//! Verdaccio reference: `packages/config/src/` (`config.ts`, `parse.ts`, `config-path.ts`,
//! `address.ts`, `uplinks.ts`, `package-access.ts`) and `getMatchedPackagesSpec` in
//! `packages/core/core/src/auth-utils.ts`.

use std::path::{Path, PathBuf};
use std::time::Duration;

use indexmap::IndexMap;

mod js;
pub mod listen;
mod load;
pub mod memory;
pub mod pattern;

pub use listen::{ListenAddress, Scheme, parse_address};
pub use memory::{MemoryOptions, MemoryPlan};
pub use pattern::pattern_matches;

/// Reads one environment variable. [`Config::load`] uses the process environment; tests pass a
/// closure over a fixed map.
pub type EnvLookup<'a> = &'a dyn Fn(&str) -> Option<String>;

/// The process environment as an [`EnvLookup`] target.
pub fn process_env(key: &str) -> Option<String> {
    std::env::var(key).ok()
}

/// Errors while locating or loading the config file.
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    /// The file cannot be read, or the default config cannot be created.
    #[error("cannot read or create config file {path}: {source}")]
    Io {
        /// File path.
        path: PathBuf,
        /// Underlying error.
        source: std::io::Error,
    },
    /// The path does not end in `.yaml` / `.yml` (case-insensitive), same as Verdaccio:
    /// `config file must be a YAML file (.yaml or .yml)`.
    #[error("config file must be a YAML file (.yaml or .yml): {0}")]
    NotYaml(PathBuf),
    /// Invalid YAML, or a document that is not a mapping. Verdaccio's message:
    /// `CONFIG: it does not look like a valid config file`.
    #[error("CONFIG: it does not look like a valid config file ({path}): {message}")]
    Parse {
        /// File path.
        path: PathBuf,
        /// Parser message.
        message: String,
    },
    /// A value Verdaccio rejects at startup. The message follows Verdaccio's assertion text,
    /// e.g. `CONFIG: no url for uplink: npmjs`, `CONFIG: reserved uplink name: all`,
    /// `CONFIG: bad "packages" value (object expected)`, `invalid interval: 5x`.
    #[error("{0}")]
    Invalid(String),
}

/// The loaded configuration. Immutable after load; share it as `Arc<Config>`.
#[derive(Debug, Clone)]
pub struct Config {
    /// Absolute path of the config file. Relative paths in the file (`storage`, `log.path`)
    /// are resolved against its directory.
    pub config_path: PathBuf,
    /// Absolute storage root. `VERDACCIO_STORAGE_PATH` wins over `storage`; a relative value is
    /// resolved against the config file directory (`local-database.ts:getStoragePath`).
    /// Missing in both places is a [`ConfigError::Invalid`] (`storage field is mandatory`).
    pub storage: PathBuf,
    /// Raw `listen` entries (a string becomes a one-element list; non-string items are
    /// dropped). Empty when absent. Resolve with [`Config::listen_address`].
    pub listen: Vec<String>,
    /// `url_prefix` as written (not yet wrapped with slashes; `fastopm-render` does that).
    pub url_prefix: Option<String>,
    /// Uplinks keyed by name, in JavaScript object key order (see [`Config::packages`]).
    pub uplinks: IndexMap<String, UplinkConfig>,
    /// `packages` rules in JavaScript object key order, which is what Verdaccio iterates:
    /// integer-like keys (`"0"` or `[1-9][0-9]*` below 2^32 - 1) first in ascending numeric
    /// order, then the other keys in file order. A `**` rule with no proxy is appended when the
    /// file has none (`normalisePackageAccess`).
    pub packages: Vec<PackageRule>,
    /// `log` settings.
    pub log: LogConfig,
    /// `http_proxy`; when the key is absent, the `http_proxy` then `HTTP_PROXY` env vars.
    pub http_proxy: Option<String>,
    /// `https_proxy`; when the key is absent, the `https_proxy` then `HTTPS_PROXY` env vars.
    pub https_proxy: Option<String>,
    /// `no_proxy` split on `,` (a YAML list is taken as is); when the key is absent, the
    /// `no_proxy` then `NO_PROXY` env vars.
    pub no_proxy: Vec<String>,
    /// Text sent to uplinks inside `User-Agent: npm (<user_agent>)`. `hidden` when `user_agent`
    /// is absent; otherwise the YAML value converted like JavaScript `String(value)`.
    pub user_agent: String,
    /// `server_id`, or 12 random hex characters when absent. Used in `Via` for loop detection.
    pub server_id: String,
    /// `VERDACCIO_PUBLIC_URL`, kept only when it parses as an absolute `http`/`https` URL.
    pub public_url: Option<String>,
    /// Lower-cased name of the header carrying the client protocol:
    /// `VERDACCIO_FORWARDED_PROTO` or `x-forwarded-proto`.
    pub forwarded_proto_header: String,
    /// The `fastopm:` section (fastopm-only options, all off by default).
    pub fastopm: FastopmConfig,
    /// Human-readable warnings collected during load (ignored keys, ignored `access` rules,
    /// multiple listen addresses...). The binary logs them once at startup.
    pub warnings: Vec<String>,
}

/// The top-level `fastopm:` section: improvements over Verdaccio that change what clients see,
/// each off by default, and resource settings Verdaccio does not have (memory budgets). Absent or `null` → all defaults. Unknown keys inside it produce a
/// warning; a value of the wrong type is a [`ConfigError::Invalid`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FastopmConfig {
    /// `refresh_on_miss` (default `false`): when `GET /:package/:version` or
    /// `GET /:package/-/:filename` asks for a version, dist-tag or tarball the cached manifest
    /// does not have, and that manifest is being served from cache because it is within
    /// `maxage`, sync it with the uplinks now (ignoring `maxage`) and look again. Verdaccio
    /// answers 404 until `maxage` expires.
    pub refresh_on_miss: bool,
    /// `refresh_on_miss_min_interval` (default `30s`): minimum time between two such forced
    /// syncs of the same package, so clients asking for versions that do not exist cannot turn
    /// every request into an uplink request. Same interval syntax as `maxage`; `0` removes the
    /// limit (concurrent requests still share one sync).
    pub refresh_on_miss_min_interval: Duration,
    /// Memory budgets: `memory_limit`, `manifest_cache`, `render_cache`, `work_memory` (sizes
    /// such as `512mb`; `memory_limit: auto` is the default). Absent options are derived from
    /// the container memory limit, see [`memory::MemoryPlan::resolve`].
    pub memory: MemoryOptions,
}

impl Default for FastopmConfig {
    fn default() -> Self {
        Self {
            refresh_on_miss: false,
            refresh_on_miss_min_interval: Duration::from_secs(30),
            memory: MemoryOptions::default(),
        }
    }
}

/// One entry of `uplinks`.
///
/// Defaults are Verdaccio's (`packages/proxy/src/proxy.ts`, constructor). A key present with
/// value `null` takes the default (`setConfig` uses `isNil`).
#[derive(Debug, Clone, PartialEq)]
pub struct UplinkConfig {
    /// Uplink name (the key under `uplinks`). Never `all`, `owner`, `anonymous`, `undefined`,
    /// `none`, and never contains whitespace (`sanityCheckNames`).
    pub name: String,
    /// Registry URL with one trailing `/` removed. Required, must be a string.
    pub url: String,
    /// Per-request timeout, default `30s`.
    pub timeout: Duration,
    /// How long a cached manifest is used without asking this uplink, default `2m`.
    pub maxage: Duration,
    /// Circuit-breaker threshold, default `2`.
    pub max_fails: u32,
    /// How long the breaker stays open, default `5m`.
    pub fail_timeout: Duration,
    /// Per-request retry limit, default `2` (`retry` in Verdaccio; a number, or an object whose
    /// `limit` is used).
    pub retry: u32,
    /// `false`: stream tarballs without writing them to disk. Default `true`.
    pub cache: bool,
    /// Extra headers sent to the uplink, added last (override computed headers except
    /// `If-None-Match` and `Accept` on conditional requests).
    pub headers: IndexMap<String, String>,
    /// Token sent as `Authorization` unless `headers` already has one.
    pub auth: Option<UplinkAuth>,
    /// Per-uplink `http_proxy` (wins over the global one when the key is present).
    /// `Some("")` when the key is present but `null` or empty: no proxy for this uplink, the
    /// global value is not used ([`Config::effective_proxy`] returns `None`).
    pub http_proxy: Option<String>,
    /// Per-uplink `https_proxy`, same rules as `http_proxy`.
    pub https_proxy: Option<String>,
    /// Per-uplink `no_proxy`; `None` when the key is absent (then the global list applies).
    pub no_proxy: Option<Vec<String>>,
    /// `agent_options.maxSockets`: requests in flight to this uplink at once. Verdaccio's
    /// default `agent_options` gives 40; an `agent_options` without `maxSockets` gives Node.js'
    /// `Infinity` (`None`).
    pub max_sockets: Option<u32>,
    /// `agent_options.maxFreeSockets`: idle connections kept (10 by default, Node.js' 256 when
    /// `agent_options` is set without it).
    pub max_free_sockets: usize,
}

/// `uplinks.<name>.auth`.
///
/// Kept as written: like Verdaccio, problems are reported per request by `fastopm-uplink`
/// (`setAuthNext`), never at load time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UplinkAuth {
    /// `type` as written, `None` when absent (then `Basic`). The uplink accepts `basic` and
    /// `bearer` case-insensitively and sends the value capitalized; anything else fails each
    /// request with `Auth type '<type>' not allowed`.
    pub auth_type: Option<String>,
    /// Where the token comes from.
    pub token: TokenSource,
}

/// Source of an uplink token, resolved on every request (`setAuthNext`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TokenSource {
    /// `auth.token: <string>`.
    Inline(String),
    /// `auth.token_env: <VAR>`, or `NPM_TOKEN` when `token_env` is `true` or neither `token`
    /// (as a string) nor `token_env` is set. An unset variable fails the request with
    /// `token is required`.
    Env(String),
    /// `token_env` present but neither a string nor `true`: every request fails with
    /// `token is required`.
    Missing,
    /// `auth` is not a mapping (and has no mapping `token`): every request fails with
    /// `Auth invalid`.
    Invalid,
}

/// One entry of `packages`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackageRule {
    /// The key, a `minimatch` pattern such as `@*/*` or `**`.
    pub pattern: String,
    /// Uplink names from `proxy` (a space-separated string or a list), in order. Names not in
    /// `uplinks` are kept; lookups skip them.
    pub proxy: Vec<String>,
    /// `storage`: sub-folder of the storage root for matching packages.
    pub storage: Option<String>,
}

/// `log` settings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogConfig {
    /// Destination (`type`), default stdout.
    pub target: LogTarget,
    /// `format`, default `pretty`.
    pub format: LogFormat,
    /// `level`, default `http`.
    pub level: LogLevel,
    /// `colors`: `Some(false)` never colors; otherwise colors only when stdout is a terminal
    /// (Verdaccio's `hasColors` + `colorette`: `NO_COLOR` disables, `FORCE_COLOR` forces). Never
    /// for `type: file`.
    pub colors: Option<bool>,
}

impl Default for LogConfig {
    fn default() -> Self {
        Self {
            target: LogTarget::Stdout,
            format: LogFormat::Pretty,
            level: LogLevel::Http,
            colors: None,
        }
    }
}

/// Where logs go.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LogTarget {
    /// `type: stdout`
    Stdout,
    /// `type: file` with `path` (resolved against the config file directory).
    File(PathBuf),
}

/// Log line format.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogFormat {
    /// `pretty`
    Pretty,
    /// `pretty-timestamped`
    PrettyTimestamped,
    /// `json`
    Json,
}

/// Log level, Verdaccio's names (`http` sits between `warn` and `info`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum LogLevel {
    /// `fatal`
    Fatal,
    /// `error`
    Error,
    /// `warn`
    Warn,
    /// `http`
    Http,
    /// `info`
    Info,
    /// `debug`
    Debug,
    /// `trace`
    Trace,
}

/// Locate the config file (`findConfigFile` in `config-path.ts`).
///
/// With `cli_path`: that path made absolute against the current directory, even if it does not
/// exist (loading reports the error). Without it, the first existing file among:
///
/// 1. `$XDG_CONFIG_HOME/verdaccio/config.yaml`, or `$HOME/.config/verdaccio/config.yaml`
///    (only considered when that base directory exists);
/// 2. on Windows, `%APPDATA%/verdaccio/config.yaml` (when `APPDATA` exists);
/// 3. `./verdaccio/config.yaml`;
/// 4. `./config.yaml`.
///
/// When none exists, Verdaccio's default config ([`default_config_yaml`]) is written to the
/// first candidate (creating its folder) and that path is returned. For the XDG candidate the
/// line `storage: ./storage` becomes `storage: <data>/verdaccio/storage`, where `<data>` is
/// `$XDG_DATA_HOME` or `$HOME/.local/share`, only when that directory exists
/// (`updateStorageLinks`). A write failure is [`ConfigError::Io`].
pub fn find_config_file(
    cli_path: Option<&Path>,
    env: EnvLookup<'_>,
) -> Result<PathBuf, ConfigError> {
    load::find_config_file(cli_path, env)
}

/// Verdaccio's `packages/config/src/conf/default.yaml`, byte for byte (comments included), as
/// written by [`find_config_file`] when no config exists. Keys fastopm ignores in it (`auth`,
/// `web`, `middlewares`, ...) only produce load warnings.
pub fn default_config_yaml() -> &'static str {
    include_str!("default.yaml")
}

impl Config {
    /// Find, read and parse the config with the process environment
    /// ([`find_config_file`] then [`Config::from_yaml_str`]).
    pub fn load(cli_path: Option<&Path>) -> Result<Config, ConfigError> {
        let path = find_config_file(cli_path, &process_env)?;
        load::read_config(&path, &process_env)
    }

    /// Parse YAML text as if read from `config_path` (must be absolute).
    ///
    /// YAML rules follow Verdaccio's js-yaml setup: core schema plus merge keys (`<<`); an empty
    /// or comment-only document is an empty config. Applies defaults, env overrides
    /// (`VERDACCIO_STORAGE_PATH`, `VERDACCIO_PUBLIC_URL`, `VERDACCIO_FORWARDED_PROTO`,
    /// `http_proxy`/`https_proxy`/`no_proxy` and their upper-case forms) and the startup checks
    /// Verdaccio does (uplink names and URLs, `packages` shape, intervals).
    pub fn from_yaml_str(
        yaml: &str,
        config_path: &Path,
        env: EnvLookup<'_>,
    ) -> Result<Config, ConfigError> {
        load::from_yaml_str(yaml, config_path, env)
    }

    /// First rule whose pattern matches `name` ([`pattern_matches`], case-insensitive, declaration
    /// order). `None` only if no rule matches (cannot happen when the implicit `**` rule exists).
    pub fn package_rule(&self, name: &str) -> Option<&PackageRule> {
        self.packages.iter().find(|rule| pattern_matches(&rule.pattern, name))
    }

    /// Uplinks to try for `name`, in `proxy` order, skipping names absent from `uplinks`
    /// (`getProxiesForPackage`). Empty when the package is local-only. A name listed twice is
    /// tried once.
    pub fn uplinks_for(&self, name: &str) -> Vec<&UplinkConfig> {
        let Some(rule) = self.package_rule(name) else {
            return Vec::new();
        };
        let mut out: Vec<&UplinkConfig> = Vec::new();
        for uplink in rule.proxy.iter().filter_map(|n| self.uplinks.get(n)) {
            if !out.iter().any(|u| u.name == uplink.name) {
                out.push(uplink);
            }
        }
        out
    }

    /// Proxy URL to use for `uplink`, or `None` for a direct connection (`_setupProxy`).
    ///
    /// Picks `https_proxy` for an `https:` uplink URL and `http_proxy` otherwise, from the
    /// uplink when it sets the key, else from the global config. Then drops it when the uplink
    /// host matches `no_proxy` (uplink list when present, else global), wget style: both sides
    /// get a leading `.` and the host must end with the entry.
    pub fn effective_proxy<'a>(&'a self, uplink: &'a UplinkConfig) -> Option<&'a str> {
        let parsed = url::Url::parse(&uplink.url).ok();
        let https = parsed.as_ref().is_some_and(|u| u.scheme() == "https");
        let (own, global) = if https {
            (&uplink.https_proxy, &self.https_proxy)
        } else {
            (&uplink.http_proxy, &self.http_proxy)
        };
        let proxy = own.as_deref().or(global.as_deref()).filter(|p| !p.is_empty())?;
        let host = parsed.as_ref().and_then(|u| u.host_str()).unwrap_or_default();
        let host = if host.starts_with('.') { host.to_owned() } else { format!(".{host}") };
        let no_proxy = uplink.no_proxy.as_ref().unwrap_or(&self.no_proxy);
        let excluded = no_proxy.iter().any(|entry| {
            if entry.starts_with('.') {
                host.ends_with(entry.as_str())
            } else {
                host.ends_with(&format!(".{entry}"))
            }
        });
        if excluded { None } else { Some(proxy) }
    }

    /// Address to bind (`getListenAddress`): `cli_override` (`--listen`) when given, else the
    /// first valid entry of [`Config::listen`], else `http://localhost:4873`.
    ///
    /// Errors: an invalid `cli_override` or single entry; a list with no valid entry. Invalid
    /// list entries and extra valid entries are skipped (Verdaccio logs a warning for each).
    pub fn listen_address(&self, cli_override: Option<&str>) -> Result<ListenAddress, ConfigError> {
        let invalid = |addr: &str| {
            ConfigError::Invalid(format!(
                "Invalid address - {addr}, we expect a port (e.g. \"4873\"), host:port (e.g. \"localhost:4873\"), \
                 full url (e.g. \"http://localhost:4873/\") or unix:/path/socket"
            ))
        };
        let default = || ListenAddress::Tcp {
            scheme: Scheme::Http,
            host: fastopm_core::constants::DEFAULT_DOMAIN.to_owned(),
            port: fastopm_core::constants::DEFAULT_PORT,
        };
        match (cli_override, self.listen.as_slice()) {
            (Some(cli), _) if !cli.is_empty() => parse_address(cli).ok_or_else(|| invalid(cli)),
            (_, []) => Ok(default()),
            (_, [one]) if one.is_empty() => Ok(default()),
            (_, [one]) => parse_address(one).ok_or_else(|| invalid(one)),
            (_, many) => many.iter().find_map(|a| parse_address(a)).ok_or_else(|| {
                ConfigError::Invalid(
                    "No valid listen addresses found in configuration array".to_owned(),
                )
            }),
        }
    }
}
