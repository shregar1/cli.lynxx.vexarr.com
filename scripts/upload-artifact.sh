#!/usr/bin/env bash
# Generic GCS artifact uploader.
# See usage() for details.

set -euo pipefail

usage() {
  cat <<'EOF'
Upload a binary to GCS as a versioned artifact + stable pointer.

Uploads BINARY to:
  ${DEST}/${NAME}-${VERSION}     versioned (immutable — keeps history for rollback)
  ${DEST}/${NAME}                stable "always-latest" pointer (mutable)

Custom GCS metadata is attached to both copies. Auto-included:
  version, sha256, build_time, size_bytes
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

  # macOS arm64 (Apple Silicon).
  scripts/upload-artifact.sh \
    -f target/aarch64-apple-darwin/release/lynxx \
    -d gs://lynxx-public/cli \
    -p darwin/arm64 \
    -n lynxx -v v0.1.0-abc1234

  # Windows amd64. Local file is lynxx.exe; --name stays bare "lynxx" — the
  # script auto-appends .exe to the GCS object names.
  scripts/upload-artifact.sh \
    -f target/x86_64-pc-windows-gnu/release/lynxx.exe \
    -d gs://lynxx-public/cli \
    -p windows/amd64 \
    -n lynxx -v v0.1.0-abc1234
  # → uploads to gs://lynxx-public/cli/windows/amd64/lynxx.exe (stable)
  #   and gs://lynxx-public/cli/windows/amd64/lynxx-v0.1.0-abc1234.exe

  # Upload a versioned-only rollback artifact (no stable pointer overwrite).
  scripts/upload-artifact.sh \
    --binary  target/release/lynxx-gateway \
    --dest    gs://lynxx-public/gateway --platform linux/amd64 \
    --name    lynxx-gateway \
    --version v0.1.0-rollback \
    --no-stable
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
      # --platform <os>/<arch> shorthand. Split on the first slash.
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

# --- Platform validation + auto-derivation ----------------------------------
# --os and --arch must be passed together (or neither). When both are set:
#   * Allowed values are validated against the known-supported list — typo
#     protection so a stray "amd_64" doesn't create a phantom GCS prefix.
#   * If --dest already ends in /<os>/<arch>, leave it alone (callers
#     migrating from the old explicit-path style keep working unchanged).
#   * Otherwise append /<os>/<arch> to --dest.
#   * Auto-inject "os=<os>" and "arch=<arch>" into the metadata array
#     (skipped if the caller already passed them — first-wins).
#   * On --os windows, append ".exe" to the GCS artifact name so the stable
#     pointer becomes <name>.exe (matching what Windows installers expect).
#     The local --binary file is also verified to end in .exe (warning only
#     — some build pipelines strip the suffix when copying out of target/).
EXE_SUFFIX=""
if [[ -n "$OS" || -n "$ARCH" ]]; then
  [[ -n "$OS"   ]] || { echo "--arch requires --os (or use --platform <os>/<arch>)"   >&2; exit 1; }
  [[ -n "$ARCH" ]] || { echo "--os requires --arch (or use --platform <os>/<arch>)" >&2; exit 1; }
  case "$OS" in
    linux|darwin|windows) ;;
    *) echo "--os must be one of: linux | darwin | windows (got: $OS)" >&2; exit 1 ;;
  esac
  case "$ARCH" in
    amd64|arm64) ;;
    *) echo "--arch must be one of: amd64 | arm64 (got: $ARCH)" >&2; exit 1 ;;
  esac

  # Auto-append /<os>/<arch> if --dest doesn't already end with it.
  if [[ "$DEST" != *"/$OS/$ARCH" ]]; then
    DEST="$DEST/$OS/$ARCH"
  fi

  # Auto-inject os/arch metadata (first-wins so an explicit --meta os=foo
  # passed by the caller still overrides).
  have_os_meta=0; have_arch_meta=0
  for m in "${META[@]:-}"; do
    [[ "$m" == os=*   ]] && have_os_meta=1
    [[ "$m" == arch=* ]] && have_arch_meta=1
  done
  [[ "$have_os_meta"   -eq 0 ]] && META+=("os=$OS")
  [[ "$have_arch_meta" -eq 0 ]] && META+=("arch=$ARCH")

  if [[ "$OS" == "windows" ]]; then
    EXE_SUFFIX=".exe"
    # Soft check — warn if the local binary doesn't look like a Windows exe.
    [[ "$BINARY" == *.exe ]] || \
      echo "  WARN: --os windows but --binary doesn't end in .exe ($BINARY) — uploading anyway" >&2
  fi
