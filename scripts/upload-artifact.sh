#!/usr/bin/env bash
# Orchestrator for the signed-upload pipeline. See scripts/upload/*.sh for
# the individual stages. Same external CLI as the old monolith — release-all-
# platforms.sh and any human callers don't need to change anything.
#
# Pipeline:
#   1. compute-metadata.sh   — sha256, size, build_time, .sha256 sidecar, META
#   2. sign-binary.sh        — Ed25519 .sig (skipped if no --signing-key)
#   3. copy-to-gcs.sh        — uploads binary + .sha256 + .sig for both the
#                              versioned URI and the stable pointer (twice)
#   4. publish-transparency-log.sh — log entries for both URIs (skipped if
#                              unsigned or --transparency-log "")
#
# Stages share state via a context file (bash-sourceable KEY=value lines)
# that this orchestrator creates in mktemp and deletes on exit. Each stage
# appends to it; downstream stages source it to pick up upstream outputs.
#
# Backward compat: identical CLI, identical exit code on success, identical
# stdout shape. Existing callers (release-all-platforms.sh, deploy.sh)
# don't need to change.

# `-u` (nounset) is intentionally OFF — the orchestrator expands optional
# arrays like `"${meta_args[@]}"` which can be empty. Bash's strict-mode
# array expansion is fragile here and the workarounds are uglier than the
# loss of `-u` is worth at this scope. Stages keep `set -euo pipefail`.
set -eo pipefail

