#!/usr/bin/env bash
# Stage 4/4 of the upload pipeline.
#
# Publishes a signed entry to the public transparency log for an artifact
# that's already been uploaded + signed. Closes the "signer was coerced"
# gap: every release leaves a public, audit-trail-able record.
#
# Layout: by-sha256/<sha>/<iso-timestamp>.json{,.sig}
#   * Lookups are O(1) by hash (the gateway's verify path does
#     `gs://…/by-sha256/<sha>/`-prefix LIST and picks the first entry).
#   * Browsable by time within a hash for human review.
#
# Skipped when:
#   * Signing was disabled (no signature → nothing meaningful to log).
#   * --transparency-log "" was passed (test mode / local).
#
# Contract — READS from context:
#   SHA256, BUILD_TIME, SIGNATURE_FILE, SIGNING_KEY_RESOLVED, OPENSSL_BIN
# Does not modify the context.

set -euo pipefail

usage() {
  cat <<'EOF'
publish-transparency-log.sh — stage 4/4 of the upload pipeline.

Publishes a signed transparency-log entry for an already-uploaded artifact.

Usage:
  scripts/upload/publish-transparency-log.sh \
    --context          <file>          shared context (read-only here)
    --artifact-uri     <gs://...>      the artifact URI being logged
                                       (used as `path` field in entry)
    --transparency-log <gs://...>      log bucket (empty = skip)
                                       (default: gs://vexarr-transparency-log)
    --signer           <id>            signer identifier (string, free-form)
                                       (default: vexarr-release-v1)
    --dry-run                          print gcloud commands, don't run
    -h|--help

Skipped silently when the context has no SIGNATURE_FILE (signing was off)
or when --transparency-log is empty.

The script reads from the context (sourceable bash key=value):
  SHA256                   — entry "sha256" field + by-sha256/ prefix
  BUILD_TIME               — entry "timestamp" field
  SIGNATURE_FILE           — base64'd into entry "sig" field
  SIGNING_KEY_RESOLVED     — used to sign the entry itself
  OPENSSL_BIN              — for the entry signing
EOF
}

CONTEXT=""; ARTIFACT_URI=""
TRANSPARENCY_LOG="${VEXARR_TRANSPARENCY_LOG-gs://vexarr-transparency-log}"
SIGNER="vexarr-release-v1"
DRY_RUN=0

while [[ $# -gt 0 ]]; do
  case "$1" in
    --context)            CONTEXT="$2"; shift 2 ;;
    --artifact-uri)       ARTIFACT_URI="$2"; shift 2 ;;
    --transparency-log)   TRANSPARENCY_LOG="$2"; shift 2 ;;
    --signer)             SIGNER="$2"; shift 2 ;;
    --dry-run)            DRY_RUN=1; shift ;;
    -h|--help)            usage; exit 0 ;;
    *) echo "publish-transparency-log: unknown arg: $1" >&2; usage >&2; exit 1 ;;
  esac
done

[[ -n "$CONTEXT"      ]] || { echo "publish-transparency-log: --context required"      >&2; exit 1; }
[[ -n "$ARTIFACT_URI" ]] || { echo "publish-transparency-log: --artifact-uri required" >&2; exit 1; }
[[ -f "$CONTEXT"      ]] || { echo "publish-transparency-log: context not found: $CONTEXT" >&2; exit 1; }

# Load the context. Stages communicate via these keys.
# shellcheck source=/dev/null
source "$CONTEXT"

# Graceful no-ops.
if [[ -z "${SIGNATURE_FILE:-}" ]]; then
  echo "  [tlog]       skipped (no SIGNATURE_FILE in context — signing was disabled)" >&2
  exit 0
fi
if [[ -z "$TRANSPARENCY_LOG" ]]; then
  echo "  [tlog]       skipped (--transparency-log empty)" >&2
  exit 0
fi

