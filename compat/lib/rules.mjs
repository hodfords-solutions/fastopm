// ─────────────────────────────────────────────────────────────────────────────
// Every comparison exception lives in this file. If something is ignored or
// normalised, it is listed here with the reason.
// ─────────────────────────────────────────────────────────────────────────────

/** Response headers never compared. */
export const IGNORED_RESPONSE_HEADERS = {
  date: 'wall-clock time of the response',
  connection: 'hop-by-hop connection management (the harness sends Connection: close)',
  'keep-alive': 'hop-by-hop connection management',
  'transfer-encoding':
    'framing detail (chunked vs fixed length); body bytes and Content-Length are compared instead',
  'x-ratelimit-reset':
    'Verdaccio express-rate-limit: epoch seconds of the window end, differs between processes',
  'x-ratelimit-limit':
    'Verdaccio express-rate-limit (userRateLimit) bookkeeping; rate limiting is not part of the proxy-cache contract',
  'x-ratelimit-remaining':
    'Verdaccio express-rate-limit per-process request counter; same reason as x-ratelimit-limit',
};

/**
 * Response headers whose value is a function of the body. When the two bodies are
 * byte-identical they are compared literally; when bodies differ only by the
 * normalisations below, each side must derive the header from its own body the
 * same way the baseline does (e.g. ETag = "md5(body)").
 */
export const BODY_DERIVED_HEADERS = ['etag', 'content-length'];

/** Headers of requests received by the mock uplinks that are not compared. */
export const IGNORED_UPLINK_REQUEST_HEADERS = {
  connection: 'hop-by-hop connection management (keep-alive agent vs not)',
  'keep-alive': 'hop-by-hop connection management',
};

/** Files in the storage directory that are not compared. */
export const IGNORED_STORAGE_FILES = {
  '.verdaccio-db.json':
    "Verdaccio's private database: list of locally published packages and a random secret " +
    'generated at first start. fastopm neither publishes nor signs tokens, so it does not write it.',
  '.token-db.json': "Verdaccio's token database (npm token); out of fastopm scope",
  '.sinopia-db.json': 'legacy name of .verdaccio-db.json',
};

/**
 * Normalisations applied to response bodies, uplink requests and storage files
 * before comparing (each side with its own values).
 */
export const NORMALISATIONS = [
  {
    what: 'mock uplink and registry origins (127.0.0.1:<port>)',
    why: 'each side has its own ports; replaced by {uplink:<name>} / {registry}',
  },
  {
    what: '_rev random part ("3-<16 hex>" -> "3-<hex:16>")',
    why: 'Verdaccio appends a random hex string on each write; the counter and the hex length are kept',
  },
  {
    what: 'package.json _uplinks.<name>.fetched (epoch ms -> <timestamp>)',
    why: 'time of the last uplink contact',
  },
];

/**
 * Scenarios (or steps) whose differences are expected by design and do not fail the run.
 * key: scenario name; value: { reason, steps?: [step names] (default: whole scenario) }.
 */
export const ALLOWED_DIFFS = {
  'metadata/package-name-edge-cases': {
    steps: ['GET /.hidden (dotfile segment)'],
    reason:
      'Verdaccio 7.1.0 bug: a path segment starting with "." goes through the dotfiles middleware ' +
      '(next(404)) and the request is never answered; the harness gives up after 3 s. ' +
      'Answering 404 {"error": "resource not found"} instead is fine.',
  },
  'misc/client-authorization-ignored': {
    reason:
      'fastopm does not authenticate clients and ignores the Authorization header (PLAN §1: no ' +
      'auth, `access` rules not applied). Verdaccio verifies the token and answers 401 ' +
      '"bad username/password, access denied" without contacting the uplink when it is invalid; ' +
      'fastopm serves the request as anonymous. Nothing from the client header is forwarded upstream.',
  },
  'write-routes/publish-and-unpublish': {
    reason:
      'fastopm is read-only: PUT/DELETE package routes answer 405 {"error": ...} (PLAN §1); ' +
      'Verdaccio answers with its auth/publish errors.',
  },
  'write-routes/user-and-token': {
    reason: 'fastopm has no user/login/token routes: they answer 405 (PLAN §1).',
  },
  'write-routes/dist-tags-and-audit': {
    reason:
      'dist-tag and npm audit endpoints are out of scope; fastopm answers 405 (PLAN §1 and §5, audit risk).',
  },
};
