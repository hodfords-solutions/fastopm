import http from 'node:http';

/**
 * One HTTP request on a fresh connection (no agent), raw path preserved.
 * @returns {Promise<{status:number, headers:Record<string,string|string[]>, rawHeaders:string[], body:Buffer, error?:string}>}
 */
export function request(baseUrl, { method = 'GET', path = '/', headers = {}, body } = {}, timeoutMs = 60_000) {
  const url = new URL(baseUrl);
  return new Promise((resolve) => {
    const req = http.request(
      {
        host: url.hostname,
        port: url.port,
        method,
        path,
        headers,
        agent: false,
        setHost: !Object.keys(headers).some((h) => h.toLowerCase() === 'host'),
      },
      (res) => {
        const chunks = [];
        res.on('data', (c) => chunks.push(c));
        res.on('end', () =>
          resolve({
            status: res.statusCode,
            headers: res.headers,
            rawHeaders: res.rawHeaders,
            body: Buffer.concat(chunks),
          })
        );
        res.on('error', (err) =>
          resolve({
            status: res.statusCode,
            headers: res.headers,
            rawHeaders: res.rawHeaders,
            body: Buffer.concat(chunks),
            error: `response aborted: ${err.code ?? err.message}`,
          })
        );
      }
    );
    req.setTimeout(timeoutMs, () => req.destroy(new Error(`timeout after ${timeoutMs}ms`)));
    req.on('error', (err) =>
      resolve({ status: 0, headers: {}, rawHeaders: [], body: Buffer.alloc(0), error: err.code ?? err.message })
    );
    if (body !== undefined) req.write(typeof body === 'string' || Buffer.isBuffer(body) ? body : JSON.stringify(body));
    req.end();
  });
}
