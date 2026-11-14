# Contributing to fastopm

Thanks for your interest in fastopm. Bug reports, compatibility reports and pull requests are
welcome. This document explains how to set up a development environment, what a change needs
before it can be merged, and the one rule that shapes most decisions in this project: compatibility
with Verdaccio.

By participating you agree to follow the [Code of Conduct](CODE_OF_CONDUCT.md).

## Scope

fastopm is a read-only npm proxy cache that is compatible with Verdaccio's configuration, storage
layout and read-route HTTP behaviour. Publishing, client authentication, a web UI and plugins are
out of scope (see [What it does and does not do](README.md#what-it-does-and-does-not-do)). If you
want to propose something outside that scope, open an issue to discuss it before writing code.

## Development environment

You need:

- **Rust**, stable, installed with [rustup](https://rustup.rs). CI uses Rust 1.100 (the
  `RUST_TOOLCHAIN` variable in [`.github/workflows/ci.yml`](.github/workflows/ci.yml)); use that
  version or a newer stable one, with the `rustfmt` and `clippy` components.
- **Node.js** 20 or later (CI uses 24) for the compatibility harness in [`compat/`](compat) and
  the benchmarks in [`bench/`](bench). The harness installs `verdaccio@7.1.0` as its baseline.
- **Docker** with BuildKit, only if you change the [`Dockerfile`](Dockerfile) or
  [`docker/`](docker).

```bash
git clone https://github.com/hodfords-solutions/fastopm.git
cd fastopm
cargo build --release --locked -p fastopm
(cd compat && npm ci)
```

The workspace layout is described in [Architecture](README.md#architecture). Each crate's
`lib.rs` documents its behaviour and cites the Verdaccio source files it follows.

## Making a change

1. Open an issue first for anything larger than a small fix, so the approach can be agreed on.
2. Create a branch from `main`.
3. Make the change, with tests. Unit tests live next to the code or in each crate's `tests/`
   folder.
4. Run the same checks as CI:

   ```bash
   cargo fmt --all --check
   cargo clippy --workspace --all-targets --locked -- -D warnings
   cargo test --workspace --locked
   ```

5. If the change can affect HTTP responses, requests sent to uplinks, or files written to storage,
   run the compatibility harness against a release build:

   ```bash
   cargo build --release --locked -p fastopm
   cd compat
   node run.mjs --candidate-bin ../target/release/fastopm
   ```

   It must exit with code `0` (no difference outside the allowed list). Use
   `--filter <name> --verbose` to look at one scenario.

6. If the change affects how real clients install packages, and you have Internet access, run the
   end-to-end script as well (`cd compat && node e2e/run.mjs`, see
   [`compat/e2e/README.md`](compat/e2e/README.md)).

7. Commit `Cargo.lock` together with any `Cargo.toml` change: CI builds with `--locked`.

The workspace forbids `unsafe` code and warns on missing documentation for public items; CI turns
warnings into errors.

## Compatibility with Verdaccio

The reference behaviour is `verdaccio@7.1.0`, the version the harness runs. The rule:

> Anything a client or an operator can observe must match Verdaccio, unless it is listed as an
> allowed difference with a reason.

"Observable" includes status codes, response headers, response bodies byte for byte, the requests
sent to uplinks (headers, retries), the files written to storage, and how `config.yaml` is
interpreted. When in doubt, find what Verdaccio does in its source and confirm it with the harness;
where the two disagree, the harness result wins.

When fastopm has to differ (for example, it has no publish route), the difference goes into
`ALLOWED_DIFFS` in [`compat/lib/rules.mjs`](compat/lib/rules.mjs) with a reason that explains why.
Any comparison exception (an ignored header, a normalisation) also belongs in that file, with its
reason. Do not change a scenario only to make fastopm pass.

When you fix a compatibility bug, add a harness scenario that reproduces it
([`compat/README.md`](compat/README.md) explains how; that README is currently in Vietnamese, and
the scenario files in `compat/scenarios/` are good examples). Check that a new scenario is stable by
running it a few times with Verdaccio on both sides (`node run.mjs --filter <name>`).

## Reporting bugs

Open a GitHub issue with:

- the fastopm version (`fastopm --version`) and how you run it (binary, Docker image tag);
- the package manager and its version;
- the relevant part of `config.yaml`, **with tokens and internal host names removed**;
- the request and response, or the client output, and the fastopm log (for example with
  `log.level: debug` or `RUST_LOG=debug`);
- what Verdaccio does in the same situation, if you know.

A difference from Verdaccio's behaviour is a bug unless it is in the allowed-difference list.

Do not report security vulnerabilities in public issues; see [SECURITY.md](SECURITY.md).

## Pull requests

- Keep each pull request to one topic, and describe what changed and why.
- Reference the issue it addresses.
- Say which checks you ran (fmt, clippy, tests, harness, e2e) and with which clients.
- Update the README or the crate documentation when behaviour or configuration changes.
- CI must pass. A maintainer will review; expect questions about compatibility.

## License

By contributing, you agree that your contributions are licensed under the [MIT License](LICENSE)
that covers this project.
