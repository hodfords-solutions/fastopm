//! Locating, reading and interpreting the config file (`config-path.ts`, `parse.ts`,
//! `config.ts`, `uplinks.ts`, `package-access.ts`, `Proxy` constructor in `proxy.ts`).

use std::path::{Component, Path, PathBuf};
use std::time::Duration;

use fastopm_core::constants::env as env_keys;
use fastopm_core::interval::{Interval, parse_interval};
use indexmap::IndexMap;

use crate::js::{self, Js, Obj, is_js_space};
use crate::{
    Config, ConfigError, EnvLookup, FastopmConfig, LogConfig, LogFormat, LogLevel, LogTarget,
    PackageRule, TokenSource, UplinkAuth, UplinkConfig, parse_address,
};

/// Top-level keys fastopm reads; any other key produces a warning.
const KNOWN_KEYS: [&str; 12] = [
    "storage",
    "listen",
    "uplinks",
    "packages",
    "url_prefix",
    "log",
    "http_proxy",
    "https_proxy",
    "no_proxy",
    "user_agent",
    "server_id",
    "fastopm",
];

/// Keys of the `fastopm:` section.
const FASTOPM_KEYS: [&str; 6] = [
    "refresh_on_miss",
    "refresh_on_miss_min_interval",
    "memory_limit",
    "manifest_cache",
    "render_cache",
    "work_memory",
];

/// Uplink keys fastopm reads.
const UPLINK_KEYS: [&str; 13] = [
    "url",
    "timeout",
    "maxage",
    "max_fails",
    "fail_timeout",
    "retry",
    "cache",
    "headers",
    "auth",
    "http_proxy",
    "https_proxy",
    "no_proxy",
    // read by Verdaccio, harmless to ignore silently
    "agent_options",
];

/// Groups an anonymous request has in Verdaccio (`createAnonymousRemoteUser`).
const ANONYMOUS_GROUPS: [&str; 4] = ["$all", "$anonymous", "@all", "@anonymous"];

const RESERVED_UPLINK_NAMES: [&str; 5] = ["all", "owner", "anonymous", "undefined", "none"];

fn non_empty(v: Option<String>) -> Option<String> {
    v.filter(|s| !s.is_empty())
}

/// `path.resolve`-style lexical normalization of an absolute path (no filesystem access).
fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in path.components() {
        match c {
            Component::ParentDir => {
                if !out.pop() {
                    out.push(c);
                }
            }
            Component::CurDir => {}
            other => out.push(other),
        }
    }
    out
}

fn absolute(path: &Path) -> PathBuf {
    if path.is_absolute() {
        normalize(path)
    } else {
        let cwd = std::env::current_dir().unwrap_or_default();
        normalize(&cwd.join(path))
    }
}

/// A relative path is resolved against `base`; an absolute one is kept as written
/// (`getStoragePath`).
fn resolve_against(base: &Path, p: &str) -> PathBuf {
    let path = Path::new(p);
    if path.is_absolute() { path.to_path_buf() } else { normalize(&base.join(path)) }
}

struct Candidate {
    path: PathBuf,
    xdg: bool,
}

pub(crate) fn find_config_file(
    cli_path: Option<&Path>,
    env: EnvLookup<'_>,
) -> Result<PathBuf, ConfigError> {
    if let Some(cli) = cli_path {
        return Ok(absolute(cli));
    }
    let home = non_empty(env("HOME"));
    let mut candidates = Vec::new();
    let xdg_config = non_empty(env("XDG_CONFIG_HOME"))
        .map(PathBuf::from)
        .or_else(|| home.as_ref().map(|h| Path::new(h).join(".config")));
    if let Some(dir) = xdg_config.filter(|d| d.is_dir()) {
        candidates.push(Candidate {
            path: normalize(&dir.join("verdaccio").join("config.yaml")),
            xdg: true,
        });
    }
    if cfg!(windows)
        && let Some(appdata) = non_empty(env("APPDATA")).map(PathBuf::from).filter(|d| d.is_dir())
    {
        candidates.push(Candidate {
            path: absolute(&appdata.join("verdaccio").join("config.yaml")),
            xdg: false,
        });
    }
    candidates.push(Candidate { path: absolute(Path::new("verdaccio/config.yaml")), xdg: false });
    candidates.push(Candidate { path: absolute(Path::new("config.yaml")), xdg: false });

    if let Some(found) = candidates.iter().find(|c| c.path.is_file()) {
        return Ok(found.path.clone());
    }
    let first = &candidates[0];
    let io = |source| ConfigError::Io { path: first.path.clone(), source };
    if let Some(parent) = first.path.parent() {
        std::fs::create_dir_all(parent).map_err(io)?;
    }
    let mut yaml = crate::default_config_yaml().to_owned();
    if first.xdg {
        let data_dir = non_empty(env("XDG_DATA_HOME"))
            .map(PathBuf::from)
            .or_else(|| home.as_ref().map(|h| Path::new(h).join(".local").join("share")));
        if let Some(data_dir) = data_dir.filter(|d| d.is_dir()) {
            let storage = absolute(&data_dir.join("verdaccio").join("storage"));
            yaml = replace_storage_line(&yaml, &storage.to_string_lossy());
        }
    }
    std::fs::write(&first.path, yaml).map_err(io)?;
    Ok(first.path.clone())
}

