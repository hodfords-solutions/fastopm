//! `Config` loading against Verdaccio's documented behavior.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use fastopm_config::{
    Config, ConfigError, ListenAddress, LogFormat, LogLevel, LogTarget, Scheme, TokenSource,
    UplinkAuth, default_config_yaml, find_config_file,
};

const PATH: &str = "/etc/verdaccio/config.yaml";

fn env_of(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> + use<> {
    let map: HashMap<String, String> =
        pairs.iter().map(|(k, v)| ((*k).to_owned(), (*v).to_owned())).collect();
    move |k| map.get(k).cloned()
}

fn load_env(yaml: &str, pairs: &[(&str, &str)]) -> Result<Config, ConfigError> {
    Config::from_yaml_str(yaml, Path::new(PATH), &env_of(pairs))
}

fn load(yaml: &str) -> Config {
    load_env(yaml, &[]).unwrap_or_else(|e| panic!("config should load: {e}"))
}

fn error(yaml: &str) -> String {
    match load_env(yaml, &[]) {
        Ok(_) => panic!("config should be rejected:\n{yaml}"),
        Err(e) => e.to_string(),
    }
}

fn with_storage(rest: &str) -> String {
    format!("storage: ./storage\n{rest}")
}

#[test]
fn default_config_loads_with_verdaccio_defaults() {
    let c = load(default_config_yaml());
    assert_eq!(c.config_path, PathBuf::from(PATH));
    assert_eq!(c.storage, PathBuf::from("/etc/verdaccio/storage"));
    assert!(c.listen.is_empty());
    assert_eq!(c.listen_address(None).unwrap().to_string(), "http://localhost:4873");

    let npmjs = &c.uplinks["npmjs"];
    assert_eq!(npmjs.name, "npmjs");
    assert_eq!(npmjs.url, "https://registry.npmjs.org");
    assert_eq!(npmjs.maxage, Duration::from_secs(120));
    assert_eq!(npmjs.timeout, Duration::from_secs(30));
    assert_eq!(npmjs.max_fails, 2);
    assert_eq!(npmjs.fail_timeout, Duration::from_secs(300));
    assert_eq!(npmjs.retry, 2);
    assert!(npmjs.cache);
    assert!(npmjs.headers.is_empty());
    assert_eq!(npmjs.auth, None);
    assert_eq!(
        (npmjs.http_proxy.as_deref(), npmjs.https_proxy.as_deref(), npmjs.no_proxy.as_ref()),
        (None, None, None)
    );

    let patterns: Vec<&str> = c.packages.iter().map(|r| r.pattern.as_str()).collect();
    assert_eq!(patterns, ["@*/*", "**"]);
    assert!(c.packages.iter().all(|r| r.proxy == ["npmjs"] && r.storage.is_none()));

    assert_eq!(c.log.target, LogTarget::Stdout);
    assert_eq!(c.log.format, LogFormat::Pretty);
    assert_eq!(c.log.level, LogLevel::Http);
    assert_eq!(c.user_agent, "hidden");
    assert_eq!(c.server_id.len(), 12);
    assert!(c.server_id.bytes().all(|b| b.is_ascii_hexdigit()));
    assert_eq!(c.url_prefix, None);
    assert_eq!(c.public_url, None);
    assert_eq!(c.forwarded_proto_header, "x-forwarded-proto");

    // one warning per ignored top-level key, none for `$all` access rules
    for key in ["web", "auth", "server", "middlewares", "filters", "i18n"] {
        assert!(
            c.warnings.iter().any(|w| w.contains(&format!("`{key}`"))),
            "no warning for {key}: {:?}",
            c.warnings
        );
    }
    assert_eq!(c.warnings.len(), 6, "{:?}", c.warnings);
}

#[test]
fn default_config_yaml_is_verdaccio_file() {
    let original = include_str!("../src/default.yaml");
    assert_eq!(default_config_yaml(), original);
    assert!(default_config_yaml().contains("\nstorage: ./storage\n"));
    assert!(default_config_yaml().contains("url: https://registry.npmjs.org/"));
}

#[test]
fn empty_documents_and_storage() {
    for yaml in ["", "# nothing\n", "~\n"] {
        assert_eq!(error(yaml), "storage field is mandatory");
        let c = load_env(yaml, &[("VERDACCIO_STORAGE_PATH", "/srv/npm")]).unwrap();
        assert_eq!(c.storage, PathBuf::from("/srv/npm"));
        assert!(c.uplinks.is_empty());
        // the implicit `**` rule, local only
        assert_eq!(c.packages.len(), 1);
        assert_eq!(c.packages[0].pattern, "**");
        assert!(c.packages[0].proxy.is_empty());
        assert!(c.uplinks_for("lodash").is_empty());
        assert!(c.warnings.iter().any(|w| w.contains("no `**` rule")));
    }
    // env wins, a relative value is resolved against the config directory
    let c = load_env("storage: /a\n", &[("VERDACCIO_STORAGE_PATH", "../data")]).unwrap();
    assert_eq!(c.storage, PathBuf::from("/etc/data"));
    // empty env value is ignored
    let c = load_env("storage: /a\n", &[("VERDACCIO_STORAGE_PATH", "")]).unwrap();
    assert_eq!(c.storage, PathBuf::from("/a"));
    assert_eq!(error("storage: 12\n"), "storage field is mandatory");
}

#[test]
fn parse_errors() {
    for yaml in ["just a string\n", "- a\n- b\n", "a: [\n", "a: 1\na: 2\n", "x:\n  <<: 3\n"] {
        let e = load_env(yaml, &[]).unwrap_err();
        assert!(matches!(e, ConfigError::Parse { .. }), "{yaml:?} gave {e:?}");
        assert!(e.to_string().starts_with("CONFIG: it does not look like a valid config file"));
    }
}

#[test]
fn startup_assertions() {
    assert_eq!(error("uplinks: [a]\n"), "CONFIG: bad \"uplinks\" value (object expected)");
    assert_eq!(error("packages: foo\n"), "CONFIG: bad \"packages\" value (object expected)");
    assert_eq!(error("uplinks:\n  all:\n    url: http://x\n"), "CONFIG: reserved uplink name: all");
    assert_eq!(
        error("uplinks:\n  'a b':\n    url: http://x\n"),
        "CONFIG: invalid uplink name: a b"
    );
    assert_eq!(error("uplinks:\n  npm: {}\n"), "CONFIG: no url for uplink: npm");
    assert_eq!(error("uplinks:\n  npm:\n    url: ''\n"), "CONFIG: no url for uplink: npm");
    assert_eq!(error("uplinks:\n  npm: null\n"), "CONFIG: no url for uplink: npm");
    assert_eq!(error("uplinks:\n  npm:\n    url: 5\n"), "CONFIG: wrong url format for uplink: npm");
    assert!(
        error("uplinks:\n  npm:\n    url: not a url\n")
            .starts_with("CONFIG: invalid url for uplink: npm")
    );
    assert_eq!(
        error("packages:\n  foo: bar\n"),
        "CONFIG: bad \"'foo'\" package description (object expected)"
    );
    assert_eq!(
        error("packages:\n  '**': null\n"),
        "CONFIG: bad \"'**'\" package description (object expected)"
    );
    assert_eq!(
        error("packages:\n  foo: [a]\n"),
        "CONFIG: bad \"'foo'\" package description (object expected)"
    );
    assert_eq!(
        error("packages:\n  foo:\n    access: {a: 1}\n"),
        "CONFIG: bad package acl (array or string expected): {\"a\":1}"
    );
    assert_eq!(
        error(&with_storage("uplinks:\n  npm:\n    url: http://x\n    maxage: 5x\n")),
        "invalid interval: 5x"
    );
    assert_eq!(
        error(&with_storage("uplinks:\n  npm:\n    url: http://x\n    timeout: true\n")),
        "invalid interval: true"
    );
    // empty acl shapes are fine
    load(&with_storage("packages:\n  foo:\n    access: {}\n    publish: 3\n    proxy: ''\n"));
}

#[test]
fn packages_in_javascript_key_order() {
    let c = load(&with_storage(
        "uplinks:\n  npm:\n    url: http://npm\n\
         packages:\n  'b-*':\n    proxy: npm\n  '10':\n    access: $all\n  '**':\n    proxy: npm\n  '2':\n    access: $all\n",
    ));
    let patterns: Vec<&str> = c.packages.iter().map(|r| r.pattern.as_str()).collect();
    assert_eq!(patterns, ["2", "10", "b-*", "**"]);
}

#[test]
fn uplinks_in_javascript_key_order() {
    let c = load(&with_storage(
        "uplinks:\n  zz:\n    url: http://z\n  '7':\n    url: http://7\n  aa:\n    url: http://a\n",
    ));
    let names: Vec<&str> = c.uplinks.keys().map(String::as_str).collect();
    assert_eq!(names, ["7", "zz", "aa"]);
}

#[test]
fn merge_keys_keep_js_yaml_order() {
    let c = load(&with_storage(
        "common: &common\n  '@company/*':\n    proxy: npm\n  'lodash':\n    proxy: npm\n\
         uplinks:\n  npm:\n    url: http://npm\n\
         packages:\n  'first':\n    proxy: npm\n  <<: *common\n  lodash:\n    proxy: ''\n  '**':\n    proxy: npm\n",
    ));
    let patterns: Vec<&str> = c.packages.iter().map(|r| r.pattern.as_str()).collect();
    assert_eq!(patterns, ["first", "@company/*", "lodash", "**"]);
    assert!(c.package_rule("lodash").unwrap().proxy.is_empty());
}

#[test]
fn package_rule_and_uplinks_for() {
    let c = load(&with_storage(
        "uplinks:\n  a:\n    url: http://a\n  b:\n    url: http://b\n\
         packages:\n  '@private/*':\n    proxy: []\n  '@*/*':\n    proxy: b missing a b\n  'Foo-*':\n    proxy: [a]\n  '**':\n    proxy: a\n",
    ));
    assert_eq!(c.package_rule("@private/x").unwrap().pattern, "@private/*");
    assert!(c.uplinks_for("@private/x").is_empty());
    let names = |pkg: &str| c.uplinks_for(pkg).iter().map(|u| u.name.clone()).collect::<Vec<_>>();
    assert_eq!(names("@scope/x"), ["b", "a"]);
    assert_eq!(c.package_rule("foo-bar").unwrap().pattern, "Foo-*");
    assert_eq!(names("FOO-BAR"), ["a"]);
    assert_eq!(names("lodash"), ["a"]);
    assert_eq!(c.package_rule("@Private/X").unwrap().pattern, "@private/*");
    assert_eq!(c.packages[1].proxy, ["b", "missing", "a", "b"]);
}

#[test]
fn access_rules_warn_unless_anonymous_can_read() {
    let c = load(&with_storage(
        "packages:\n  a:\n    access: $authenticated\n  b:\n    access: $all\n  c: {}\n  d:\n    access: [admin, $anonymous]\n  '**':\n    access: '@all'\n",
    ));
    let warned = |p: &str| c.warnings.iter().any(|w| w.starts_with(&format!("packages `{p}`")));
    assert!(warned("a"));
    assert!(!warned("b"));
    assert!(warned("c"));
    assert!(!warned("d"));
    assert!(!warned("**"));
}

#[test]
fn package_storage() {
    let c = load(&with_storage(
        "packages:\n  '@corp/*':\n    storage: corp\n  '**':\n    storage: 7\n",
    ));
    assert_eq!(c.packages[0].storage.as_deref(), Some("corp"));
    assert_eq!(c.packages[1].storage, None);
    assert!(c.warnings.iter().any(|w| w.contains("storage must be a string")));
}

#[test]
fn uplink_options() {
    let c = load(&with_storage(
        "uplinks:\n  a:\n    url: http://a/\n    maxage: 10\n    timeout: 1m 30s\n    max_fails: 0\n    fail_timeout: 0.5\n    retry: 0\n    cache: false\n\
         \x20 b:\n    url: http://b//\n    maxage: null\n    max_fails: '3'\n    retry: {limit: 5}\n    cache: 'yes'\n\
         \x20 c:\n    url: http://c\n    max_fails: nope\n    retry: {}\n    cache: null\n    strict_ssl: false\n",
    ));
    let a = &c.uplinks["a"];
    assert_eq!(a.url, "http://a");
    assert_eq!(a.maxage, Duration::from_secs(10));
    assert_eq!(a.timeout, Duration::from_secs(90));
    assert_eq!(a.max_fails, 0);
    assert_eq!(a.fail_timeout, Duration::from_millis(500));
    assert_eq!(a.retry, 0);
    assert!(!a.cache);
    let b = &c.uplinks["b"];
    assert_eq!(b.url, "http://b/", "only one trailing slash is removed");
    assert_eq!(b.maxage, Duration::from_secs(120));
    assert_eq!(b.max_fails, 3);
    assert_eq!(b.retry, 5);
    assert!(!b.cache, "cache must be exactly true");
    let cc = &c.uplinks["c"];
    assert_eq!(cc.max_fails, u32::MAX, "NaN never opens the breaker");
    assert_eq!(cc.retry, 2);
    assert!(!cc.cache);
    assert!(c.warnings.iter().any(|w| w.contains("`strict_ssl`")));
}

#[test]
fn big_timeout_warns() {
    let c = load(&with_storage("uplinks:\n  a:\n    url: http://a\n    timeout: 30000\n"));
    assert_eq!(c.uplinks["a"].timeout, Duration::from_secs(30000));
    assert!(c.warnings.iter().any(|w| w.contains("Too big timeout value: 30000")));
}

#[test]
fn uplink_headers() {
    let c = load(&with_storage(
        "uplinks:\n  a:\n    url: http://a\n    headers:\n      Accept: application/json\n      x-num: 5\n      x-null: null\n      '1': one\n",
    ));
    let h: Vec<(&str, &str)> =
        c.uplinks["a"].headers.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
    assert_eq!(h, [("1", "one"), ("Accept", "application/json"), ("x-num", "5")]);
}

#[test]
fn uplink_auth_sources() {
    let auth = |yaml: &str| -> Option<UplinkAuth> {
        load(&with_storage(&format!("uplinks:\n  a:\n    url: http://a\n{yaml}"))).uplinks["a"]
            .auth
            .clone()
    };
    let inline = |t: &str| TokenSource::Inline(t.to_owned());
    let env = |v: &str| TokenSource::Env(v.to_owned());
    assert_eq!(auth(""), None);
    assert_eq!(
        auth("    auth:\n      type: bearer\n      token: abc\n"),
        Some(UplinkAuth { auth_type: Some("bearer".into()), token: inline("abc") })
    );
    assert_eq!(auth("    auth:\n      token: ''\n").unwrap().token, inline(""));
    assert_eq!(auth("    auth:\n      token_env: MY_TOKEN\n").unwrap().token, env("MY_TOKEN"));
    assert_eq!(auth("    auth:\n      token_env: true\n").unwrap().token, env("NPM_TOKEN"));
    assert_eq!(auth("    auth:\n      type: basic\n").unwrap().token, env("NPM_TOKEN"));
    assert_eq!(auth("    auth:\n      token: 5\n      token_env: X\n").unwrap().token, env("X"));
    assert_eq!(auth("    auth:\n      token_env: false\n").unwrap().token, TokenSource::Missing);
    assert_eq!(auth("    auth:\n      token_env: 3\n").unwrap().token, TokenSource::Missing);
    assert_eq!(
        auth("    auth: abc\n"),
        Some(UplinkAuth { auth_type: None, token: TokenSource::Invalid })
    );
    assert_eq!(auth("    auth: null\n").unwrap().token, TokenSource::Invalid);
    assert_eq!(auth("    auth: []\n").unwrap().token, env("NPM_TOKEN"));
    assert_eq!(auth("    auth:\n      type: ''\n      token: t\n").unwrap().auth_type, None);
    assert_eq!(
        auth("    auth:\n      type: Custom\n      token: t\n").unwrap().auth_type.as_deref(),
        Some("Custom")
    );
}

#[test]
fn scalar_settings() {
    let c = load(&with_storage("user_agent: 12\nserver_id: 42\nurl_prefix: /npm/\n"));
    assert_eq!(c.user_agent, "12");
    assert_eq!(c.server_id, "42");
    assert_eq!(c.url_prefix.as_deref(), Some("/npm/"));
    let c = load(&with_storage("user_agent: null\nserver_id: ''\n"));
    assert_eq!(c.user_agent, "hidden");
    assert_eq!(c.server_id.len(), 12);
    let c = load(&with_storage("user_agent: ''\nserver_id: false\n"));
    assert_eq!(c.user_agent, "");
    assert_ne!(c.server_id, "false");
}

#[test]
fn env_overrides() {
    let c = load_env(
        &with_storage(""),
        &[
            ("VERDACCIO_PUBLIC_URL", "https://npm.example.com/base/"),
            ("VERDACCIO_FORWARDED_PROTO", "X-Proto"),
        ],
    )
    .unwrap();
    assert_eq!(c.public_url.as_deref(), Some("https://npm.example.com/base/"));
    assert_eq!(c.forwarded_proto_header, "x-proto");
    for bad in ["npm.example.com", "ftp://npm.example.com", ""] {
        let c = load_env(&with_storage(""), &[("VERDACCIO_PUBLIC_URL", bad)]).unwrap();
        assert_eq!(c.public_url, None, "{bad}");
    }
}

#[test]
fn proxy_settings_from_config_and_env() {
    let env = [
        ("http_proxy", ""),
        ("HTTP_PROXY", "http://upper:1"),
        ("https_proxy", "http://lower:2"),
        ("NO_PROXY", "a.com,b.com"),
    ];
    let c = load_env(&with_storage(""), &env).unwrap();
    assert_eq!(c.http_proxy.as_deref(), Some("http://upper:1"));
    assert_eq!(c.https_proxy.as_deref(), Some("http://lower:2"));
    assert_eq!(c.no_proxy, ["a.com", "b.com"]);
    // a key in the file wins over the environment, even when null
    let c = load_env(
        &with_storage("http_proxy: null\nhttps_proxy: http://file:3\nno_proxy: [x.org]\n"),
        &env,
    )
    .unwrap();
    assert_eq!(c.http_proxy, None);
    assert_eq!(c.https_proxy.as_deref(), Some("http://file:3"));
    assert_eq!(c.no_proxy, ["x.org"]);
}

#[test]
fn effective_proxy_rules() {
    let c = load(&with_storage(
        "http_proxy: http://hp:1\nhttps_proxy: http://hsp:2\nno_proxy: internal.corp,.local\n\
         uplinks:\n\
         \x20 plain:\n    url: http://registry.example.com\n\
         \x20 secure:\n    url: https://registry.npmjs.org/\n\
         \x20 own:\n    url: https://registry.npmjs.org/\n    https_proxy: http://own:3\n\
         \x20 off:\n    url: https://registry.npmjs.org/\n    https_proxy: null\n\
         \x20 inside:\n    url: https://npm.internal.corp\n\
         \x20 exact:\n    url: http://internal.corp:8080\n\
         \x20 suffix:\n    url: http://notinternal.corp\n\
         \x20 dotted:\n    url: http://host.local\n\
         \x20 ownlist:\n    url: https://npm.internal.corp\n    no_proxy: other.org\n\
         \x20 emptylist:\n    url: https://npm.internal.corp\n    no_proxy: ''\n",
    ));
    let proxy = |name: &str| c.effective_proxy(&c.uplinks[name]).map(str::to_owned);
    assert_eq!(proxy("plain").as_deref(), Some("http://hp:1"));
    assert_eq!(proxy("secure").as_deref(), Some("http://hsp:2"));
    assert_eq!(proxy("own").as_deref(), Some("http://own:3"));
    assert_eq!(proxy("off"), None);
    assert_eq!(proxy("inside"), None);
    assert_eq!(proxy("exact"), None);
    assert_eq!(proxy("suffix").as_deref(), Some("http://hp:1"), "wget style needs a dot boundary");
    assert_eq!(proxy("dotted"), None);
    assert_eq!(
        proxy("ownlist").as_deref(),
        Some("http://hsp:2"),
        "uplink no_proxy replaces the global list"
    );
    assert_eq!(proxy("emptylist").as_deref(), Some("http://hsp:2"));
    assert_eq!(c.uplinks["emptylist"].no_proxy.as_deref(), Some(&[][..]));
}

#[test]
fn listen_addresses() {
    let tcp = |scheme, host: &str, port| ListenAddress::Tcp { scheme, host: host.to_owned(), port };
    let c = load(&with_storage("listen: 0.0.0.0:4000\n"));
    assert_eq!(c.listen, ["0.0.0.0:4000"]);
    assert_eq!(c.listen_address(None).unwrap(), tcp(Scheme::Http, "0.0.0.0", 4000));
    assert_eq!(
        c.listen_address(Some("https://[::1]:443")).unwrap(),
        tcp(Scheme::Https, "::1", 443)
    );
    assert_eq!(c.listen_address(Some("")).unwrap(), tcp(Scheme::Http, "0.0.0.0", 4000));
    assert!(
        c.listen_address(Some("nope"))
            .unwrap_err()
            .to_string()
            .starts_with("Invalid address - nope")
    );

    let c = load(&with_storage("listen: 5000\n"));
    assert_eq!(c.listen_address(None).unwrap(), tcp(Scheme::Http, "localhost", 5000));

    let c = load(&with_storage(
        "listen:\n  - bad address\n  - 7\n  - unix:/tmp/f.sock\n  - localhost:9\n",
    ));
    assert_eq!(c.listen, ["bad address", "unix:/tmp/f.sock", "localhost:9"]);
    assert_eq!(
        c.listen_address(None).unwrap(),
        ListenAddress::Unix { path: PathBuf::from("/tmp/f.sock") }
    );
    assert!(c.warnings.iter().any(|w| w.starts_with("invalid address - bad address")));
    assert!(c.warnings.iter().any(|w| w.contains("using the first valid one unix:/tmp/f.sock")));

    let c = load(&with_storage("listen: [a, b]\n"));
    assert_eq!(
        c.listen_address(None).unwrap_err().to_string(),
        "No valid listen addresses found in configuration array"
    );
    let c = load(&with_storage("listen: 'x:y'\n"));
    assert!(c.listen_address(None).is_err());
    let c = load(&with_storage("listen: ''\n"));
    assert_eq!(c.listen_address(None).unwrap().to_string(), "http://localhost:4873");
}

#[test]
fn log_settings() {
    let c = load(&with_storage(
        "log:\n  type: file\n  path: logs/fastopm.log\n  format: json\n  level: warn\n",
    ));
    assert_eq!(c.log.target, LogTarget::File(PathBuf::from("/etc/verdaccio/logs/fastopm.log")));
    assert_eq!(c.log.format, LogFormat::Json);
    assert_eq!(c.log.level, LogLevel::Warn);
    let c = load(&with_storage("log:\n  format: pretty-timestamped\n  level: loud\n"));
    assert_eq!(c.log.format, LogFormat::PrettyTimestamped);
    assert_eq!(c.log.level, LogLevel::Http);
    assert!(c.warnings.iter().any(|w| w.contains("unknown level")));
    assert!(LogLevel::Warn < LogLevel::Http && LogLevel::Http < LogLevel::Info);
}

#[test]
fn load_reads_files() {
    let dir = tempfile::tempdir().unwrap();
    let yml = dir.path().join("conf.YML");
    std::fs::write(&yml, "storage: ./data\n").unwrap();
    let c = Config::load(Some(&yml)).unwrap();
    assert_eq!(c.storage, dir.path().join("data"));
    assert_eq!(c.config_path, yml);

    let txt = dir.path().join("conf.txt");
    std::fs::write(&txt, "storage: ./data\n").unwrap();
    assert!(matches!(Config::load(Some(&txt)), Err(ConfigError::NotYaml(_))));
    assert!(matches!(
        Config::load(Some(&dir.path().join("missing.yaml"))),
        Err(ConfigError::Io { .. })
    ));
}

#[test]
fn find_config_file_uses_cli_path() {
    let found = find_config_file(Some(Path::new("some/dir/../config.yaml")), &env_of(&[])).unwrap();
    assert!(found.is_absolute());
    assert!(found.ends_with("some/config.yaml"));
}

#[test]
fn find_config_file_writes_default_to_xdg() {
    let home = tempfile::tempdir().unwrap();
    let config_home = home.path().join("cfg");
    let data_home = home.path().join("data");
    std::fs::create_dir_all(&config_home).unwrap();
    std::fs::create_dir_all(&data_home).unwrap();
    let env = env_of(&[
        ("XDG_CONFIG_HOME", config_home.to_str().unwrap()),
        ("XDG_DATA_HOME", data_home.to_str().unwrap()),
    ]);
    let path = find_config_file(None, &env).unwrap();
    assert_eq!(path, config_home.join("verdaccio/config.yaml"));
    let written = std::fs::read_to_string(&path).unwrap();
    let storage = data_home.join("verdaccio/storage");
    assert!(written.contains(&format!("\nstorage: {}\n", storage.display())), "{written}");
    assert_eq!(written.replace(&storage.display().to_string(), "./storage"), default_config_yaml());

    // found on the next call, not rewritten
    std::fs::write(&path, "storage: /custom\n").unwrap();
    assert_eq!(find_config_file(None, &env).unwrap(), path);
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "storage: /custom\n");
}

#[test]
fn find_config_file_skips_storage_rewrite_without_data_dir() {
    let home = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(home.path().join(".config")).unwrap();
    let env = env_of(&[("HOME", home.path().to_str().unwrap())]);
    let path = find_config_file(None, &env).unwrap();
    assert_eq!(path, home.path().join(".config/verdaccio/config.yaml"));
    assert_eq!(std::fs::read_to_string(&path).unwrap(), default_config_yaml());
}
