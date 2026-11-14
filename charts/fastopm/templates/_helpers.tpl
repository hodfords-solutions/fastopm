{{/*
Expand the name of the chart.
*/}}
{{- define "fastopm.name" -}}
{{- default .Chart.Name .Values.nameOverride | trunc 63 | trimSuffix "-" }}
{{- end }}

{{/*
Create a default fully qualified app name (63 characters at most, DNS naming spec).
If the release name contains the chart name it is used as the full name.
*/}}
{{- define "fastopm.fullname" -}}
{{- if .Values.fullnameOverride }}
{{- .Values.fullnameOverride | trunc 63 | trimSuffix "-" }}
{{- else }}
{{- $name := default .Chart.Name .Values.nameOverride }}
{{- if contains $name .Release.Name }}
{{- .Release.Name | trunc 63 | trimSuffix "-" }}
{{- else }}
{{- printf "%s-%s" .Release.Name $name | trunc 63 | trimSuffix "-" }}
{{- end }}
{{- end }}
{{- end }}

{{/*
Chart name and version, as used by the chart label.
*/}}
{{- define "fastopm.chart" -}}
{{- printf "%s-%s" .Chart.Name .Chart.Version | replace "+" "_" | trunc 63 | trimSuffix "-" }}
{{- end }}

{{/*
Common labels
*/}}
{{- define "fastopm.labels" -}}
helm.sh/chart: {{ include "fastopm.chart" . }}
{{ include "fastopm.selectorLabels" . }}
app.kubernetes.io/version: {{ .Values.image.tag | default .Chart.AppVersion | trunc 63 | quote }}
app.kubernetes.io/managed-by: {{ .Release.Service }}
{{- end }}

{{/*
Selector labels
*/}}
{{- define "fastopm.selectorLabels" -}}
app.kubernetes.io/name: {{ include "fastopm.name" . }}
app.kubernetes.io/instance: {{ .Release.Name }}
{{- end }}

{{/*
Name of the service account to use
*/}}
{{- define "fastopm.serviceAccountName" -}}
{{- if .Values.serviceAccount.create }}
{{- default (include "fastopm.fullname" .) .Values.serviceAccount.name }}
{{- else }}
{{- default "default" .Values.serviceAccount.name }}
{{- end }}
{{- end }}

{{/*
Name of the ConfigMap holding config.yaml
*/}}
{{- define "fastopm.configMapName" -}}
{{- .Values.existingConfigMap | default (include "fastopm.fullname" .) }}
{{- end }}

{{/*
Renders a value that contains template.
Usage: {{ include "fastopm.tplvalues.render" (dict "value" .Values.path.to.the.Value "context" $) }}
*/}}
{{- define "fastopm.tplvalues.render" -}}
{{- if typeIs "string" .value }}
{{- tpl .value .context }}
{{- else }}
{{- tpl (.value | toYaml) .context }}
{{- end }}
{{- end -}}

{{/*
Container image. Digest takes precedence over the tag; the version label keeps using the tag.
*/}}
{{- define "fastopm.image" -}}
{{- if .Values.image.digest -}}
{{- printf "%s@%s" .Values.image.repository .Values.image.digest -}}
{{- else -}}
{{- printf "%s:%s" .Values.image.repository (.Values.image.tag | default .Chart.AppVersion) -}}
{{- end -}}
{{- end -}}

{{/*
imagePullSecrets from image.pullSecrets and global.image.pullSecrets
*/}}
{{- define "fastopm.imagePullSecrets" -}}
{{- $pullSecrets := list }}
{{- with .Values.global }}
{{- with .image }}
{{- range .pullSecrets }}
{{- $pullSecrets = append $pullSecrets . }}
{{- end }}
{{- end }}
{{- end }}
{{- range .Values.image.pullSecrets }}
{{- $pullSecrets = append $pullSecrets . }}
{{- end }}
{{- if $pullSecrets }}
imagePullSecrets:
{{- range $pullSecrets | uniq }}
  - name: {{ . }}
{{- end }}
{{- end }}
{{- end -}}

{{/*
Pod labels: common labels + global.podLabels + podLabels (values rendered through tpl)
*/}}
{{- define "fastopm.podLabels" -}}
{{- include "fastopm.labels" . }}
{{- $labels := dict }}
{{- with .Values.global }}
{{- range $k, $v := .podLabels }}
{{- $_ := set $labels $k (tpl (toString $v) $) }}
{{- end }}
{{- end }}
{{- range $k, $v := .Values.podLabels }}
{{- $_ := set $labels $k (tpl (toString $v) $) }}
{{- end }}
{{- with $labels }}
{{ toYaml . }}
{{- end }}
{{- end -}}