/// `defaultConfig.replace(/^storage: .\/storage$/m, 'storage: <dir>')`: first match only.
fn replace_storage_line(yaml: &str, storage: &str) -> String {
    let mut out = String::with_capacity(yaml.len() + storage.len());
    let mut done = false;
    for line in yaml.split_inclusive('\n') {
        let body = line.trim_end_matches(['\n', '\r']);
        let is_match = body.len() == "storage: ./storage".len()
            && body.starts_with("storage: ")
            && body.ends_with("/storage")
            && body.chars().nth(9).is_some_and(|c| c != '\u{2028}' && c != '\u{2029}');
        if !done && is_match {
            out.push_str("storage: ");
            out.push_str(storage);
            out.push_str(&line[body.len()..]);
            done = true;
        } else {
            out.push_str(line);
        }
    }
    out
}

/// `parseConfigFile`: the file must exist, be `.yaml`/`.yml`, and parse.
pub(crate) fn read_config(path: &Path, env: EnvLookup<'_>) -> Result<Config, ConfigError> {
    if !path.is_file() {
        return Err(ConfigError::Io {
            path: path.to_path_buf(),
            source: std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "config file does not exist or not reachable",
            ),
        });
    }
    let is_yaml = path
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("yaml") || e.eq_ignore_ascii_case("yml"));
    if !is_yaml {
        return Err(ConfigError::NotYaml(path.to_path_buf()));
    }
    let bytes = std::fs::read(path)
        .map_err(|source| ConfigError::Io { path: path.to_path_buf(), source })?;
    from_yaml_str(&String::from_utf8_lossy(&bytes), path, env)
}

fn invalid(message: impl Into<String>) -> ConfigError {
    ConfigError::Invalid(message.into())
}

/// `uplinks` / `packages`: absent or `null` is `{}`, anything but a plain object is an error.
fn strategic<'a>(root: &'a Obj, key: &str) -> Result<Option<&'a Obj>, ConfigError> {
    match root.get(key) {
        None | Some(Js::Null) => Ok(None),
        Some(Js::Obj(o)) => Ok(Some(o)),
        Some(_) => Err(invalid(format!("CONFIG: bad \"{key}\" value (object expected)"))),
    }
}

