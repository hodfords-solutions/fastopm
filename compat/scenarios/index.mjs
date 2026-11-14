// All scenarios, in run/report order. To add one: create or extend a file in
// this folder and list it here (see README.md "Adding a scenario").

import etag from './etag.mjs';
import maxage from './maxage.mjs';
import metadata from './metadata.mjs';
import misc from './misc.mjs';
import multiUplink from './multi-uplink.mjs';
import preseeded from './preseeded.mjs';
import refreshOnMiss from './refresh-on-miss.mjs';
import tarballUrl from './tarball-url.mjs';
import tarball from './tarball.mjs';
import uplinkFailures from './uplink-failures.mjs';
import version from './version.mjs';
import writeRoutes from './write-routes.mjs';

export const SCENARIOS = [
  ...metadata,
  ...version,
  ...tarball,
  ...etag,
  ...maxage,
  ...uplinkFailures,
  ...multiUplink,
  ...tarballUrl,
  ...misc,
  ...preseeded,
  ...writeRoutes,
  ...refreshOnMiss,
];

const seen = new Set();
for (const s of SCENARIOS) {
  if (seen.has(s.name)) throw new Error(`duplicate scenario name ${s.name}`);
  seen.add(s.name);
}