{{/*
Pod annotations: global.podAnnotations + podAnnotations (values rendered through tpl)
*/}}
{{- define "fastopm.podAnnotations" -}}
{{- $annotations := dict }}
{{- with .Values.global }}
{{- range $k, $v := .podAnnotations }}
{{- $_ := set $annotations $k (tpl (toString $v) $) }}
{{- end }}
{{- end }}
{{- range $k, $v := .Values.podAnnotations }}
{{- $_ := set $annotations $k (tpl (toString $v) $) }}
{{- end }}
{{- with $annotations }}
{{- toYaml . }}
{{- end }}
{{- end -}}

{{/*
Annotations of the pod template: config / secret checksums (pods restart when they change)
followed by the user's pod annotations.
*/}}
{{- define "fastopm.podTemplateAnnotations" -}}
{{- if not .Values.existingConfigMap }}
checksum/config: {{ include (print $.Template.BasePath "/configmap.yaml") . | sha256sum }}
{{- end }}
{{- if .Values.secretEnvVars }}
checksum/env-secret: {{ include (print $.Template.BasePath "/secret.yaml") . | sha256sum }}
{{- end }}
{{- with (include "fastopm.podAnnotations" .) }}
{{ . }}
{{- end }}
{{- end -}}

{{/*
Health probes on GET /-/ping. Fallback values cover --reuse-values upgrades.
*/}}
{{- define "fastopm.probes" -}}
{{- $startup := dict "enabled" false "initialDelaySeconds" 0 "periodSeconds" 10 "timeoutSeconds" 5 "successThreshold" 1 "failureThreshold" 30 }}
{{- $liveness := dict "enabled" true "initialDelaySeconds" 5 "periodSeconds" 10 "timeoutSeconds" 5 "successThreshold" 1 "failureThreshold" 6 }}
{{- $readiness := dict "enabled" true "initialDelaySeconds" 2 "periodSeconds" 10 "timeoutSeconds" 5 "successThreshold" 1 "failureThreshold" 3 }}
{{- $defaults := dict "startupProbe" $startup "livenessProbe" $liveness "readinessProbe" $readiness }}
{{- range $name, $settings := $defaults }}
{{- $probe := mergeOverwrite (deepCopy $settings) (index $.Values $name | default dict) }}
{{- if $probe.enabled }}
{{ $name }}:
  httpGet:
    path: /-/ping
    port: http
  initialDelaySeconds: {{ $probe.initialDelaySeconds }}
  periodSeconds: {{ $probe.periodSeconds }}
  timeoutSeconds: {{ $probe.timeoutSeconds }}
  {{- if eq $name "readinessProbe" }}
  successThreshold: {{ $probe.successThreshold }}
  {{- else }}
  successThreshold: 1
  {{- end }}
  failureThreshold: {{ $probe.failureThreshold }}
{{- end }}
{{- end }}
{{- end -}}

{{/*
Environment of the fastopm container. VERDACCIO_PORT is pinned to the container port unless
extraEnvVars sets it, so a variable injected by a service link can never change the port.
*/}}
{{- define "fastopm.env" -}}
{{- $names := list }}
{{- range .Values.extraEnvVars }}
{{- $names = append $names .name }}
{{- end }}
{{- if and .Values.publicUrl (has "VERDACCIO_PUBLIC_URL" $names) }}
{{- fail "set either publicUrl or VERDACCIO_PUBLIC_URL in extraEnvVars, not both" }}
{{- end }}
{{- if not (has "VERDACCIO_PORT" $names) }}
- name: VERDACCIO_PORT
  value: "4873"
{{- end }}
{{- with .Values.publicUrl }}
- name: VERDACCIO_PUBLIC_URL
  value: {{ tpl . $ | quote }}
{{- end }}
{{- with .Values.extraEnvVars }}
{{ toYaml . }}
{{- end }}
{{- end -}}

{{/*
Pod template shared by the Deployment and the StatefulSet.
*/}}
{{- define "fastopm.podTemplate" -}}
metadata:
  {{- with (include "fastopm.podTemplateAnnotations" . | trim) }}
  annotations:
    {{- . | nindent 4 }}
  {{- end }}
  labels:
    {{- include "fastopm.podLabels" . | nindent 4 }}