pub(crate) fn from_yaml_str(
    yaml: &str,
    config_path: &Path,
    env: EnvLookup<'_>,
) -> Result<Config, ConfigError> {
    let config_path = absolute(config_path);
    let parse_error = |message: String| ConfigError::Parse { path: config_path.clone(), message };
    let root = match js::parse(yaml).map_err(parse_error)? {
        Js::Null => Obj::default(),
        Js::Obj(o) => o,
        other => return Err(parse_error(format!("expected a mapping, got {}", other.to_json()))),
    };
    let base_dir = config_path.parent().map(Path::to_path_buf).unwrap_or_default();
    let mut warnings = Vec::new();
    for (key, _) in root.iter() {
        if !KNOWN_KEYS.contains(&key) {
            warnings.push(format!("config key `{key}` is not supported by fastopm and is ignored"));
        }
    }

    let empty = Obj::default();
    let uplinks_obj = strategic(&root, "uplinks")?.unwrap_or(&empty);
    let packages_obj = strategic(&root, "packages")?.unwrap_or(&empty);
    let uplink_entries = uplinks_obj.js_ordered();

    // uplinkSanityCheck, then sanityCheckUplinksProps
    for (name, _) in &uplink_entries {
        if RESERVED_UPLINK_NAMES.contains(name) {
            return Err(invalid(format!("CONFIG: reserved uplink name: {name}")));
        }
        if name.chars().any(is_js_space) {
            return Err(invalid(format!("CONFIG: invalid uplink name: {name}")));
        }
    }
    let mut urls = Vec::new();
    for (name, value) in &uplink_entries {
        let url_value = match value {
            Js::Obj(o) => o.get("url"),
            _ => None,
        };
        let url = match url_value {
            Some(Js::Str(s)) if !s.is_empty() => s.as_str(),
            Some(v) if v.truthy() => {
                return Err(invalid(format!("CONFIG: wrong url format for uplink: {name}")));
            }
            _ => return Err(invalid(format!("CONFIG: no url for uplink: {name}"))),
        };
        if url::Url::parse(url).is_err() {
            return Err(invalid(format!(
                "CONFIG: invalid url for uplink: {name}: Invalid URL: {url}"
            )));
        }
        urls.push(url.strip_suffix('/').unwrap_or(url).to_owned());
    }

    let packages = package_rules(packages_obj, &mut warnings)?;

    let storage_value = non_empty(env(env_keys::STORAGE_PATH)).map(Js::Str);
    let storage = match storage_value.as_ref().or(root.get("storage")) {
        Some(Js::Str(s)) => resolve_against(&base_dir, s),
        _ => return Err(invalid("storage field is mandatory")),
    };

    let mut uplinks = IndexMap::new();
    for ((name, value), url) in uplink_entries.iter().zip(urls) {
        let o = match value {
            Js::Obj(o) => o,
            _ => &empty,
        };
        let uplink = uplink_config(name, url, o, &mut warnings)?;
        uplinks.insert((*name).to_owned(), uplink);
    }

    let listen = listen_entries(root.get("listen"), &mut warnings);

    let url_prefix = match root.get("url_prefix") {
        None | Some(Js::Null) => None,
        Some(v @ (Js::Arr(_) | Js::Obj(_))) => {
            warnings.push(format!("url_prefix must be a string, got {}; ignored", v.to_json()));
            None
        }
        Some(v) => Some(v.to_js_string()),
    };

    let log = log_config(root.get("log"), &base_dir, &mut warnings);
    let fastopm = fastopm_config(root.get("fastopm"), &mut warnings)?;

    let global_proxy = |key: &str| match root.get(key) {
        Some(v) => proxy_value(v).filter(|p| !p.is_empty()),
        None => non_empty(env(key)).or_else(|| non_empty(env(&key.to_uppercase()))),
    };
    let http_proxy = global_proxy("http_proxy");
    let https_proxy = global_proxy("https_proxy");
    let no_proxy = match root.get("no_proxy") {
        Some(v) => no_proxy_list(v),
        None => non_empty(env("no_proxy"))
            .or_else(|| non_empty(env("NO_PROXY")))
            .map(|s| no_proxy_list(&Js::Str(s)))
            .unwrap_or_default(),
    };

    let user_agent = match root.get("user_agent") {
        None | Some(Js::Null) => "hidden".to_owned(),
        Some(v) => v.to_js_string(),
    };
    let server_id = match root.get("server_id") {
        Some(v) if v.truthy() => v.to_js_string(),
        _ => fastopm_core::util::random_hex(6),
    };

    let public_url = env(env_keys::PUBLIC_URL)
        .filter(|u| url::Url::parse(u).is_ok_and(|p| matches!(p.scheme(), "http" | "https")));
    let forwarded_proto_header = env(env_keys::FORWARDED_PROTO)
        .map_or_else(|| "x-forwarded-proto".to_owned(), |h| h.to_lowercase());

    Ok(Config {
        config_path,
        storage,
        listen,
        url_prefix,
        uplinks,
        packages,
        log,
        http_proxy,
        https_proxy,
        no_proxy,
        user_agent,
        server_id,
        public_url,
        forwarded_proto_header,
        fastopm,
        warnings,
    })
}