usage() {
  cat <<'EOF'
Upload a binary to GCS as a versioned artifact + stable pointer, signed
with Ed25519 and published to the transparency log. Orchestrates the
stages in scripts/upload/.

Uploads BINARY to:
  ${DEST}/${NAME}-${VERSION}${EXE}    versioned (immutable, kept for rollback)
  ${DEST}/${NAME}${EXE}               stable "always-latest" pointer (mutable)
  + .sha256 sidecar for each (text/plain)
  + .sig sidecar for each (when signing enabled)
  + transparency-log entry per URI (when signing + log enabled)

Custom GCS metadata is attached to both copies. Auto-included:
  version, sha256, build_time, size_bytes
  os, arch — when --os/--arch (or --platform) is set
Caller-supplied metadata is layered on top via --meta.

Usage:
  scripts/upload-artifact.sh \
    --binary  <path>           file to upload (required)
    --dest    <gs://...>       destination prefix (required, no trailing slash)
                               with --os / --arch / --platform, this is the BASE
                               (e.g. gs://lynxx-public/cli) and /<os>/<arch> is
                               auto-appended; without them, pass the full path
    --name    <str>            artifact name, e.g. lynxx (required)
                               with --os windows, ".exe" is auto-appended to the
                               GCS object names (callers don't need to think
                               about extensions)
    --version <str>            version, e.g. v0.1.0-abc1234 (required)
    --os      <linux|darwin|windows>
                               OS slot in the GCS path. Auto-appended to --dest
                               (unless --dest already ends in /<os>/<arch>),
                               auto-added to metadata as "os=<value>", and
                               drives the .exe suffix on Windows.
    --arch    <amd64|arm64>    arch slot in the GCS path. Same auto-append +
                               auto-metadata as --os.
    --platform <os/arch>       shorthand for --os <os> --arch <arch>
                               (e.g. --platform darwin/arm64)
    --meta    <key=value>      extra metadata (repeatable; values cannot contain commas)
    --no-stable                skip the stable pointer upload (versioned only)
    --signing-key <ref>        Ed25519 PEM private key for release signing.
                               Accepts a filesystem path OR a Secret Manager URI:
                                  gcp-secret://<name>[@<version>][?project=<id>]
                               (default: $VEXARR_SIGNING_KEY env var, else unsigned;
                                recommended in CI: gcp-secret://vexarr-release-signing-key)
    --openssl-bin <path>       openssl binary to use for signing
                               (default: $VEXARR_OPENSSL env var, then PATH;
                                must be OpenSSL 3+, NOT LibreSSL)
    --transparency-log <gs://> public transparency log bucket
                               (default: gs://vexarr-transparency-log,
                                set to "" to skip log publication;
                                only used when signing is enabled)
    --dry-run                  print the gcloud commands instead of running them

Short flags: -f, -d, -n, -v, -m, -p, -h

Examples:
  # Old style — explicit full destination path. Still works.
  scripts/upload-artifact.sh \
    -f target/x86_64-unknown-linux-gnu/release/lynxx \
    -d gs://lynxx-public/cli/linux/amd64 \
    -n lynxx \
    -v v0.1.0-abc1234 \
    -m os=linux -m arch=amd64 -m commit=abc1234

  # New style — pass the base prefix + --platform; everything else is auto.
  scripts/upload-artifact.sh \
    -f target/x86_64-unknown-linux-gnu/release/lynxx \
    -d gs://lynxx-public/cli \
    -p linux/amd64 \
    -n lynxx -v v0.1.0-abc1234 -m commit=abc1234
  # → uploads to gs://lynxx-public/cli/linux/amd64/lynxx{,-v0.1.0-abc1234}

  # Windows amd64. Local file is lynxx.exe; --name stays bare "lynxx" — the
  # script auto-appends .exe to the GCS object names.
  scripts/upload-artifact.sh \
    -f target/x86_64-pc-windows-gnu/release/lynxx.exe \
    -d gs://lynxx-public/cli \
    -p windows/amd64 \
    -n lynxx -v v0.1.0-abc1234
  # → uploads to gs://lynxx-public/cli/windows/amd64/lynxx.exe (stable)
EOF
}

BINARY=""; DEST=""; NAME=""; VERSION=""
OS=""; ARCH=""
declare -a META=()
SKIP_STABLE=0
DRY_RUN=0
SIGNING_KEY="${VEXARR_SIGNING_KEY:-}"
OPENSSL_BIN="${VEXARR_OPENSSL:-}"
TRANSPARENCY_LOG="${VEXARR_TRANSPARENCY_LOG-gs://vexarr-transparency-log}"

while [[ $# -gt 0 ]]; do
  case "$1" in
    -f|--binary)         BINARY="$2"; shift 2 ;;
    -d|--dest)           DEST="$2"; shift 2 ;;
    -n|--name)           NAME="$2"; shift 2 ;;
    -v|--version)        VERSION="$2"; shift 2 ;;
    -m|--meta)           META+=("$2"); shift 2 ;;
    --os)                OS="$2"; shift 2 ;;
    --arch)              ARCH="$2"; shift 2 ;;
    -p|--platform)
      [[ "$2" == */* ]] || { echo "--platform must be os/arch, got: $2" >&2; exit 1; }
      OS="${2%%/*}"; ARCH="${2#*/}"; shift 2 ;;
    --no-stable)         SKIP_STABLE=1; shift ;;
    --signing-key)       SIGNING_KEY="$2"; shift 2 ;;
    --openssl-bin)       OPENSSL_BIN="$2"; shift 2 ;;
    --transparency-log)  TRANSPARENCY_LOG="$2"; shift 2 ;;
    --dry-run)           DRY_RUN=1; shift ;;
    -h|--help)           usage; exit 0 ;;
    *) echo "unknown arg: $1" >&2; usage >&2; exit 1 ;;
  esac
done