fi

command -v gcloud  >/dev/null 2>&1 || { echo "gcloud CLI required" >&2; exit 1; }
command -v shasum  >/dev/null 2>&1 || { echo "shasum required"     >&2; exit 1; }

SIZE=$(wc -c < "$BINARY" | tr -d ' ')
SHA256=$(shasum -a 256 "$BINARY" | cut -d' ' -f1)
BUILD_TIME=$(date -u +%Y-%m-%dT%H:%M:%SZ)

META_PAIRS=(
  "version=$VERSION"
  "sha256=$SHA256"
  "build_time=$BUILD_TIME"
  "size_bytes=$SIZE"
)
for m in "${META[@]:-}"; do
  [[ -n "$m" ]] || continue
  [[ "$m" == *"="* ]] || { echo "--meta must be key=value, got: $m" >&2; exit 1; }
  case "$m" in
    *,*) echo "--meta values cannot contain commas (gcloud limitation): $m" >&2; exit 1 ;;
  esac
  META_PAIRS+=("$m")
done
META_ARG=$(IFS=,; echo "${META_PAIRS[*]}")

VERSIONED="$DEST/${NAME}-${VERSION}${EXE_SUFFIX}"
STABLE="$DEST/${NAME}${EXE_SUFFIX}"

# Sidecar checksum file. The gateway's /v1/install/binary endpoint reads
# this and returns it as X-Content-SHA256 so install scripts can verify
# integrity client-side. Format: single line "<hex-sha256>  <filename>\n"
# (compatible with `shasum -a 256 -c` for manual verification).
CHECKSUM_FILE=$(mktemp)
SIGNATURE_FILE=""
trap 'rm -f "$CHECKSUM_FILE" "$SIGNATURE_FILE"' EXIT
printf '%s  %s\n' "$SHA256" "${NAME}${EXE_SUFFIX}" > "$CHECKSUM_FILE"

# Optional Ed25519 signature. Activated when --signing-key / $VEXARR_SIGNING_KEY
# is set AND OpenSSL 3+ is available. Produces a <name>.sig sidecar (binary
# format, 64-byte Ed25519 signature over the raw artifact bytes). Older
# OpenSSL builds (LibreSSL on macOS by default, OpenSSL 1.1.x on older Linux)
# do not support Ed25519 — point --openssl-bin at /opt/homebrew/opt/openssl@3
# or similar.
#
# --signing-key accepts either a filesystem path or a gcp-secret:// URI that
# resolves to a Secret Manager payload. The URI form is preferred in CI/CD so
# the private key never lands on disk.

# Resolve --signing-key into a real on-disk PEM path. Materialises Secret
# Manager payloads into a chmod-600 temp file that we wipe on exit.
# Sets globals (RESOLVED_KEY_FILE, SIGNING_KEY_RESOLVED) instead of echoing,
# so `exit` propagates cleanly without command-substitution interception.
RESOLVED_KEY_FILE=""
SIGNING_KEY_RESOLVED=""
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
        echo "gcloud CLI required for gcp-secret:// keys" >&2
        return 1
      }
      local tmp
      tmp=$(mktemp)
      chmod 600 "$tmp"
      local ok=0
      if [[ -n "$project" ]]; then
        gcloud secrets versions access "$version" --secret="$name" --project="$project" > "$tmp" 2>/dev/null && ok=1
      else
        gcloud secrets versions access "$version" --secret="$name" > "$tmp" 2>/dev/null && ok=1
      fi
      if [[ "$ok" -ne 1 ]]; then
        rm -f "$tmp"
        echo "failed to fetch Secret Manager payload: $ref" >&2
        return 1
      fi
      RESOLVED_KEY_FILE="$tmp"
      SIGNING_KEY_RESOLVED="$tmp"
      return 0
      ;;
    *)
      [[ -f "$ref" ]] || {
        echo "signing key not found: $ref" >&2
        return 1
      }
      SIGNING_KEY_RESOLVED="$ref"
      return 0
      ;;
  esac
}