/// The `fastopm:` section. Its own options, so values of the wrong type fail the load instead
/// of being guessed at (Verdaccio never reads this section).
fn fastopm_config(
    value: Option<&Js>,
    warnings: &mut Vec<String>,
) -> Result<FastopmConfig, ConfigError> {
    let mut out = FastopmConfig::default();
    let o = match value {
        None | Some(Js::Null) => return Ok(out),
        Some(Js::Obj(o)) => o,
        Some(other) => {
            return Err(invalid(format!(
                "CONFIG: bad \"fastopm\" value (object expected): {}",
                other.to_json()
            )));
        }
    };
    for (key, _) in o.iter() {
        if !FASTOPM_KEYS.contains(&key) {
            warnings.push(format!("fastopm: unknown option `{key}` is ignored"));
        }
    }
    match o.get("refresh_on_miss") {
        None | Some(Js::Null) => {}
        Some(Js::Bool(b)) => out.refresh_on_miss = *b,
        Some(other) => {
            return Err(invalid(format!(
                "CONFIG: fastopm.refresh_on_miss must be true or false, got {}",
                other.to_json()
            )));
        }
    }
    let min_interval = o.get("refresh_on_miss_min_interval").filter(|v| !matches!(v, Js::Null));
    if min_interval.is_some() {
        out.refresh_on_miss_min_interval = interval(min_interval, "30s")
            .map_err(|e| invalid(format!("CONFIG: fastopm.refresh_on_miss_min_interval: {e}")))?;
    }
    out.memory.memory_limit = size_option(o.get("memory_limit"), "memory_limit", true)?;
    out.memory.manifest_cache = size_option(o.get("manifest_cache"), "manifest_cache", false)?;
    out.memory.render_cache = size_option(o.get("render_cache"), "render_cache", false)?;
    out.memory.work_memory = size_option(o.get("work_memory"), "work_memory", false)?;
    Ok(out)
}

/// A size option of the `fastopm:` section ([`crate::memory::parse_size`]); `null`, absent
/// (and `auto` when `auto` is allowed) → `None`.
fn size_option(value: Option<&Js>, key: &str, auto: bool) -> Result<Option<u64>, ConfigError> {
    let bad = |v: &Js| {
        invalid(format!(
            "CONFIG: fastopm.{key}: invalid size {} (expected bytes or a number with a unit: kb, mb, gb{})",
            v.to_json(),
            if auto { ", or auto" } else { "" }
        ))
    };
    match value {
        None | Some(Js::Null) => Ok(None),
        Some(Js::Str(s)) if auto && s.trim().eq_ignore_ascii_case("auto") => Ok(None),
        Some(v @ Js::Str(s)) => crate::memory::parse_size(s).map(Some).ok_or_else(|| bad(v)),
        // `as` saturates at u64::MAX
        Some(Js::Num(n)) if n.is_finite() && *n >= 0.0 => Ok(Some(n.floor() as u64)),
        Some(v) => Err(bad(v)),
    }
}

/// `normalizeUserList`: `null`/empty → `[]`, a string is split on white space, a list is kept
/// (non-string items dropped), a non-empty object is an error.
fn user_list(value: Option<&Js>) -> Result<Vec<String>, ConfigError> {
    Ok(match value {
        Some(Js::Str(s)) => {
            s.split(is_js_space).filter(|p| !p.is_empty()).map(str::to_owned).collect()
        }
        Some(Js::Arr(items)) => items
            .iter()
            .filter_map(|i| match i {
                Js::Str(s) if !s.is_empty() => Some(s.clone()),
                _ => None,
            })
            .collect(),
        Some(Js::Obj(o)) if !o.is_empty() => {
            return Err(invalid(format!(
                "CONFIG: bad package acl (array or string expected): {}",
                Js::Obj(o.clone()).to_json()
            )));
        }
        _ => Vec::new(),
    })
}

/// `normalisePackageAccess` plus the fields fastopm uses.
fn package_rules(
    packages: &Obj,
    warnings: &mut Vec<String>,
) -> Result<Vec<PackageRule>, ConfigError> {
    let mut rules = Vec::new();
    for (pattern, value) in packages.js_ordered() {
        let Js::Obj(rule) = value else {
            return Err(invalid(format!(
                "CONFIG: bad \"'{pattern}'\" package description (object expected)"
            )));
        };
        let access = user_list(rule.get("access"))?;
        user_list(rule.get("publish"))?;
        let proxy = user_list(rule.get("proxy"))?;
        user_list(rule.get("unpublish"))?;
        user_list(rule.get("stage"))?;
        if !access.iter().any(|g| ANONYMOUS_GROUPS.contains(&g.as_str())) {
            let who = if access.is_empty() { "nobody".to_owned() } else { access.join(" ") };
            warnings.push(format!(
                "packages `{pattern}`: access ({who}) is not enforced; fastopm serves these packages to every client"
            ));
        }
        let storage = match rule.get("storage") {
            None | Some(Js::Null) => None,
            Some(Js::Str(s)) => Some(s.clone()),
            Some(other) => {
                warnings.push(format!(
                    "packages `{pattern}`: storage must be a string, got {}; ignored",
                    other.to_json()
                ));
                None
            }
        };
        rules.push(PackageRule { pattern: pattern.to_owned(), proxy, storage });
    }
    if !packages.has("**") {
        warnings.push(
            "packages: no `**` rule; Verdaccio refuses packages that match no rule, fastopm serves them from storage only"
                .to_owned(),
        );
        rules.push(PackageRule { pattern: "**".to_owned(), proxy: Vec::new(), storage: None });
    }
    Ok(rules)
}

