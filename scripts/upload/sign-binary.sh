#!/usr/bin/env bash
# Stage 2/4 of the upload pipeline.
#
# Resolves an Ed25519 signing key (filesystem path OR Secret Manager URI),
# signs the binary with OpenSSL 3+ in raw mode, and writes a .sig sidecar.
# No-op + exit 0 when --signing-key is empty so callers can pipeline this
# stage unconditionally and let it skip itself for unsigned dev releases.
#
# Contract — appends to the shared context file:
#   SIGNATURE_FILE         path to the binary .sig blob (64-byte Ed25519)
#   SIG_HEX                full hex of the signature (for logging / debugging)
#   SIGNING_KEY_RESOLVED   on-disk path to the PEM (real file OR mktemp from
#                          Secret Manager); downstream stages (e.g.
#                          publish-transparency-log) reuse it without
#                          re-resolving from Secret Manager.
#   OPENSSL_BIN            resolved openssl 3+ binary (the autodiscovered one
#                          on macOS, or the explicit override)
#
# When --signing-key is empty the context is untouched and the script exits 0.

set -euo pipefail

usage() {
  cat <<'EOF'
sign-binary.sh — stage 2/4 of the upload pipeline.

Signs a binary with an Ed25519 key (filesystem path or Secret Manager URI).
No-op when --signing-key is empty.

Usage:
  scripts/upload/sign-binary.sh \
    --context     <file>       shared context (appended to)
    --binary      <path>       file to sign (required)
    --signing-key <ref>        Ed25519 PEM. Accepts a filesystem path OR:
                                 gcp-secret://<name>[@<version>][?project=<id>]
                               Empty string → no-op.
    --openssl-bin <path>       openssl 3+ binary (LibreSSL doesn't support
                               Ed25519). If empty, auto-discovers via the
                               common Homebrew paths on macOS.
    -h|--help

On success (when key was provided), context gains:
  SIGNATURE_FILE=<path>
  SIG_HEX=<hex>
  SIGNING_KEY_RESOLVED=<path>
  OPENSSL_BIN=<path>
EOF
}

CONTEXT=""; BINARY=""; SIGNING_KEY=""; OPENSSL_BIN=""

while [[ $# -gt 0 ]]; do
  case "$1" in
    --context)     CONTEXT="$2"; shift 2 ;;
    --binary)      BINARY="$2"; shift 2 ;;
    --signing-key) SIGNING_KEY="$2"; shift 2 ;;
    --openssl-bin) OPENSSL_BIN="$2"; shift 2 ;;
    -h|--help)     usage; exit 0 ;;
    *) echo "sign-binary: unknown arg: $1" >&2; usage >&2; exit 1 ;;
  esac
done

[[ -n "$CONTEXT" ]] || { echo "sign-binary: --context required" >&2; exit 1; }
[[ -n "$BINARY"  ]] || { echo "sign-binary: --binary required"  >&2; exit 1; }
[[ -f "$BINARY"  ]] || { echo "sign-binary: binary not found: $BINARY" >&2; exit 1; }

# Empty key → graceful no-op. Lets the orchestrator call this stage
# unconditionally; unsigned dev releases just skip the work.
if [[ -z "$SIGNING_KEY" ]]; then
  echo "  [sign]       skipped (no --signing-key)" >&2
  exit 0
fi

# Resolve `gcp-secret://<name>[@<version>][?project=<id>]` to an on-disk PEM.
# Filesystem paths pass through unchanged. The Secret Manager temp file is
# chmod 600 and is left in /tmp for downstream stages to reuse (they need
# the same key to sign the transparency log entry); the orchestrator cleans
# it up via trap.
resolve_signing_key() {
  local ref="$1"
  case "$ref" in
    gcp-secret://*)
      local rest="${ref#gcp-secret://}"
      local name="$rest" version="latest" project=""
      if [[ "$rest" == *\?* ]]; then
        local query="${rest#*\?}"
        name="${rest%%\?*}"
        case "$query" in project=*) project="${query#project=}" ;; esac
      fi
      if [[ "$name" == *@* ]]; then
        version="${name#*@}"
        name="${name%@*}"
      fi
      command -v gcloud >/dev/null 2>&1 || {
        echo "sign-binary: gcloud CLI required for gcp-secret:// keys" >&2
        return 1
      }
      local tmp
      tmp=$(mktemp)
      chmod 600 "$tmp"
      if [[ -n "$project" ]]; then
        gcloud secrets versions access "$version" --secret="$name" --project="$project" > "$tmp" 2>/dev/null || {
          rm -f "$tmp"
          echo "sign-binary: failed to fetch Secret Manager payload: $ref" >&2
          return 1
        }
      else
        gcloud secrets versions access "$version" --secret="$name" > "$tmp" 2>/dev/null || {
          rm -f "$tmp"
          echo "sign-binary: failed to fetch Secret Manager payload: $ref" >&2
          return 1
        }
      fi
      printf '%s' "$tmp"
      ;;
    *)
      [[ -f "$ref" ]] || {
        echo "sign-binary: key file not found: $ref" >&2
        return 1
      }
      printf '%s' "$ref"
      ;;
  esac
}

SIGNING_KEY_RESOLVED=$(resolve_signing_key "$SIGNING_KEY") || exit 1

# Auto-discover OpenSSL 3+ if not explicit. LibreSSL (default on macOS)
# does NOT support Ed25519 — we must skip it.
if [[ -z "$OPENSSL_BIN" ]]; then
  for candidate in \
      /opt/homebrew/opt/openssl@3/bin/openssl \
      /opt/homebrew/opt/openssl/bin/openssl \
      /usr/local/opt/openssl@3/bin/openssl \
      openssl
  do
    if command -v "$candidate" >/dev/null 2>&1 && \
       "$candidate" version 2>/dev/null | grep -q "^OpenSSL"; then
      OPENSSL_BIN="$candidate"
      break
    fi
  done
fi
[[ -n "$OPENSSL_BIN" ]] || {
  echo "sign-binary: no OpenSSL 3+ found (LibreSSL does not support Ed25519)" >&2
  exit 1
}

SIGNATURE_FILE=$(mktemp)
if ! "$OPENSSL_BIN" pkeyutl -sign -inkey "$SIGNING_KEY_RESOLVED" -rawin -in "$BINARY" -out "$SIGNATURE_FILE" 2>/dev/null; then
  rm -f "$SIGNATURE_FILE"
  echo "sign-binary: signing failed (key must be Ed25519, openssl must be 3+)" >&2
  exit 1
fi

SIG_HEX=$(od -An -v -tx1 "$SIGNATURE_FILE" | tr -d ' \n')

{
  printf 'SIGNATURE_FILE=%q\n'       "$SIGNATURE_FILE"
  printf 'SIG_HEX=%q\n'              "$SIG_HEX"
  printf 'SIGNING_KEY_RESOLVED=%q\n' "$SIGNING_KEY_RESOLVED"
  printf 'OPENSSL_BIN=%q\n'          "$OPENSSL_BIN"
} >> "$CONTEXT"

echo "  [sign]       ${SIG_HEX:0:32}…  (${#SIG_HEX}/2 bytes, openssl=$OPENSSL_BIN)" >&2