SIG_HEX=""
if [[ -n "$SIGNING_KEY" ]]; then
  resolve_signing_key "$SIGNING_KEY" || exit 1
  SIGNING_KEY="$SIGNING_KEY_RESOLVED"
  if [[ -z "$OPENSSL_BIN" ]]; then
    # Prefer Homebrew OpenSSL on macOS over LibreSSL.
    for candidate in /opt/homebrew/opt/openssl@3/bin/openssl /opt/homebrew/opt/openssl/bin/openssl /usr/local/opt/openssl@3/bin/openssl openssl; do
      if command -v "$candidate" >/dev/null 2>&1 && \
         "$candidate" version 2>/dev/null | grep -q "^OpenSSL"; then
        OPENSSL_BIN="$candidate"
        break
      fi
    done
  fi
  [[ -n "$OPENSSL_BIN" ]] || { echo "no OpenSSL 3+ found (LibreSSL does not support Ed25519)" >&2; exit 1; }

  SIGNATURE_FILE=$(mktemp)
  if ! "$OPENSSL_BIN" pkeyutl -sign -inkey "$SIGNING_KEY" -rawin -in "$BINARY" -out "$SIGNATURE_FILE" 2>/dev/null; then
    echo "signing failed (check that key is Ed25519 and openssl is 3+)" >&2
    exit 1
  fi
  SIG_HEX=$(od -An -v -tx1 "$SIGNATURE_FILE" | tr -d ' \n')
  echo "    signed:   ${SIG_HEX:0:32}… (${#SIG_HEX}/2 bytes)"
fi

echo "==> Artifact"
echo "    binary:   $BINARY"
echo "    size:     $SIZE bytes"
echo "    sha256:   $SHA256"
echo "    version:  $VERSION"
echo "    metadata: $META_ARG"
echo

run() {
  if [[ "$DRY_RUN" -eq 1 ]]; then
    printf '  [dry-run]'
    printf ' %q' "$@"
    echo
  else
    "$@"
  fi
}

echo "==> Uploading versioned -> $VERSIONED"
run gcloud storage cp --custom-metadata="$META_ARG" "$BINARY" "$VERSIONED"
run gcloud storage cp --content-type="text/plain" "$CHECKSUM_FILE" "${VERSIONED}.sha256"
if [[ -n "$SIGNATURE_FILE" ]]; then
  run gcloud storage cp --content-type="application/octet-stream" "$SIGNATURE_FILE" "${VERSIONED}.sig"
fi

if [[ "$SKIP_STABLE" -eq 0 ]]; then
  echo "==> Uploading stable    -> $STABLE"
  run gcloud storage cp --custom-metadata="$META_ARG" "$BINARY" "$STABLE"
  run gcloud storage cp --content-type="text/plain" "$CHECKSUM_FILE" "${STABLE}.sha256"
  if [[ -n "$SIGNATURE_FILE" ]]; then
    run gcloud storage cp --content-type="application/octet-stream" "$SIGNATURE_FILE" "${STABLE}.sig"
  fi
fi