/// `listen`: a string (or other scalar) is one entry, a list keeps its strings.
fn listen_entries(value: Option<&Js>, warnings: &mut Vec<String>) -> Vec<String> {
    let entries = match value {
        None | Some(Js::Null) => return Vec::new(),
        Some(Js::Arr(items)) => {
            let strings: Vec<String> = items
                .iter()
                .filter_map(|i| match i {
                    Js::Str(s) => Some(s.clone()),
                    _ => None,
                })
                .collect();
            if strings.is_empty() {
                warnings.push(
                    "listen: the list has no string entry (Verdaccio refuses to start); using the default address"
                        .to_owned(),
                );
            }
            strings
        }
        Some(v) => vec![v.to_js_string()],
    };
    if entries.len() > 1 {
        for bad in entries.iter().filter(|e| parse_address(e).is_none()) {
            warnings.push(format!(
                "invalid address - {bad}, we expect a port (e.g. \"4873\"), host:port (e.g. \"localhost:4873\"), \
                 full url (e.g. \"http://localhost:4873/\") or unix:/path/socket"
            ));
        }
        if let Some(first) = entries.iter().find_map(|e| parse_address(e)) {
            warnings.push(format!(
                "Multiple listen addresses are not supported, using the first valid one {first}"
            ));
        }
    }
    entries
}

fn log_config(value: Option<&Js>, base_dir: &Path, warnings: &mut Vec<String>) -> LogConfig {
    let mut log = LogConfig::default();
    let o = match value {
        None | Some(Js::Null) => return log,
        Some(Js::Obj(o)) => o,
        Some(other) => {
            warnings.push(format!(
                "log must be a mapping, got {}; using the defaults",
                other.to_json()
            ));
            return log;
        }
    };
    match o.get("type") {
        None | Some(Js::Null) => {}
        Some(Js::Str(t)) if t == "stdout" => {}
        Some(Js::Str(t)) if t == "file" => match o.get("path") {
            Some(Js::Str(p)) if !p.is_empty() => {
                log.target = LogTarget::File(resolve_against(base_dir, p))
            }
            _ => warnings.push("log: type `file` needs a `path`; logging to stdout".to_owned()),
        },
        Some(other) => {
            warnings.push(format!("log: unsupported type {}; logging to stdout", other.to_json()));
        }
    }
    match o.get("format") {
        None | Some(Js::Null) => {}
        Some(Js::Str(f)) if f == "pretty" => log.format = LogFormat::Pretty,
        Some(Js::Str(f)) if f == "pretty-timestamped" => log.format = LogFormat::PrettyTimestamped,
        Some(Js::Str(f)) if f == "json" => log.format = LogFormat::Json,
        Some(other) => {
            warnings.push(format!("log: unsupported format {}; using pretty", other.to_json()))
        }
    }
    match o.get("level") {
        Some(v) if v.truthy() => {
            let level = match v {
                Js::Str(l) => match l.as_str() {
                    "fatal" => Some(LogLevel::Fatal),
                    "error" => Some(LogLevel::Error),
                    "warn" => Some(LogLevel::Warn),
                    "http" => Some(LogLevel::Http),
                    "info" => Some(LogLevel::Info),
                    "debug" => Some(LogLevel::Debug),
                    "trace" => Some(LogLevel::Trace),
                    _ => None,
                },
                _ => None,
            };
            match level {
                Some(level) => log.level = level,
                None => warnings.push(format!("log: unknown level {}; using http", v.to_json())),
            }
        }
        _ => {}
    }
    match o.get("colors") {
        None | Some(Js::Null) => {}
        // `hasColors`: a truthy value asks for colors when supported, a falsy one disables them
        Some(v) => log.colors = Some(v.truthy()),
    }
    log
}

