# e2e — real clients, real npmjs

End-to-end test of fastopm as a drop-in Verdaccio proxy cache: real package managers install a real
project through fastopm and through `verdaccio@7.1.0` (from `compat/node_modules`), with the upstream
being the real `https://registry.npmjs.org/`. **Needs Internet.** Not part of the offline harness
(`compat/run.mjs`), not run in CI.

```bash
cargo build --release -p fastopm
cd compat && npm install
node e2e/run.mjs                                  # every client found, both servers
node e2e/run.mjs --clients npm,pnpm --servers fastopm --keep
```

| Option | |
|---|---|
| `--fastopm-bin <path>` | default `../target/release/fastopm` |
| `--clients a,b` | among `npm`, `pnpm`, `yarn` (classic or berry, by version), `bun`; default: every one on `PATH` |
| `--servers a,b` | `fastopm`, `verdaccio` (default both; the cross-storage rounds need both) |
| `--work <dir>` | working directory (default a new one under the system temp dir); logs and lockfiles stay there |
| `--keep` | also keep projects, client caches and storages |
| `--json <file>` | all results as JSON |

Exit code: `0` all rounds passed, `1` a round failed, `2` setup error.

## What it does

Project: `e2e/project/package.json` — ~300 packages, scoped ones (`@babel/*`, `@types/*`,
`@webassemblyjs/*`), several binaries (`tsc`, `eslint`, `webpack`, `prettier`, `semver`, `rimraf`).
Installs run with `--ignore-scripts`.

Each server runs from Verdaccio's default `config.yaml` (copied unchanged from `@verdaccio/config`,
`storage: ./storage`, uplink `npmjs` → registry.npmjs.org) on an empty storage. Rounds, per client:

| Round | Server | Lockfile | Pass when |
|---|---|---|---|
| `direct` | none (registry.npmjs.org) | none | install works; gives the *npmjs lockfile* and a reference time |
| `cold` | fastopm / verdaccio, empty storage | none | install works; gives the *server lockfile* |
| `warm` | same, storage from `cold` | server lockfile | 0 tarballs fetched from the uplink |
| `npmjs-lock` | same | npmjs lockfile | install works (the client checks every `integrity`); same `integrity` for every `name@version` present in both lockfiles |
| `offline-lock` | restarted with `uplinks.npmjs.url` = a closed local port | server lockfile | install works, 0 tarball fetches |
| `offline-nolock` | same | none (resolution from the cached metadata) | install works, 0 tarball fetches |
| `cross(<writer>)` | the *other* server, offline, on a copy of `<writer>`'s storage | `<writer>`'s lockfile | install works, 0 tarball fetches |

Every install also checks the tree: scoped packages present, the six binaries linked,
`tsc --version` runs.

Uplink activity is read from the server log: fastopm runs with
`RUST_LOG=warn,fastopm_server::access=info,fastopm_uplink=info,fastopm_store=info`
(override with `E2E_FASTOPM_RUST_LOG`) and logs `fetching tarball`; Verdaccio logs
`fetching tarball from npmjs - <url>` at its default `http` level.

## Isolation

Every round gets a fresh project directory and a fresh fake `HOME` (with `XDG_*` under it), so client
caches (npm cache, pnpm store, yarn cache, bun cache) start empty and the user's `~/.npmrc`,
`~/.yarnrc`, pnpm or bun configuration is neither read nor written. Inherited `npm_*`, `yarn_*`,
`pnpm_*`, `bun_*`, `corepack_*` and `VERDACCIO_*` variables are dropped. The registry is given on the
command line or in the fake home.

Lockfiles are used as clients use them: npm rewrites `registry.npmjs.org` URLs to the configured
registry itself; pnpm and bun lockfiles carry no URL to rewrite (`127.0.0.1:<port>` origins of an
earlier round are replaced); yarn classic downloads from the `resolved` URL as written, so the
script points those URLs at the server under test (integrity values are left untouched).
Yarn berry's `checksum` is not an npm `integrity`, so the integrity comparison is skipped for it.