# --- Transparency log publication --------------------------------------------
# After signing the binary, publish a signed log entry to the public
# transparency log bucket. This closes the "signer was coerced" gap: every
# release leaves a public, audit-trail-able record. Install scripts that fetch
# the log entry can detect "the bucket has a signed binary that was never
# logged" (a hallmark of off-the-record substitution).
#
# Entry layout:  by-sha256/<sha256>/<ISO-timestamp>.json{,.sig}
# This keeps lookups O(1) for the gateway (LIST by-sha256/<sha>/) while
# remaining publicly browsable by time within a hash.
#
# Skipped silently when:
#   - signing is disabled (no signature → nothing meaningful to log)
#   - --transparency-log "" was passed
LOG_ENTRY_FILES=()
log_publish() {
  [[ -n "$SIGNATURE_FILE" ]] || return 0
  [[ -n "$TRANSPARENCY_LOG" ]] || return 0

  local dest_path="$1"  # full gs:// URI of the artifact being logged

  # Extract bucket name + relative path from gs://bucket/path/to/object.
  local without_scheme="${dest_path#gs://}"
  local lbucket="${without_scheme%%/*}"
  local lpath="${without_scheme#*/}"

  # Base64 the binary signature for the entry payload.
  local sig_b64
  sig_b64=$(base64 < "$SIGNATURE_FILE" | tr -d '\n')

  # Canonical JSON — fixed key order, no spaces, no trailing newline.
  # CHANGES here invalidate signatures of prior log entries; treat as a
  # versioning event (bump "v"). Keep keys alphabetised after "v".
  local entry_json
  entry_json=$(printf '{"v":1,"bucket":"%s","path":"%s","sha256":"%s","sig":"%s","signer":"vexarr-release-v1","timestamp":"%s"}' \
    "$lbucket" "$lpath" "$SHA256" "$sig_b64" "$BUILD_TIME")

  local entry_file entry_sig_file
  entry_file=$(mktemp)
  entry_sig_file=$(mktemp)
  LOG_ENTRY_FILES+=("$entry_file" "$entry_sig_file")
  printf '%s' "$entry_json" > "$entry_file"

  # Sign the canonical entry bytes with the same Ed25519 release key.
  # Anyone with the embedded public key can later verify the entry was
  # published by the legitimate signer (not forged into the log).
  if ! "$OPENSSL_BIN" pkeyutl -sign -inkey "$SIGNING_KEY" -rawin -in "$entry_file" -out "$entry_sig_file" 2>/dev/null; then
    echo "  WARN: failed to sign transparency log entry — skipping log publication" >&2
    return 0
  fi

  # Sanitise timestamp for use in object names (replace ':' which is fine but
  # awkward in URLs, leave the rest alone).
  local ts_safe="${BUILD_TIME//:/}"
  local prefix="by-sha256/${SHA256}"
  local entry_uri="${TRANSPARENCY_LOG%/}/${prefix}/${ts_safe}.json"
  local entry_sig_uri="${entry_uri}.sig"

  echo "==> Publishing transparency log entry"
  echo "    entry:    $entry_uri"
  run gcloud storage cp --content-type="application/json" "$entry_file" "$entry_uri"
  run gcloud storage cp --content-type="application/octet-stream" "$entry_sig_file" "$entry_sig_uri"
}

# Trap update — clean up log temp files + any Secret Manager-materialised key.
trap 'rm -f "$CHECKSUM_FILE" "$SIGNATURE_FILE" "$RESOLVED_KEY_FILE" "${LOG_ENTRY_FILES[@]}"' EXIT

# Publish entries for both versioned and stable paths so install scripts
# fetching either can find a matching log record.
log_publish "$VERSIONED"
[[ "$SKIP_STABLE" -eq 0 ]] && log_publish "$STABLE"

echo
echo "Done."
sig_suffix=""
[[ -n "$SIGNATURE_FILE" ]] && sig_suffix=" + .sig"
echo "  versioned: $VERSIONED  (+ .sha256${sig_suffix})"
[[ "$SKIP_STABLE" -eq 0 ]] && echo "  stable:    $STABLE  (+ .sha256${sig_suffix})"
if [[ -n "$SIGNATURE_FILE" && -n "$TRANSPARENCY_LOG" ]]; then
  echo "  logged:    $TRANSPARENCY_LOG/by-sha256/${SHA256:0:12}…/${BUILD_TIME//:/}.json"
fi
[[ -z "$SIGNING_KEY" ]] && echo "  (unsigned — pass --signing-key or set VEXARR_SIGNING_KEY for Ed25519 signing)"
# Explicit success exit. Without this the script's exit code is the exit of
# the previous `[[ -z … ]] && echo` expression — which is 1 when signing IS
# enabled (the && short-circuits because SIGNING_KEY is non-empty). Tools
# that wrap this script (e.g. release-all-platforms.sh) would otherwise see
# successful uploads as failures.
exit 0