spec:
  serviceAccountName: {{ include "fastopm.serviceAccountName" . }}
  automountServiceAccountToken: {{ .Values.serviceAccount.automountServiceAccountToken }}
  enableServiceLinks: {{ .Values.enableServiceLinks }}
  {{- with (include "fastopm.imagePullSecrets" . | trim) }}
  {{- . | nindent 2 }}
  {{- end }}
  {{- with .Values.podSecurityContext }}
  securityContext:
    {{- toYaml . | nindent 4 }}
  {{- end }}
  {{- with .Values.extraInitContainers }}
  initContainers:
    {{- include "fastopm.tplvalues.render" (dict "value" . "context" $) | nindent 4 }}
  {{- end }}
  containers:
    - name: {{ include "fastopm.name" . }}
      image: {{ include "fastopm.image" . | quote }}
      imagePullPolicy: {{ .Values.image.pullPolicy }}
      {{- if or .Values.secretEnvVars .Values.existingSecret }}
      envFrom:
        {{- if .Values.secretEnvVars }}
        - secretRef:
            name: {{ include "fastopm.fullname" . }}-env
        {{- end }}
        {{- if .Values.existingSecret }}
        - secretRef:
            name: {{ tpl .Values.existingSecret . }}
        {{- end }}
      {{- end }}
      env:
        {{- include "fastopm.env" . | trim | nindent 8 }}
      ports:
        - name: http
          containerPort: 4873
          protocol: TCP
      {{- include "fastopm.probes" . | trim | nindent 6 }}
      {{- with .Values.securityContext }}
      securityContext:
        {{- toYaml . | nindent 8 }}
      {{- end }}
      {{- with .Values.resources }}
      resources:
        {{- toYaml . | nindent 8 }}
      {{- end }}
      volumeMounts:
        - name: config
          mountPath: /verdaccio/conf
          readOnly: true
        - name: storage
          mountPath: /verdaccio/storage
        {{- with .Values.persistence.mounts }}
        {{- include "fastopm.tplvalues.render" (dict "value" . "context" $) | nindent 8 }}
        {{- end }}
  volumes:
    - name: config
      configMap:
        name: {{ include "fastopm.configMapName" . }}
    {{- if and .Values.persistence.enabled .Values.persistence.existingClaim }}
    - name: storage
      persistentVolumeClaim:
        claimName: {{ tpl .Values.persistence.existingClaim . }}
    {{- else if and .Values.persistence.enabled (eq .Values.type "deployment") }}
    - name: storage
      persistentVolumeClaim:
        claimName: {{ include "fastopm.fullname" . }}
    {{- else if not .Values.persistence.enabled }}
    - name: storage
      emptyDir: {}
    {{- end }}
    {{- with .Values.persistence.volumes }}
    {{- include "fastopm.tplvalues.render" (dict "value" . "context" $) | nindent 4 }}
    {{- end }}
  {{- with .Values.affinity }}
  affinity:
    {{- toYaml . | nindent 4 }}
  {{- end }}
  {{- with .Values.nodeSelector }}
  nodeSelector:
    {{- toYaml . | nindent 4 }}
  {{- end }}
  {{- with .Values.tolerations }}
  tolerations:
    {{- toYaml . | nindent 4 }}
  {{- end }}
  {{- if .Values.priorityClass.enabled }}
  priorityClassName: {{ required "priorityClass.name is required when priorityClass.enabled is true" .Values.priorityClass.name }}
  {{- end }}
  {{- with .Values.topologySpreadConstraints }}
  topologySpreadConstraints:
    {{- toYaml . | nindent 4 }}
  {{- end }}
  {{- with .Values.hostAliases }}
  hostAliases:
    {{- toYaml . | nindent 4 }}
  {{- end }}
{{- end -}}

{{/*
Storage class field of a PersistentVolumeClaim spec ("-" means "", empty means omitted).
*/}}
{{- define "fastopm.storageClass" -}}
{{- with .Values.persistence.storageClass }}
{{- if eq "-" . }}
storageClassName: ""
{{- else }}
storageClassName: {{ . | quote }}
{{- end }}
{{- end }}
{{- end -}}

{{/*
Workload kind, validated.
*/}}
{{- define "fastopm.validate" -}}
{{- if not (has .Values.type (list "deployment" "statefulset")) }}
{{- fail (printf "type must be deployment or statefulset, got %q" .Values.type) }}
{{- end }}
{{- end -}}
