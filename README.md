# fastopm

A read-only npm proxy cache written in Rust that reads Verdaccio's `config.yaml` and storage layout.

[![CI](https://github.com/hodfords-solutions/fastopm/actions/workflows/ci.yml/badge.svg)](https://github.com/hodfords-solutions/fastopm/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

fastopm sits between your package managers (npm, pnpm, yarn, bun) and an upstream registry such as
`registry.npmjs.org`. It caches package metadata and tarballs on disk, so repeated installs are
served locally and installs keep working when the upstream registry is down. It is meant to replace
[Verdaccio](https://verdaccio.org) **when Verdaccio is used only as a proxy cache**: point it at your
existing Verdaccio `config.yaml` and storage folder and it serves the same packages.

It does **not** publish packages, authenticate clients, serve a web UI or load plugins. If you need
any of those, keep using Verdaccio.

> **Status: 0.1.0, early.** The behaviour is checked against Verdaccio 7.1.0 by a differential
> test harness and by end-to-end installs with npm, pnpm and yarn classic (see
> [Compatibility](#compatibility-and-how-it-is-verified)). It has not yet seen wide production use.
> Expect rough edges and report them.
>
> fastopm is an independent project. It is not part of, affiliated with, or endorsed by the
> Verdaccio project.

## Contents

- [Why fastopm](#why-fastopm)
- [What it does and does not do](#what-it-does-and-does-not-do)
- [Quick start](#quick-start)
- [Pointing package managers at fastopm](#pointing-package-managers-at-fastopm)
- [Configuration](#configuration)
- [Migrating from Verdaccio and rolling back](#migrating-from-verdaccio-and-rolling-back)
- [Compatibility and how it is verified](#compatibility-and-how-it-is-verified)
- [Performance](#performance)
- [Architecture](#architecture)
- [Development](#development)
- [Contributing](#contributing)
- [Security](#security)
- [License](#license)
- [Acknowledgements](#acknowledgements)

## Why fastopm

Many teams run Verdaccio only to cache the public registry for CI runners and developer machines.
In that role most of Verdaccio (publishing, users, the web UI, the plugin system) is unused, and the
part that is used, the read path, is where the load is. fastopm implements only that read path,
as a single native binary, and keeps everything a client or an operator can observe compatible
with Verdaccio:

- **Same configuration.** It reads Verdaccio's `config.yaml`. Keys it does not use are ignored with
  a warning at startup, so a Verdaccio config loads unchanged.
- **Same storage.** It reads and writes Verdaccio's on-disk layout. You can switch an existing cache
  folder from Verdaccio to fastopm and back.
- **Same HTTP responses** on the read routes: status codes, headers, JSON bodies (byte for byte,
  including key order and `ETag`), error messages, and `dist.tarball` URLs rewritten to the proxy.
- **Same Docker contract.** Port 4873, `/verdaccio/conf/config.yaml`, the `/verdaccio/storage`
  volume, UID 10001, and the `VERDACCIO_PORT` / `VERDACCIO_PROTOCOL` / `VERDACCIO_ADDRESS`
  variables, so it can replace the `verdaccio/verdaccio` image in an existing deployment.

How it differs from Verdaccio internally (design goals, see [Performance](#performance)): parsed
manifests and rendered responses are cached in memory, matching `If-None-Match` requests are
answered without rebuilding the body, concurrent requests for the same uncached package share one
upstream request, and the server uses all CPU cores.

## What it does and does not do

| Supported | Not supported |
|---|---|
| `GET /:package`: full metadata, or the abbreviated form when the client sends `Accept: application/vnd.npm.install-v1+json` | Publishing, unpublishing, deprecating, dist-tag changes: these routes return `405 {"error": "unsupported registry call"}` |
| `GET /:package/:version` (exact version or dist-tag) | Client authentication: `npm login`, users, tokens. The `Authorization` header from clients is ignored |
| `GET /:package/-/:filename` (tarballs, streamed while being cached) | `access` / `publish` / `unpublish` rules under `packages:` (not enforced; a warning is logged) |
| `GET /-/ping` | Web UI (`GET /` returns `404 {"error": "Web interface is disabled in the config file"}`) |
| `HEAD` on the routes above, CORS preflight (`OPTIONS`) | Plugins (auth, storage, middleware, filters) |
| Scoped packages as `/@scope/pkg`, `/@scope%2Fpkg` and `/%40scope/pkg` | Search, `npm audit` (the audit endpoint returns 405), stars, profile, 2FA |
| Several uplinks per package, tried in order; `maxage`, timeouts, retries, circuit breaker | TLS termination: an `https://` listen address is refused; put a TLS reverse proxy in front |
| Serving the cache when the upstream is unreachable | Multiple listen addresses (the first valid one is used, as in Verdaccio) |
| Packages previously published to a Verdaccio storage (served read-only) | Cloud storage backends: only the local filesystem layout |
| Private upstream registries (`auth`, `headers` per uplink), HTTP(S) proxies | |
| Loop detection with the `Via` header (`508` when a request comes back to the same `server_id`) | |

## Quick start

### Docker

No image has been published to Docker Hub yet. Build it from this repository (BuildKit, the
default builder in current Docker, is required):

```bash
git clone https://github.com/hodfords-solutions/fastopm.git
cd fastopm
docker build -t fastopm:local .
docker run -d --name fastopm -p 4873:4873 -v fastopm-storage:/verdaccio/storage fastopm:local
curl http://localhost:4873/-/ping    # {}
```

The image ships the config in [`docker/config.yaml`](docker/config.yaml): every package is proxied
to `https://registry.npmjs.org/` and cached under `/verdaccio/storage/data`. To use your own config,
mount it over the default:

```bash
docker run -d --name fastopm -p 4873:4873 \
  -v "$PWD/config.yaml:/verdaccio/conf/config.yaml:ro" \
  -v fastopm-storage:/verdaccio/storage \
  fastopm:local
```

The container runs as UID 10001, group 0, like the Verdaccio image. A bind-mounted storage folder
must be writable by that user. The image has a `HEALTHCHECK` on `GET /-/ping`.

When images are published, they will be at `hodfords/fastopm` on Docker Hub: `edge` from the `main`
branch, and `X.Y.Z`, `X.Y`, `X` and `latest` from release tags (see
[`.github/workflows/docker.yml`](.github/workflows/docker.yml)).

### From source

Requires a recent stable Rust toolchain (CI builds with Rust 1.100).

```bash
git clone https://github.com/hodfords-solutions/fastopm.git
cd fastopm
cargo build --release --locked -p fastopm
```

Create a minimal `config.yaml`:

```yaml
storage: ./storage
uplinks:
  npmjs:
    url: https://registry.npmjs.org/
packages:
  '@*/*':
    access: $all
    proxy: npmjs
  '**':
    access: $all
    proxy: npmjs
```

(`access: $all` only states what fastopm does anyway; without it, fastopm logs a warning that the
rule is not enforced.) Start the server:

```bash
./target/release/fastopm --config ./config.yaml --listen 127.0.0.1:4873
```

Command line:

```text
fastopm [--config <PATH>] [--listen <ADDR>]
fastopm --version

-c, --config <PATH>  Path to config.yaml (default: Verdaccio's search order)
-l, --listen <ADDR>  Listen address, overrides `listen` in the config: `4873`, `host:port`,
                     `http://host:port/`, `unix:/path.sock`
-v, --version        Print the version and exit
-h, --help           Print help
```

Without `--config`, fastopm looks for the config file the way Verdaccio does, and, like Verdaccio,
**writes Verdaccio's default config** to the first location if none exists:

1. `$XDG_CONFIG_HOME/verdaccio/config.yaml` (or `~/.config/verdaccio/config.yaml`), when that base
   directory exists
2. on Windows, `%APPDATA%/verdaccio/config.yaml`
3. `./verdaccio/config.yaml`
4. `./config.yaml`

Without a `listen` key or `--listen`, the server listens on `http://localhost:4873`.

## Pointing package managers at fastopm

The examples use `http://localhost:4873/`; replace it with the address clients reach fastopm on.

| Client | Tested end to end |
|---|---|
| npm | yes (npm 11) |
| pnpm | yes (pnpm 12) |
| yarn classic (1.x) | yes (1.22) |
| yarn berry (2+) | not yet |
| bun | not yet |

**npm** (per project in `.npmrc`, or for the user with `npm config set registry <url>`):

```ini
registry=http://localhost:4873/
```

`npm install` also calls the audit endpoint, which fastopm does not implement (`405`). Disable the
audit step with `audit=false` in `.npmrc` or `--no-audit`.

**pnpm** reads the same `.npmrc`:

```ini
registry=http://localhost:4873/
```

**yarn classic (1.x)**:

```bash
yarn config set registry http://localhost:4873/
```

`yarn.lock` stores full `resolved` URLs. A lockfile generated against another registry keeps
downloading from that registry until the URLs are rewritten.

**yarn berry (2+)**, in `.yarnrc.yml` (yarn berry refuses plain-HTTP registries that are not
whitelisted):

```yaml
npmRegistryServer: "http://localhost:4873"
unsafeHttpWhitelist:
  - localhost
```

**bun**, in `bunfig.toml`:

```toml
[install]
registry = "http://localhost:4873/"
```

## Configuration

fastopm reads Verdaccio's `config.yaml`. Only the keys below are used. Relative paths are resolved
against the directory of the config file. Durations use Verdaccio's format (`30s`, `2m`, `1h`, or a
number of seconds).

### Top-level keys

| Key | Default | Meaning |
|---|---|---|
| `storage` | required | Storage root. Overridden by `VERDACCIO_STORAGE_PATH` |
| `listen` | `http://localhost:4873` | `4873`, `host:port`, `http://host:port/` or `unix:/path.sock`. A list is accepted; the first valid entry is used. `https://` is refused. `--listen` wins |
| `uplinks` | none | Upstream registries, see below |
| `packages` | none | Rules matched against the package name (minimatch patterns such as `@*/*`, `**`, case-insensitive, first match wins), see below |
| `url_prefix` | none | Path prefix used when building `dist.tarball` URLs (for a proxy mounted under a sub-path). Routes are still served at the root, as in Verdaccio |
| `log.type` | `stdout` | `stdout`, or `file` together with `log.path` |
| `log.format` | `pretty` | `pretty`, `pretty-timestamped` or `json` |
| `log.level` | `http` | `fatal`, `error`, `warn`, `http` (adds the access log), `info`, `debug`, `trace`. `RUST_LOG`, when set, replaces this mapping |
| `http_proxy`, `https_proxy` | env `http_proxy`/`HTTP_PROXY`, `https_proxy`/`HTTPS_PROXY` | Outgoing proxy for uplink requests |
| `no_proxy` | env `no_proxy`/`NO_PROXY` | Comma-separated string or list of hosts that bypass the proxy |
| `user_agent` | `hidden` | Sent upstream as `User-Agent: npm (<user_agent>)`, and to clients as `X-Powered-By` |
| `server_id` | 12 random hex characters | Identifier used in the `Via` header for loop detection. Set it when several instances share an upstream chain |

### `uplinks.<name>`

Same options and defaults as Verdaccio:

| Option | Default | Meaning |
|---|---|---|
| `url` | required | Registry URL |
| `maxage` | `2m` | How long cached metadata is used without asking this uplink |
| `timeout` | `30s` | Per-request timeout |
| `max_fails` / `fail_timeout` | `2` / `5m` | Circuit breaker: after `max_fails` failed attempts the uplink is treated as offline for `fail_timeout`. As in Verdaccio, with the default `retry` one failed metadata request is enough |
| `retry` | `2` | Retries for metadata requests on network errors and 5xx (a number, or `{limit: n}`) |
| `cache` | `true` | `false`: tarballs are streamed but not written to disk (metadata is still stored) |
| `headers` | none | Extra request headers |
| `auth` | none | `type` (`basic` or `bearer`, default basic), and `token` (inline) or `token_env` (variable name, or `true` for `NPM_TOKEN`). Without either, `NPM_TOKEN` is read |
| `http_proxy`, `https_proxy`, `no_proxy` | global values | Per-uplink proxy settings |

`agent_options` is accepted and ignored silently. Any other uplink option (for example `strict_ssl`
or `ca`) is ignored with a warning.

### `packages.<pattern>`

| Option | Meaning |
|---|---|
| `proxy` | Uplink name(s) to try, in order (space-separated string or list). Without `proxy`, matching packages are served from storage only |
| `storage` | Sub-folder of the storage root for matching packages |

`access`, `publish` and `unpublish` are accepted but not enforced: every client that can reach
fastopm can read every package. When there is no `**` rule, fastopm adds one without `proxy` (serve
from storage only) and logs a warning; Verdaccio would refuse those packages instead.

### Ignored keys

Every other top-level key (`auth`, `web`, `security`, `middlewares`, `plugins`, `filters`,
`server`, `i18n`, `notify`, `max_body_size`, ...) is ignored and reported once at startup:

```text
config key `auth` is not supported by fastopm and is ignored
```

### Environment variables

| Variable | Meaning |
|---|---|
| `VERDACCIO_PUBLIC_URL` | Public base URL used to build `dist.tarball` URLs (absolute `http`/`https` URL) |
| `VERDACCIO_FORWARDED_PROTO` | Header that carries the original protocol behind a reverse proxy (default `X-Forwarded-Proto`) |
| `VERDACCIO_STORAGE_PATH` | Overrides `storage` |
| `NPM_TOKEN` | Default uplink token source when `auth` has no `token` / `token_env` |
| `http_proxy`, `https_proxy`, `no_proxy` (and upper-case forms) | Used when the config file has no such key |
| `RUST_LOG` | Log filter (`tracing` `EnvFilter` syntax); replaces the `log.level` mapping |
| `XDG_CONFIG_HOME`, `XDG_DATA_HOME`, `HOME`, `APPDATA` | Config file search, see [From source](#from-source) |

The Docker image also reads `VERDACCIO_PORT` (default `4873`), `VERDACCIO_PROTOCOL` (default `http`)
and `VERDACCIO_ADDRESS` (default `[::]`) to build the `--listen` argument.

The in-memory manifest cache is limited to 256 MiB. It is not configurable yet.

## Migrating from Verdaccio and rolling back

fastopm uses the same files as Verdaccio: `<storage>/<name>/package.json` (compact JSON with
`_uplinks`, `_distfiles` and `_rev`) and `<storage>/<name>/<file>.tgz`, with scoped packages under
`<storage>/@scope/<name>/`. Writes go to a temporary file followed by a rename.

**Before you switch**, check that nothing depends on what fastopm does not do: publishing (including
CI jobs that publish internal packages), `npm login` or tokens, `access` rules that hide packages,
the web UI, search, `npm audit`, plugins. Packages already published to the Verdaccio storage stay
readable through fastopm, but cannot be updated.

**Switching (binary):**

1. Stop Verdaccio.
2. Start fastopm with the same config and the same storage:
   `fastopm --config /path/to/verdaccio/config.yaml`.
3. Read the startup warnings: they list every config key and `access` rule that fastopm ignores.

**Switching (Docker):** replace the `verdaccio/verdaccio` image with the fastopm image and keep the
same volume for `/verdaccio/storage` and the same config mount at `/verdaccio/conf/config.yaml`.
Port, UID and environment variables are the same.

**Rolling back:** stop fastopm and start Verdaccio again on the same config and storage. Metadata
and tarballs that fastopm cached are in Verdaccio's format and are used as they are. Verdaccio
creates its `.verdaccio-db.json` file at startup if it is missing.

Switching in both directions on one storage folder is covered by the end-to-end tests (`cross`
rounds). Running Verdaccio and fastopm **at the same time** on one storage folder has not been
tested; stop one before starting the other.

## Compatibility and how it is verified

The reference is `verdaccio@7.1.0`, run unmodified.

**Differential harness** ([`compat/`](compat/README.md), README in Vietnamese). The same requests
are sent to Verdaccio and to fastopm, each with its own mock upstream registry and an empty or
pre-seeded storage. For every step it compares status, headers and body byte for byte, the requests
each mock upstream received (method, path, headers, retries), and the storage tree at the end. It
runs offline. Latest result: 62 scenarios, 57 identical, 5 allowed differences, 0 unexpected
differences. Every allowed difference is listed with its reason in `compat/lib/rules.mjs`:

| Allowed difference | Verdaccio | fastopm |
|---|---|---|
| Write routes (publish, unpublish, dist-tags, user, token, audit), 3 scenarios | its own auth and publish errors (for example `401`) | `405 {"error": "unsupported registry call"}` |
| Invalid client `Authorization` header | `401` | ignored, request served; nothing from the header is sent upstream |
| Path segment starting with `.` (`GET /.hidden`) | never answers | `404` |

The harness also runs in CI after the release build. That job does not fail the workflow yet
(controlled by the `COMPAT_REQUIRED` repository variable).

**End-to-end installs** ([`compat/e2e/`](compat/e2e/README.md)). Real package managers install a
project of about 300 packages through fastopm and through Verdaccio, with `registry.npmjs.org` as
the upstream: cold cache, warm cache (no tarball fetched upstream), a lockfile generated against
npmjs (integrity values must match), upstream unreachable with and without a lockfile, and each
server reading the storage written by the other. Last full run: 39 rounds, all passed, with
npm 11.19, pnpm 12.1 and yarn 1.22.22. **bun and yarn berry have not been tested**; the script has
code paths for both. This suite needs Internet access and does not run in CI.

Other differences you may notice:

- `npm install` without `--no-audit` gets `405` from the audit endpoint.
- Without a `**` rule in `packages:`, unmatched packages are served from storage instead of being
  refused.
- Verdaccio forwards `npm audit` to `registry.npmjs.org` when its audit middleware is enabled
  (the default); fastopm does not.

## Performance

Benchmark results have not been published yet. This section describes what the design aims for and
how to measure it yourself.

Where Verdaccio spends time on the read path, and what fastopm does instead:

| Verdaccio | fastopm |
|---|---|
| Reads and parses `package.json` from disk on every metadata request | Keeps parsed manifests in a size-bounded in-memory cache (256 MiB), revalidated against the file's modification time and size so changes by another process are picked up |
| Serializes the response body and computes its MD5 `ETag` on every request | Caches rendered bodies with their `ETag` per package, response form and base URL |
| Builds the full body before answering a matching `If-None-Match` | Answers `304` from the cached `ETag` |
| Sends one upstream request per concurrent client for an uncached package | Coalesces concurrent requests for the same metadata or tarball into one upstream request |
| Runs on a single Node.js event loop | Runs on a multi-threaded Tokio runtime |

The benchmark suite in [`bench/`](bench/README.md) (README in Vietnamese) runs fastopm and
Verdaccio 7.1.0 against the same mock upstream and storage. It covers abbreviated and full metadata
with a warm cache, `304` responses, local tarballs, a cold cache, expired `maxage`, and resource use. It needs a release build, Node.js and the `oha` load generator; the README
lists the setup. A full run takes about an hour:

```bash
node bench/run.mjs --quick    # about 4 minutes
node bench/run.mjs            # full run
```

Numbers depend heavily on the machine, the packages and the cache state. Measure with your own
workload before drawing conclusions.

## Architecture

One Cargo workspace, one binary. Each crate has a single responsibility:

| Crate | Responsibility |
|---|---|
| [`fastopm-core`](crates/fastopm-core) | Shared types: errors and Verdaccio's error messages, manifest, package-name validation, durations, JSON output identical to `JSON.stringify` |
| [`fastopm-config`](crates/fastopm-config) | `config.yaml` parsing, defaults, `packages:` pattern matching, listen addresses, config file search |
| [`fastopm-storage`](crates/fastopm-storage) | Verdaccio's on-disk layout: `package.json` and tarballs, atomic writes, per-package locks |
| [`fastopm-uplink`](crates/fastopm-uplink) | HTTP client for upstream registries: headers, auth, proxies, retries, circuit breaker, `ETag` |
| [`fastopm-render`](crates/fastopm-render) | Response bodies: full and abbreviated metadata, `dist.tarball` rewriting, public URL, `ETag` |
| [`fastopm-store`](crates/fastopm-store) | Orchestration: memory cache, disk, uplinks, metadata merge, write-back, request coalescing |
| [`fastopm-server`](crates/fastopm-server) | HTTP routes (axum), scoped-name handling, content negotiation, compression, errors, access log |
| [`fastopm`](crates/fastopm) | The binary: command line, logging, startup, graceful shutdown on SIGINT/SIGTERM |

Outside the workspace: [`compat/`](compat/README.md) (Node.js compatibility harness and end-to-end
script), [`bench/`](bench/README.md) (benchmarks), [`docker/`](docker) and the
[`Dockerfile`](Dockerfile) (multi-arch image, `linux/amd64` and `linux/arm64`).

The source comments cite the Verdaccio source files each behaviour comes from.

## Development

Requirements: Rust stable (CI uses 1.100), and Node.js 20 or later for the harness and the
benchmarks (CI uses Node.js 24).

The checks CI runs:

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
cargo build --release --locked -p fastopm
```

Compatibility harness, against the release build:

```bash
cd compat
npm ci
node run.mjs --candidate-bin ../target/release/fastopm
node run.mjs --candidate-bin ../target/release/fastopm --filter tarball/ --verbose
node run.mjs    # Verdaccio against itself: checks the harness, must report 0 differences
```

Exit code `0` means no difference outside the allowed list, `1` a difference, `2` a harness error.

End-to-end installs with the clients found on `PATH` (needs Internet):

```bash
cd compat
node e2e/run.mjs
node e2e/run.mjs --clients npm,pnpm --servers fastopm --keep
```

Multi-arch Docker image:

```bash
docker buildx build --platform linux/amd64,linux/arm64 -t fastopm:dev .
```

## Contributing

Bug reports, compatibility reports (especially for bun and yarn berry) and pull requests are
welcome. Read [CONTRIBUTING.md](CONTRIBUTING.md) first. The main rule: any behaviour a client can
observe must match Verdaccio, or be added to the harness's list of allowed differences with a
reason. This project follows a [Code of Conduct](CODE_OF_CONDUCT.md).

## Security

fastopm does not authenticate clients. Anyone who can reach it can read every cached package and
every package an uplink's credentials give access to. Run it on a trusted network or behind a
reverse proxy that handles TLS and access control.

Please report vulnerabilities privately, as described in [SECURITY.md](SECURITY.md), not in public
issues.

## License

[MIT](LICENSE).

## Acknowledgements

fastopm reproduces the read-path behaviour of [Verdaccio](https://github.com/verdaccio/verdaccio),
and its configuration format, storage layout and Docker image conventions come from that project.
Verdaccio's source was the reference for every compatibility decision, and its published release
is the baseline the test harness compares against. Thanks to the Verdaccio maintainers and
contributors. fastopm is not affiliated with or endorsed by the Verdaccio project; "Verdaccio" is
used here only to describe compatibility.