/// `http_proxy` / `https_proxy` value as `String(value)`; `null` and non-strings that are not
/// scalars mean "no proxy".
fn proxy_value(v: &Js) -> Option<String> {
    match v {
        Js::Null | Js::Arr(_) | Js::Obj(_) => None,
        Js::Str(s) => Some(s.clone()),
        other => Some(other.to_js_string()),
    }
}

/// `no_proxy`: a non-empty string is split on `,`, a list is taken item by item, anything else
/// excludes nothing.
fn no_proxy_list(v: &Js) -> Vec<String> {
    match v {
        Js::Str(s) if !s.is_empty() => s.split(',').map(str::to_owned).collect(),
        Js::Arr(items) => {
            items.iter().filter(|i| !matches!(i, Js::Null)).map(Js::to_js_string).collect()
        }
        _ => Vec::new(),
    }
}

/// `parseInterval(setConfig(config, key, default))`.
fn interval(value: Option<&Js>, default: &str) -> Result<Duration, ConfigError> {
    let bad = |v: &Js| invalid(format!("invalid interval: {}", v.to_js_string()));
    match value {
        None | Some(Js::Null) => parse_interval(default).map_err(|e| invalid(e.to_string())),
        Some(Js::Str(s)) => parse_interval(s).map_err(|e| invalid(e.to_string())),
        Some(v @ Js::Num(n)) => {
            Interval::from_secs_f64(*n).map(Interval::duration).ok_or_else(|| bad(v))
        }
        Some(v) => Err(bad(v)),
    }
}

/// A count from `Number(value)`: `NaN` → `nan`, negative → 0, fractions rounded with `round`.
fn count(n: f64, nan: u32, round: fn(f64) -> f64) -> u32 {
    if n.is_nan() {
        nan
    } else if n <= 0.0 {
        0
    } else {
        let r = round(n);
        if r >= f64::from(u32::MAX) { u32::MAX } else { r as u32 }
    }
}

fn uplink_config(
    name: &str,
    url: String,
    o: &Obj,
    warnings: &mut Vec<String>,
) -> Result<UplinkConfig, ConfigError> {
    for (key, _) in o.iter() {
        if !UPLINK_KEYS.contains(&key) {
            warnings.push(format!(
                "uplinks `{name}`: option `{key}` is not supported by fastopm and is ignored"
            ));
        }
    }
    let set = |key: &str| o.get(key).filter(|v| !matches!(v, Js::Null));

    if let Some(t) = o.get("timeout")
        && t.truthy()
        && t.to_number() >= 1000.0
    {
        warnings.push(format!(
            "uplinks `{name}`: Too big timeout value: {}. We changed time format to nginx-like one \
             (see http://nginx.org/en/docs/syntax.html) so please update your config accordingly",
            t.to_js_string()
        ));
    }
    let maxage = interval(set("maxage"), "2m")?;
    let timeout = interval(set("timeout"), "30s")?;
    // a failing count never reaches NaN: the breaker never opens
    let max_fails = set("max_fails").map_or(2, |v| count(v.to_number(), u32::MAX, f64::ceil));
    let fail_timeout = interval(set("fail_timeout"), "5m")?;
    let retry = match set("retry") {
        None => 2,
        Some(Js::Obj(r)) => r
            .get("limit")
            .filter(|v| !matches!(v, Js::Null))
            .map_or(2, |v| count(v.to_number(), 0, f64::floor)),
        Some(Js::Arr(_)) => 2,
        Some(v) => count(v.to_number(), 0, f64::floor),
    };
    let cache = match o.get("cache") {
        None => true,
        Some(v) => *v == Js::Bool(true),
    };
    // `setConfig(config, 'agent_options', {keepAlive: true, maxSockets: 40, maxFreeSockets: 10})`
    // then `new http.Agent(agent_options)`
    let (max_sockets, max_free_sockets) = match set("agent_options") {
        None => (Some(40), 10),
        Some(Js::Obj(a)) => {
            let positive = |key: &str| {
                a.get(key)
                    .map(Js::to_number)
                    .filter(|n| n.is_finite() && *n >= 1.0)
                    .map(|n| n.min(f64::from(u32::MAX)).floor() as u32)
            };
            let free = positive("maxFreeSockets").map_or(256, |n| n as usize);
            (positive("maxSockets"), free)
        }
        Some(_) => (None, 256),
    };

    let mut headers = IndexMap::new();
    match o.get("headers") {
        Some(Js::Obj(h)) => {
            for (key, value) in h.js_ordered() {
                match value {
                    Js::Null => {}
                    Js::Obj(_) => warnings
                        .push(format!("uplinks `{name}`: header `{key}` is not a string; ignored")),
                    Js::Arr(items) => {
                        let joined =
                            items.iter().map(Js::to_js_string).collect::<Vec<_>>().join(", ");
                        headers.insert(key.to_owned(), joined);
                    }
                    other => {
                        headers.insert(key.to_owned(), other.to_js_string());
                    }
                }
            }
        }
        Some(v) if v.truthy() => {
            warnings.push(format!(
                "uplinks `{name}`: headers must be a mapping, got {}; ignored",
                v.to_json()
            ));
        }
        _ => {}
    }

    let auth = o.get("auth").map(uplink_auth);
    let proxy_key = |key: &str| o.get(key).map(|v| proxy_value(v).unwrap_or_default());

    Ok(UplinkConfig {
        name: name.to_owned(),
        url,
        timeout,
        maxage,
        max_fails,
        fail_timeout,
        retry,
        cache,
        headers,
        auth,
        http_proxy: proxy_key("http_proxy"),
        https_proxy: proxy_key("https_proxy"),
        no_proxy: o.get("no_proxy").map(no_proxy_list),
        max_sockets,
        max_free_sockets,
    })
}

