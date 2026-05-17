#!/usr/bin/env bash
# Stage 1/4 of the upload pipeline.
#
# Inspects a binary on disk and emits the metadata downstream stages need:
# SHA256, byte size, ISO-8601 build timestamp, comma-joined metadata string
# for the gcloud `--custom-metadata` flag, and the path to a sidecar
# `.sha256` checksum file (compatible with `shasum -a 256 -c`).
#
# Pure compute — no GCS writes, no signing, no network. Safe to dry-run by
# default. Idempotent: re-running with the same inputs overwrites the same
# context keys.
#
# Contract — all stages in the pipeline share state via a "context file"
# (bash-sourceable KEY=value lines). This stage APPENDS:
#   SHA256          hex digest of $BINARY
#   SIZE            byte count
#   BUILD_TIME      ISO-8601 UTC (the time this stage ran, not the file mtime)
#   CHECKSUM_FILE   path to the generated <name><exe-suffix>.sha256 sidecar
#   META            "version=…,sha256=…,build_time=…,size_bytes=…,os=…,arch=…,
#                    <caller-supplied k=v…>"  — caller-supplied wins on collision
#
# The CHECKSUM_FILE path is a mktemp; the orchestrator owns its lifecycle and
# is expected to clean it up via trap.

set -euo pipefail

usage() {
  cat <<'EOF'
compute-metadata.sh — stage 1/4 of the upload pipeline.

Computes SHA256, size, build timestamp, custom-metadata string, and a sidecar
checksum file for a binary. Appends results to a shared context file.

Usage:
  scripts/upload/compute-metadata.sh \
    --context <file>           shared context (bash key=value lines; appended to)
    --binary  <path>           file to inspect (required)
    --name    <str>            artifact name (required; used in checksum file)
    --version <str>            version tag (goes into metadata)
    --exe-suffix <str>         "" or ".exe" (used to name the checksum file
                               correctly so `shasum -c` works on Windows
                               downloads). Default: "".
    --platform <os/arch>       optional; auto-injects os= and arch= meta
    --meta    <key=value>      extra metadata (repeatable; cannot contain commas)
    -h|--help

The context file is created if it doesn't exist. On success, it gains:
  SHA256=<hex>
  SIZE=<bytes>
  BUILD_TIME=<iso8601>
  CHECKSUM_FILE=<path>
  META=<comma,joined,kvs>
EOF
}

CONTEXT=""; BINARY=""; NAME=""; VERSION=""; EXE_SUFFIX=""; PLATFORM=""
declare -a META=()

while [[ $# -gt 0 ]]; do
  case "$1" in
    --context)     CONTEXT="$2"; shift 2 ;;
    --binary)      BINARY="$2"; shift 2 ;;
    --name)        NAME="$2"; shift 2 ;;
    --version)     VERSION="$2"; shift 2 ;;
    --exe-suffix)  EXE_SUFFIX="$2"; shift 2 ;;
    --platform)    PLATFORM="$2"; shift 2 ;;
    --meta)        META+=("$2"); shift 2 ;;
    -h|--help)     usage; exit 0 ;;
    *) echo "compute-metadata: unknown arg: $1" >&2; usage >&2; exit 1 ;;
  esac
done

[[ -n "$CONTEXT" ]] || { echo "compute-metadata: --context required" >&2; exit 1; }
[[ -n "$BINARY"  ]] || { echo "compute-metadata: --binary required"  >&2; exit 1; }
[[ -n "$NAME"    ]] || { echo "compute-metadata: --name required"    >&2; exit 1; }
[[ -n "$VERSION" ]] || { echo "compute-metadata: --version required" >&2; exit 1; }
[[ -f "$BINARY"  ]] || { echo "compute-metadata: binary not found: $BINARY" >&2; exit 1; }

command -v shasum >/dev/null 2>&1 || { echo "compute-metadata: shasum required" >&2; exit 1; }

SIZE=$(wc -c < "$BINARY" | tr -d ' ')
SHA256=$(shasum -a 256 "$BINARY" | cut -d' ' -f1)
BUILD_TIME=$(date -u +%Y-%m-%dT%H:%M:%SZ)

# Sidecar checksum file in the format `shasum -a 256 -c` expects:
# "<hex>  <filename>\n". The filename includes the .exe suffix on Windows
# so verification works on the downloaded artifact name.
CHECKSUM_FILE=$(mktemp)
printf '%s  %s\n' "$SHA256" "${NAME}${EXE_SUFFIX}" > "$CHECKSUM_FILE"

# Build the comma-joined metadata string for gcloud's --custom-metadata.
# Auto-keys first (so caller --meta can override), then platform-derived keys
# (os, arch), then caller-supplied extras. Duplicate keys are NOT deduplicated
# here — gcloud takes the last value, so caller-supplied wins by construction.
META_PAIRS=(
  "version=$VERSION"
  "sha256=$SHA256"
  "build_time=$BUILD_TIME"
  "size_bytes=$SIZE"
)
if [[ -n "$PLATFORM" && "$PLATFORM" == */* ]]; then
  META_PAIRS+=("os=${PLATFORM%%/*}" "arch=${PLATFORM#*/}")
fi
for m in "${META[@]:-}"; do
  [[ -n "$m" ]] || continue
  [[ "$m" == *"="* ]] || { echo "compute-metadata: --meta must be key=value: $m" >&2; exit 1; }
  case "$m" in
    *,*) echo "compute-metadata: --meta values cannot contain commas (gcloud limit): $m" >&2; exit 1 ;;
  esac
  META_PAIRS+=("$m")
done
META_ARG=$(IFS=,; echo "${META_PAIRS[*]}")

# Append to context. shell-quote VALUES so spaces / special chars survive
# the next `source` of the context file. Keys are bare identifiers.
{
  printf 'SHA256=%q\n'        "$SHA256"
  printf 'SIZE=%q\n'          "$SIZE"
  printf 'BUILD_TIME=%q\n'    "$BUILD_TIME"
  printf 'CHECKSUM_FILE=%q\n' "$CHECKSUM_FILE"
  printf 'META=%q\n'          "$META_ARG"
} >> "$CONTEXT"

# Human-readable progress on stderr (orchestrator captures stdout as protocol).
echo "  [metadata]   sha256=${SHA256:0:16}…  size=${SIZE}B  build_time=${BUILD_TIME}" >&2