[[ -n "${SHA256:-}"               ]] || { echo "publish-transparency-log: context missing SHA256"               >&2; exit 1; }
[[ -n "${BUILD_TIME:-}"           ]] || { echo "publish-transparency-log: context missing BUILD_TIME"           >&2; exit 1; }
[[ -n "${SIGNING_KEY_RESOLVED:-}" ]] || { echo "publish-transparency-log: context missing SIGNING_KEY_RESOLVED" >&2; exit 1; }
[[ -n "${OPENSSL_BIN:-}"          ]] || { echo "publish-transparency-log: context missing OPENSSL_BIN"          >&2; exit 1; }

# Extract bucket + path components from gs://bucket/path/to/object for the
# entry JSON. We log them separately so downstream verification can rebuild
# the artifact URI without parsing.
WITHOUT_SCHEME="${ARTIFACT_URI#gs://}"
LBUCKET="${WITHOUT_SCHEME%%/*}"
LPATH="${WITHOUT_SCHEME#*/}"

# Base64-encode the binary signature for the entry payload (JSON-safe).
SIG_B64=$(base64 < "$SIGNATURE_FILE" | tr -d '\n')

# Canonical JSON layout — fixed key order, no spaces, no trailing newline.
# Bumping this schema invalidates the signatures of all prior log entries;
# treat any change here as a versioning event ("v" field). Keep keys
# alphabetised after "v" so the canonical bytes are stable.
ENTRY_JSON=$(printf '{"v":1,"bucket":"%s","path":"%s","sha256":"%s","sig":"%s","signer":"%s","timestamp":"%s"}' \
  "$LBUCKET" "$LPATH" "$SHA256" "$SIG_B64" "$SIGNER" "$BUILD_TIME")

ENTRY_FILE=$(mktemp)
ENTRY_SIG_FILE=$(mktemp)
trap 'rm -f "$ENTRY_FILE" "$ENTRY_SIG_FILE"' EXIT
printf '%s' "$ENTRY_JSON" > "$ENTRY_FILE"

# Sign the canonical entry bytes with the SAME Ed25519 release key.
# Anyone with the embedded public key can verify the entry was published
# by the legitimate signer (not forged into the log by an attacker who
# only controls the bucket).
if ! "$OPENSSL_BIN" pkeyutl -sign -inkey "$SIGNING_KEY_RESOLVED" -rawin -in "$ENTRY_FILE" -out "$ENTRY_SIG_FILE" 2>/dev/null; then
  echo "publish-transparency-log: failed to sign entry (returning success — main upload is unaffected)" >&2
  exit 0
fi

# Sanitise ISO timestamp for the GCS object name (drop colons — they're URL-
# legal but awkward to copy/paste from terminals).
TS_SAFE="${BUILD_TIME//:/}"
PREFIX="by-sha256/${SHA256}"
ENTRY_URI="${TRANSPARENCY_LOG%/}/${PREFIX}/${TS_SAFE}.json"
ENTRY_SIG_URI="${ENTRY_URI}.sig"

# Delegate the actual upload to copy-to-gcs.sh so all GCS interaction
# flows through one place (consistent dry-run, future retries, etc.).
COPY_SCRIPT="$(cd "$(dirname "$0")" && pwd)/copy-to-gcs.sh"
[[ -x "$COPY_SCRIPT" ]] || { echo "publish-transparency-log: copy-to-gcs.sh missing or non-executable: $COPY_SCRIPT" >&2; exit 1; }

declare -a DRY=()
[[ "$DRY_RUN" -eq 1 ]] && DRY=(--dry-run)

echo "  [tlog]       $ENTRY_URI" >&2

"$COPY_SCRIPT" \
  --source       "$ENTRY_FILE" \
  --destination  "$ENTRY_URI" \
  --content-type application/json \
  "${DRY[@]}"

"$COPY_SCRIPT" \
  --source       "$ENTRY_SIG_FILE" \
  --destination  "$ENTRY_SIG_URI" \
  --content-type application/octet-stream \
  "${DRY[@]}"