[[ -n "$BINARY"  ]] || { echo "missing --binary"  >&2; exit 1; }
[[ -n "$DEST"    ]] || { echo "missing --dest"    >&2; exit 1; }
[[ -n "$NAME"    ]] || { echo "missing --name"    >&2; exit 1; }
[[ -n "$VERSION" ]] || { echo "missing --version" >&2; exit 1; }
[[ -f "$BINARY"  ]] || { echo "binary not found: $BINARY" >&2; exit 1; }
[[ "$DEST" == gs://* ]] || { echo "--dest must start with gs://" >&2; exit 1; }
DEST="${DEST%/}"

command -v gcloud >/dev/null 2>&1 || { echo "gcloud CLI required" >&2; exit 1; }

# ── Platform validation + auto-derivation ────────────────────────────────────
# Same as the monolith: --os and --arch together → validate, auto-append the
# /<os>/<arch> path, auto-inject os/arch metadata, set EXE_SUFFIX on Windows.
EXE_SUFFIX=""
PLATFORM=""
if [[ -n "$OS" || -n "$ARCH" ]]; then
  [[ -n "$OS"   ]] || { echo "--arch requires --os (or use --platform <os>/<arch>)"   >&2; exit 1; }
  [[ -n "$ARCH" ]] || { echo "--os requires --arch (or use --platform <os>/<arch>)" >&2; exit 1; }
  case "$OS"   in linux|darwin|windows) ;; *) echo "--os must be linux|darwin|windows (got: $OS)"   >&2; exit 1 ;; esac
  case "$ARCH" in amd64|arm64)          ;; *) echo "--arch must be amd64|arm64 (got: $ARCH)"        >&2; exit 1 ;; esac
  PLATFORM="$OS/$ARCH"

  if [[ "$DEST" != *"/$OS/$ARCH" ]]; then
    DEST="$DEST/$OS/$ARCH"
  fi

  if [[ "$OS" == "windows" ]]; then
    EXE_SUFFIX=".exe"
    [[ "$BINARY" == *.exe ]] || \
      echo "  WARN: --os windows but --binary doesn't end in .exe ($BINARY) — uploading anyway" >&2
  fi
fi

VERSIONED="$DEST/${NAME}-${VERSION}${EXE_SUFFIX}"
STABLE="$DEST/${NAME}${EXE_SUFFIX}"

# ── Set up the shared context + clean-up trap ────────────────────────────────
STAGES_DIR="$(cd "$(dirname "$0")" && pwd)/upload"
for stage in compute-metadata sign-binary copy-to-gcs publish-transparency-log; do
  [[ -x "$STAGES_DIR/$stage.sh" ]] || {
    echo "missing or non-executable stage: $STAGES_DIR/$stage.sh" >&2
    exit 1
  }
done

