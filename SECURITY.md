# Security policy

## Supported versions

fastopm is at an early stage (0.1.x). Security fixes are made on `main` and in the latest release
only.

| Version | Supported |
|---|---|
| latest 0.1.x release and `main` | yes |
| anything older | no |

## Reporting a vulnerability

Please do **not** open a public issue, discussion or pull request for a security problem.

Report it privately through GitHub's vulnerability reporting:
[**Report a vulnerability**](https://github.com/hodfords-solutions/fastopm/security/advisories/new)
(the "Security" tab of the repository, then "Report a vulnerability").

Include what you can of:

- the affected version or commit, and how fastopm is run (binary, Docker image tag);
- a description of the issue and its impact;
- steps or a minimal configuration and requests that reproduce it;
- any suggested fix.

Remove real tokens, passwords and internal host names from configuration excerpts.

We will acknowledge the report, keep you informed while we investigate, and agree on a disclosure
date with you before details are published. Please give us reasonable time to release a fix
before disclosing the issue publicly.

## Deployment notes

These are properties of fastopm's design, not vulnerabilities. Take them into account when you
deploy it.

**No client authentication.** fastopm does not authenticate or authorize clients. The
`Authorization` header sent by clients is ignored, and `access`, `publish` and `unpublish` rules
under `packages:` in `config.yaml` are not enforced. Anyone who can open a connection to fastopm can
read:

- every package in its storage, including packages that were published privately to a Verdaccio
  instance that used the same storage;
- every package that the configured uplinks give access to. If an uplink has credentials
  (`auth` or an `Authorization` entry in `headers`), fastopm effectively shares that access with
  all of its clients.

Run fastopm only on a trusted network, bound to a private address, or behind a reverse proxy that
restricts who can reach it. The default `listen` address is `localhost:4873`; the Docker image
listens on all interfaces (`[::]:4873`), so restrict access to the published port.

**No TLS.** fastopm serves plain HTTP and refuses an `https://` listen address. Terminate TLS in a
reverse proxy in front of it. Behind a proxy, set `VERDACCIO_PUBLIC_URL` or forward the original
protocol (`X-Forwarded-Proto`, or the header named by `VERDACCIO_FORWARDED_PROTO`) so tarball URLs
point to the right address.

**Uplink credentials in the configuration.** Uplink tokens can be written inline
(`auth.token`) or read from an environment variable (`auth.token_env`, default `NPM_TOKEN`).
Prefer `token_env` and keep tokens out of `config.yaml`. If the file contains tokens or private
`headers`, restrict its permissions, keep it out of version control, and remove secrets before
sharing it in an issue. fastopm sends uplink credentials only to URLs served by that uplink; tarball
URLs on other hosts are fetched without credentials.

**Trust in the upstream.** fastopm caches what its uplinks return. Package managers verify the
`integrity` of tarballs against the metadata, but the metadata itself comes from the uplink. Use
`https://` uplink URLs.

**Storage shared with Verdaccio.** fastopm writes to the storage folder with the same layout and
permissions model as Verdaccio (the Docker image runs as UID 10001, group 0). Anyone who can write
to that folder can change what clients install.