/// `auth` as `setAuthNext` reads it on each request.
fn uplink_auth(v: &Js) -> UplinkAuth {
    let empty = Obj::default();
    let o = match v {
        Js::Obj(o) => o,
        // arrays are objects for lodash `isObject`, with no `token`/`token_env`/`type`
        Js::Arr(_) => &empty,
        _ => return UplinkAuth { auth_type: None, token: TokenSource::Invalid },
    };
    let auth_type = o.get("type").filter(|t| t.truthy()).map(Js::to_js_string);
    let npm_token = || TokenSource::Env(env_keys::NPM_TOKEN.to_owned());
    let token = match (o.get("token"), o.get("token_env")) {
        (Some(Js::Str(t)), _) => TokenSource::Inline(t.clone()),
        (_, None | Some(Js::Null)) => npm_token(),
        (_, Some(Js::Str(var))) => TokenSource::Env(var.clone()),
        (_, Some(Js::Bool(true))) => npm_token(),
        (_, Some(_)) => TokenSource::Missing,
    };
    UplinkAuth { auth_type, token }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn storage_line_rewrite() {
        let yaml = "# c\nstorage: ./storage\nplugins: ./plugins\nstorage: ./storage\n";
        assert_eq!(
            replace_storage_line(yaml, "/data/verdaccio/storage"),
            "# c\nstorage: /data/verdaccio/storage\nplugins: ./plugins\nstorage: ./storage\n"
        );
        assert_eq!(replace_storage_line("storage: ./storage\r\n", "/x"), "storage: /x\r\n");
        assert_eq!(replace_storage_line("storage: ./storagex\n", "/x"), "storage: ./storagex\n");
    }

    #[test]
    fn normalize_is_lexical() {
        assert_eq!(normalize(Path::new("/a/b/../c/./d")), PathBuf::from("/a/c/d"));
        assert_eq!(
            resolve_against(Path::new("/etc/verdaccio"), "./storage"),
            PathBuf::from("/etc/verdaccio/storage")
        );
        assert_eq!(
            resolve_against(Path::new("/etc/verdaccio"), "../data"),
            PathBuf::from("/etc/data")
        );
        assert_eq!(
            resolve_against(Path::new("/etc/verdaccio"), "/srv/x/../y"),
            PathBuf::from("/srv/x/../y")
        );
    }

    #[test]
    fn counts() {
        assert_eq!(count(f64::NAN, u32::MAX, f64::ceil), u32::MAX);
        assert_eq!(count(-1.0, u32::MAX, f64::ceil), 0);
        assert_eq!(count(1.5, u32::MAX, f64::ceil), 2);
        assert_eq!(count(1.5, 0, f64::floor), 1);
        assert_eq!(count(1e20, 0, f64::floor), u32::MAX);
    }
}