CONTEXT=$(mktemp)
# Track temp files created by stages so we can clean them up.
cleanup() {
  local rc=$?
  # Source the context one last time so we know what the stages allocated.
  # shellcheck source=/dev/null
  [[ -f "$CONTEXT" ]] && source "$CONTEXT" 2>/dev/null || true
  for f in "${CHECKSUM_FILE:-}" "${SIGNATURE_FILE:-}"; do
    [[ -n "$f" && -f "$f" ]] && rm -f "$f"
  done
  # Only delete the resolved Secret Manager temp key, not a user-supplied PEM.
  if [[ -n "${SIGNING_KEY_RESOLVED:-}" && "$SIGNING_KEY_RESOLVED" == /tmp/* && "$SIGNING_KEY_RESOLVED" != "$SIGNING_KEY" ]]; then
    rm -f "$SIGNING_KEY_RESOLVED"
  fi
  rm -f "$CONTEXT"
  exit $rc
}
trap cleanup EXIT

# Seed the context with what the orchestrator owns.
{
  printf 'BINARY=%q\n'           "$BINARY"
  printf 'NAME=%q\n'             "$NAME"
  printf 'VERSION=%q\n'          "$VERSION"
  printf 'DEST=%q\n'              "$DEST"
  printf 'OS=%q\n'                "$OS"
  printf 'ARCH=%q\n'              "$ARCH"
  printf 'PLATFORM=%q\n'          "$PLATFORM"
  printf 'EXE_SUFFIX=%q\n'        "$EXE_SUFFIX"
  printf 'VERSIONED=%q\n'         "$VERSIONED"
  printf 'STABLE=%q\n'            "$STABLE"
  printf 'SKIP_STABLE=%q\n'       "$SKIP_STABLE"
  printf 'DRY_RUN=%q\n'           "$DRY_RUN"
  printf 'TRANSPARENCY_LOG=%q\n'  "$TRANSPARENCY_LOG"
} > "$CONTEXT"

echo "==> Artifact"
echo "    binary:   $BINARY"
echo "    name:     $NAME"
echo "    version:  $VERSION"
[[ -n "$PLATFORM" ]] && echo "    platform: $PLATFORM"
echo "    dest:     $DEST"
echo

# ── Stage 1: compute metadata (sha256, size, build_time, checksum sidecar) ───
declare -a meta_args=()
for m in "${META[@]:-}"; do
  [[ -n "$m" ]] && meta_args+=(--meta "$m")
done
declare -a platform_args=()
[[ -n "$PLATFORM" ]] && platform_args=(--platform "$PLATFORM")

"$STAGES_DIR/compute-metadata.sh" \
  --context     "$CONTEXT" \
  --binary      "$BINARY" \
  --name        "$NAME" \
  --version     "$VERSION" \
  --exe-suffix  "$EXE_SUFFIX" \
  "${platform_args[@]}" \
  "${meta_args[@]}"

# Re-source after each stage so we pick up its outputs in this shell.
# shellcheck source=/dev/null
source "$CONTEXT"

# ── Stage 2: sign the binary (Ed25519, no-op when --signing-key empty) ───────
"$STAGES_DIR/sign-binary.sh" \
  --context     "$CONTEXT" \
  --binary      "$BINARY" \
  --signing-key "$SIGNING_KEY" \
  --openssl-bin "$OPENSSL_BIN"
# shellcheck source=/dev/null
source "$CONTEXT"

# ── Stage 3: copy to GCS (binary + .sha256 [+ .sig]) for both URIs ───────────
declare -a dry=()
[[ "$DRY_RUN" -eq 1 ]] && dry=(--dry-run)

copy_artifact_triple() {
  # Uploads:  $1 (binary), $1.sha256, $1.sig (when signed) at the given URI.
  local uri="$1"
  "$STAGES_DIR/copy-to-gcs.sh" \
    --source       "$BINARY" \
    --destination  "$uri" \
    --content-type application/octet-stream \
    --metadata     "$META" \
    "${dry[@]}"
  "$STAGES_DIR/copy-to-gcs.sh" \
    --source       "$CHECKSUM_FILE" \
    --destination  "${uri}.sha256" \
    --content-type text/plain \
    "${dry[@]}"
  if [[ -n "${SIGNATURE_FILE:-}" ]]; then
    "$STAGES_DIR/copy-to-gcs.sh" \
      --source       "$SIGNATURE_FILE" \
      --destination  "${uri}.sig" \
      --content-type application/octet-stream \
      "${dry[@]}"
  fi
}

echo "==> Uploading versioned -> $VERSIONED"
copy_artifact_triple "$VERSIONED"

if [[ "$SKIP_STABLE" -eq 0 ]]; then
  echo "==> Uploading stable    -> $STABLE"
  copy_artifact_triple "$STABLE"
fi

# ── Stage 4: transparency log entry per URI ──────────────────────────────────
"$STAGES_DIR/publish-transparency-log.sh" \
  --context          "$CONTEXT" \
  --artifact-uri     "$VERSIONED" \
  --transparency-log "$TRANSPARENCY_LOG" \
  "${dry[@]}"

if [[ "$SKIP_STABLE" -eq 0 ]]; then
  "$STAGES_DIR/publish-transparency-log.sh" \
    --context          "$CONTEXT" \
    --artifact-uri     "$STABLE" \
    --transparency-log "$TRANSPARENCY_LOG" \
    "${dry[@]}"
fi

echo
echo "Done."
sig_suffix=""
[[ -n "${SIGNATURE_FILE:-}" ]] && sig_suffix=" + .sig"
echo "  versioned: $VERSIONED  (+ .sha256${sig_suffix})"
[[ "$SKIP_STABLE" -eq 0 ]] && echo "  stable:    $STABLE  (+ .sha256${sig_suffix})"
if [[ -n "${SIGNATURE_FILE:-}" && -n "$TRANSPARENCY_LOG" ]]; then
  echo "  logged:    $TRANSPARENCY_LOG/by-sha256/${SHA256:0:12}…/${BUILD_TIME//:/}.json"
fi
if [[ -z "$SIGNING_KEY" ]]; then
  echo "  (unsigned — pass --signing-key or set VEXARR_SIGNING_KEY for Ed25519 signing)"
fi

# Explicit success exit. Without this the script's exit code would be the
# exit of the previous conditional (`[[ … ]] && echo`), which is 1 when the
# condition is false. Tools that wrap this script (release-all-platforms.sh)
# would otherwise see successful uploads as failures.
exit 0
