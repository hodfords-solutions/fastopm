# fastopm Helm chart

Deploys [fastopm](https://github.com/hodfords-solutions/fastopm), a read-only npm proxy cache
that reads Verdaccio's `config.yaml` and storage layout, on Kubernetes.

fastopm only serves reads. It does not publish packages, authenticate clients, serve a web UI or
load plugins. Read the [project README](../../README.md) before replacing Verdaccio with it.

The values follow the [Verdaccio chart](https://github.com/verdaccio/charts) wherever the meaning
is the same, so most values written for that chart can be reused
(see [Coming from the Verdaccio chart](#coming-from-the-verdaccio-chart)).

## Image

**No fastopm image is published on Docker Hub yet.** The chart defaults to
`hodfords/fastopm:<appVersion>`, which will not pull until images are released. Until then, build
the image from the repository and push it to a registry your cluster can pull from:

```bash
git clone https://github.com/hodfords-solutions/fastopm.git
cd fastopm
docker build -t registry.example.com/fastopm:0.1.0 .
docker push registry.example.com/fastopm:0.1.0

helm install npm ./charts/fastopm \
  --set image.repository=registry.example.com/fastopm \
  --set image.tag=0.1.0
```

On a local [kind](https://kind.sigs.k8s.io) cluster you can load the image instead of pushing it:

```bash
docker build -t fastopm:local .
kind load docker-image fastopm:local
helm install npm ./charts/fastopm \
  --set image.repository=fastopm --set image.tag=local --set image.pullPolicy=Never
```

## Installing

From the chart repository published on GitHub Pages:

```bash
helm repo add fastopm https://hodfords-solutions.github.io/fastopm
helm repo update
helm install npm fastopm/fastopm --namespace npm --create-namespace
```

From a checkout of the repository:

```bash
helm install npm ./charts/fastopm --namespace npm --create-namespace
```

Check it, then point a package manager at it:

```bash
helm test npm --namespace npm      # GET /-/ping through the service
kubectl --namespace npm port-forward svc/npm-fastopm 4873:4873
npm config set registry http://127.0.0.1:4873/
```

Upgrade and uninstall:

```bash
helm upgrade npm fastopm/fastopm --namespace npm -f my-values.yaml
helm uninstall npm --namespace npm
```

`helm uninstall` deletes the cache volume created by the chart unless
`persistence.resourcePolicy=keep`. Volumes created by a StatefulSet (`type=statefulset`) are not
deleted by Helm.

## Configuration

`configMap` holds fastopm's `config.yaml`. The default proxies every package to
`https://registry.npmjs.org/` and caches it under `/verdaccio/storage/data` (the same settings as
the image's [`docker/config.yaml`](../../docker/config.yaml)). Only the keys listed in the project
README are used; any other Verdaccio key is ignored with a warning in the pod log, so an existing
Verdaccio config can be pasted as it is. Changing `configMap` restarts the pods.

Keep `storage` under `/verdaccio/storage`, the mounted volume. `listen` is ignored: the container
always listens on port 4873.

Uplink credentials should not be written into `configMap`. Pass them as environment variables and
reference them with `token_env`:

```yaml
configMap: |
  storage: /verdaccio/storage/data
  uplinks:
    internal:
      url: https://npm.internal.example.com/
      auth:
        type: bearer
        token_env: INTERNAL_NPM_TOKEN
  packages:
    '@internal/*':
      proxy: internal
    '**':
      proxy: internal
existingSecret: npm-uplink-tokens   # Secret with an INTERNAL_NPM_TOKEN key
```

### Running behind an ingress

fastopm writes absolute `dist.tarball` URLs into package metadata. By default it builds them from
the request's `Host` header and `X-Forwarded-Proto`, which most ingress controllers forward, so
an ingress on its own host usually needs nothing else:

```yaml
ingress:
  enabled: true
  className: nginx
  hosts: [npm.example.com]
  tls:
    - secretName: npm-example-com-tls
      hosts: [npm.example.com]
```

When the controller rewrites `Host`, or clients reach fastopm through another proxy, set the public
URL explicitly:

```yaml
publicUrl: https://npm.example.com/   # sets VERDACCIO_PUBLIC_URL
```

fastopm serves its routes at the root. To publish it under a sub-path such as
`https://example.com/npm/`, make the ingress strip the prefix (with ingress-nginx, a regex path
and `nginx.ingress.kubernetes.io/rewrite-target`) and add `url_prefix: /npm/` to `configMap`, so
the tarball URLs carry the prefix.

`gatewayApi.enabled=true` creates an HTTPRoute instead of (or as well as) the Ingress.

### Persistence

By default the chart creates a `ReadWriteOnce` PersistentVolumeClaim of 8 GiB, mounted at
`/verdaccio/storage`, and the Deployment uses the `Recreate` strategy so the old and the new pod
never need the volume at the same time.

`persistence.existingClaim` mounts an existing claim instead, for example the claim of a Verdaccio
release: fastopm reads and writes the same layout. Stop Verdaccio first; running both on one
storage folder at the same time has not been tested.

The pod runs as UID 10001 and `podSecurityContext.fsGroup` (default `10001`) makes the volume
writable. A claim used by the Verdaccio chart (`fsGroup: 101`) is re-owned to group 10001 the
first time it is mounted, which can take a while on a large cache; set
`podSecurityContext.fsGroup=101` to keep its ownership instead.

`persistence.enabled=false` uses an `emptyDir`: the cache is lost when the pod is deleted.

### Running more than one replica

fastopm keeps its cache on disk, and the chart defaults to one replica. Run more than one only in
one of these setups:

| Setup | Values | Status |
|---|---|---|
| One volume per replica, each replica an independent cache | `type=statefulset`, `replicaCount=N` | Covered by the chart CI: install with 2 replicas and `helm test` ([`ci/statefulset-values.yaml`](ci/statefulset-values.yaml)) |
| Throw-away cache per replica | `persistence.enabled=false`, `replicaCount=N` | Covered by the chart CI: install with 2 replicas and `helm test` ([`ci/no-persistence-values.yaml`](ci/no-persistence-values.yaml)) |
| One `ReadWriteMany` volume shared by all replicas | `persistence.accessMode=ReadWriteMany`, a storage class that supports it, `replicaCount=N` | **Not tested.** fastopm writes each file to a temporary name and renames it, but several processes writing to one storage folder at the same time are not covered by any test |

With independent caches, each replica fetches a package from the uplink the first time it is asked
for it. `podDisruptionBudget.enabled=true` keeps replicas available during node drains.

## Coming from the Verdaccio chart

Same keys, same meaning: `image.*`, `type`, `nameOverride`, `fullnameOverride`, `replicaCount`,
`revisionHistoryLimit`, `strategy`, `service.*`, `ingress.*`, `gatewayApi.*`, `configMap`,
`existingConfigMap`, `persistence.*`, `extraEnvVars`, `secretEnvVars`, `existingSecret`,
`startupProbe`, `livenessProbe`, `readinessProbe`, `resources`, `podSecurityContext`,
`securityContext`, `serviceAccount.*`, `annotations`, `podLabels`, `podAnnotations`,
`nodeSelector`, `tolerations`, `affinity`, `topologySpreadConstraints`, `priorityClass.*`,
`hostAliases`, `extraInitContainers`, `podDisruptionBudget.*`, `extraManifests`.

Differences:

| Verdaccio chart | This chart | Why |
|---|---|---|
| `secrets.htpasswd`, `secrets.existingSecretHtpasswd*` | removed | fastopm does not authenticate clients |
| `cachingNginx.*` | removed | fastopm caches parsed manifests and rendered responses in memory itself |
| `extraPorts` | removed | fastopm listens on one port |
| `autoscaling.*`, `replicaCountEnabled` | removed | Scaling a disk cache needs one of the setups above; an HPA on a Deployment with a `ReadWriteOnce` volume does not work |
| `extraEnvVars` defaults to `VERDACCIO_PORT=4873` | defaults to `[]`; the chart sets `VERDACCIO_PORT` itself unless `extraEnvVars` does | |
| service links enabled | `enableServiceLinks: false` | a Service named `verdaccio` injects `VERDACCIO_PORT=tcp://...` and breaks the listen address |
| — | `publicUrl` | sets `VERDACCIO_PUBLIC_URL` |
| `podSecurityContext.fsGroup: 101`, `securityContext.runAsUser: 10001` | `fsGroup: 10001` plus a restricted security context (non-root, no privilege escalation, all capabilities dropped, read-only root filesystem, `RuntimeDefault` seccomp) | the fastopm image writes only to `/verdaccio/storage` |
| `existingConfigMap: false` | `existingConfigMap: ""` (a name) | The ConfigMap is not rendered when it is set |
| — | `test.enabled` | `helm test` pod calling `GET /-/ping` |
| Ingress API fallbacks for Kubernetes < 1.19 | `networking.k8s.io/v1` only | the chart requires Kubernetes 1.23 or later |
| StatefulSet: `podManagementPolicy` default (`OrderedReady`) | `Parallel` | replicas are independent caches |
| `persistence.storageClass` unset renders `storageClassName: ""` in StatefulSet claims | unset omits `storageClassName` (cluster default); `"-"` renders `""` | |

The `configMap` of the Verdaccio chart loads as it is, but `auth`, `web`, `middlewares` and the
`access` / `publish` rules are ignored, so remove them to avoid the startup warnings.

## Parameters

| Key | Default | Description |
|---|---|---|
| `image.repository` | `hodfords/fastopm` | Image repository (not published yet, see [Image](#image)) |
| `image.tag` | `""` | Image tag; defaults to the chart's `appVersion` |
| `image.digest` | `""` | Image digest; takes precedence over the tag |
| `image.pullPolicy` | `IfNotPresent` | Image pull policy |
| `image.pullSecrets` | `[]` | Names of image pull secrets (merged with `global.image.pullSecrets`) |
| `type` | `deployment` | `deployment` or `statefulset` (one claim per replica) |
| `nameOverride` | `""` | Override the chart name |
| `fullnameOverride` | `""` | Override the full resource name |
| `replicaCount` | `1` | Number of pods; see [Running more than one replica](#running-more-than-one-replica) |
| `revisionHistoryLimit` | `10` | Old ReplicaSets / revisions kept |
| `strategy` | `{}` | Deployment strategy; empty: `Recreate` with persistence, `RollingUpdate` without |
| `service.type` | `ClusterIP` | Service type |
| `service.port` | `4873` | Service port |
| `service.targetPort` | `""` | Target port; empty: the container port `http` (4873) |
| `service.name` | `""` | Port name; empty: `http` |
| `service.nodePort` | unset | Node port for `NodePort` / `LoadBalancer` |
| `service.annotations` | `{}` | Service annotations |
| `service.clusterIP` | `""` | Fixed cluster IP |
| `service.externalIPs` | `[]` | External IPs |
| `service.loadBalancerIP` | `""` | Load balancer IP |
| `service.loadBalancerSourceRanges` | `[]` | Allowed source ranges for a load balancer |
| `ingress.enabled` | `false` | Create an Ingress |
| `ingress.className` | `""` | Ingress class |
| `ingress.hosts` | `[]` | Host names (rendered through `tpl`); empty: one rule without host |
| `ingress.paths` | `["/"]` | Paths sent to fastopm on every host |
| `ingress.pathType` | `Prefix` | Path type of `paths` and default of `extraPaths` |
| `ingress.extraPaths` | `[]` | Path entries placed first (`path`, `pathType`, `service`, `port`) |
| `ingress.annotations` | `{}` | Ingress annotations (rendered through `tpl`) |
| `ingress.labels` | `{}` | Extra Ingress labels |
| `ingress.tls` | `[]` | Ingress TLS section |
| `gatewayApi.enabled` | `false` | Create a Gateway API HTTPRoute |
| `gatewayApi.parentRefs` | `[]` | Gateways the route attaches to |
| `gatewayApi.hostnames` | `[]` | Route host names |
| `gatewayApi.rules` | `[]` | Custom rules; empty: `PathPrefix /` to the service |
| `gatewayApi.annotations`, `gatewayApi.labels` | `{}` | HTTPRoute metadata |
| `publicUrl` | `""` | Public base URL; sets `VERDACCIO_PUBLIC_URL` |
| `configMap` | see `values.yaml` | fastopm `config.yaml` (rendered through `tpl`) |
| `existingConfigMap` | `""` | Existing ConfigMap with a `config.yaml` key, used instead of `configMap` |
| `persistence.enabled` | `true` | Store the cache on a PersistentVolumeClaim; `false`: `emptyDir` |
| `persistence.existingClaim` | `""` | Existing claim to mount |
| `persistence.storageClass` | `""` | Storage class; `"-"`: `storageClassName: ""` |
| `persistence.accessMode` | `ReadWriteOnce` | Access mode |
| `persistence.size` | `8Gi` | Requested size |
| `persistence.annotations` | `{}` | Claim annotations |
| `persistence.resourcePolicy` | `""` | `keep`: `helm uninstall` keeps the claim |
| `persistence.selector` | `{}` | Selector for an existing PersistentVolume |
| `persistence.volumes` | `[]` | Extra pod volumes (rendered through `tpl`) |
| `persistence.mounts` | `[]` | Extra container volume mounts (rendered through `tpl`) |
| `extraEnvVars` | `[]` | Extra container environment variables |
| `secretEnvVars` | `{}` | Variables stored in a Secret created by the chart |
| `existingSecret` | `""` | Existing Secret whose keys become environment variables |
| `enableServiceLinks` | `false` | Kubernetes service link variables |
| `startupProbe.*` | disabled, 10 s period, 30 failures | Startup probe on `GET /-/ping` |
| `livenessProbe.*` | enabled, 5 s delay, 10 s period, 6 failures | Liveness probe on `GET /-/ping` |
| `readinessProbe.*` | enabled, 2 s delay, 10 s period, 3 failures | Readiness probe on `GET /-/ping` |
| `resources` | `{}` | Container resources |
| `podSecurityContext` | `fsGroup: 10001`, `fsGroupChangePolicy: OnRootMismatch`, `runAsNonRoot`, `RuntimeDefault` seccomp | Pod security context |
| `securityContext` | `runAsUser: 10001`, `runAsNonRoot`, no privilege escalation, `drop: [ALL]`, `readOnlyRootFilesystem` | Container security context |
| `serviceAccount.create` | `false` | Create a ServiceAccount |
| `serviceAccount.name` | `""` | ServiceAccount name |
| `serviceAccount.annotations` | `{}` | ServiceAccount annotations |
| `serviceAccount.automountServiceAccountToken` | `false` | Mount the API token (fastopm does not use it) |
| `annotations` | `{}` | Deployment / StatefulSet / PodDisruptionBudget annotations |
| `podLabels`, `podAnnotations` | `{}` | Extra pod labels and annotations (rendered through `tpl`) |
| `nodeSelector`, `tolerations`, `affinity`, `topologySpreadConstraints` | empty | Scheduling |
| `priorityClass.enabled`, `priorityClass.name` | `false`, `""` | Pod priority class |
| `hostAliases` | `[]` | Extra `/etc/hosts` entries |
| `extraInitContainers` | `[]` | Init containers (rendered through `tpl`) |
| `podDisruptionBudget.enabled` | `false` | Create a PodDisruptionBudget |
| `podDisruptionBudget.maxUnavailable` | `1` | Ignored when `minAvailable` is set |
| `podDisruptionBudget.minAvailable` | unset | Minimum available pods |
| `test.enabled` | `true` | Render the `helm test` pod |
| `extraManifests` | `[]` | Extra manifests (rendered through `tpl`) |

## Development

```bash
helm lint --strict charts/fastopm
helm template npm charts/fastopm --set ingress.enabled=true --set 'ingress.hosts={npm.example.com}'
```

[`.github/workflows/helm.yml`](../../.github/workflows/helm.yml) runs `helm lint`, `ct lint` and,
on a kind cluster, `ct install` (install + `helm test`) for every file in [`ci/`](ci), with an
image built from the repository's Dockerfile. Bump `version` in `Chart.yaml` in every pull
request that changes the chart; merging to `main` publishes it.
