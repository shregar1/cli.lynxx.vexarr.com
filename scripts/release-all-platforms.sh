#!/usr/bin/env bash
# Build + upload a Rust binary for every supported platform.
#
# Wraps `cargo zigbuild` (cross-compilation via zig) + `upload-artifact.sh`
# (signed GCS upload with transparency log) in one pass. Lives next to
# upload-artifact.sh in each repo's scripts/ directory and is byte-identical
# across repos — see the "Identical copy synced" line in upload-artifact.sh.
#
# Default platform matrix (override with repeated --platform):
#   linux/amd64    (x86_64-unknown-linux-gnu)
#   linux/arm64    (aarch64-unknown-linux-gnu)
#   darwin/amd64   (x86_64-apple-darwin)
#   darwin/arm64   (aarch64-apple-darwin)
#   windows/amd64  (x86_64-pc-windows-gnu)

set -euo pipefail

usage() {
  cat <<'EOF'
Build a Rust binary for every supported platform and upload each to GCS.

Usage:
  scripts/release-all-platforms.sh \
    --dest <gs://...>          GCS destination prefix base. /<os>/<arch> is
                               auto-appended per platform. (required)
    --name <str>               binary name + GCS artifact name.
                               (default: auto-derived from Cargo.toml [[bin]])
    --version <str>            version tag, e.g. v0.1.0-abc1234.
                               (default: v$cargo_version-$git_short_sha)
    --platform <os/arch>       restrict to specific platforms (repeatable).
                               If not given, all 5 in the default matrix run.
                               Allowed: linux/amd64, linux/arm64,
                                        darwin/amd64, darwin/arm64,
                                        windows/amd64
    --no-build                 skip cargo build; just upload binaries that
                               already exist in target/<rust-target>/release/.
    --meta <key=value>         extra metadata for every upload (repeatable;
                               forwarded to upload-artifact.sh).
    --signing-key <ref>        Ed25519 signing key (same semantics as
                               upload-artifact.sh).
                               (default: gcp-secret://vexarr-release-signing-key)
    --openssl-bin <path>       OpenSSL 3+ binary for signing.
                               (default: /opt/homebrew/opt/openssl@3/bin/openssl)
    --dry-run                  forward --dry-run to upload-artifact.sh.
                               Build step still runs unless --no-build is set.
    --keep-going               on per-platform build/upload failure, log and
                               continue to the next platform instead of
                               aborting. Final exit code is 1 if any failed.
    -h|--help                  this help.

Examples:
  # Full release — all 5 platforms, signed, transparency-logged.
  cd cli.lynxx.vexarr.com
  scripts/release-all-platforms.sh --dest gs://lynxx-public/cli

  # Only the platforms you actually have customers on.
  scripts/release-all-platforms.sh \
    --dest gs://lynxx-public/cli \
    --platform linux/amd64 \
    --platform darwin/arm64 \
    --platform windows/amd64

  # Re-upload pre-built binaries without rebuilding (e.g. a partial-failure
  # recovery after a network blip on a single upload).
  scripts/release-all-platforms.sh \
    --dest gs://lynxx-public/cli --no-build

  # Dry-run the gateway release (no GCS writes, no transparency log entries).
  cd backend.lynxx.vexarr.com
  scripts/release-all-platforms.sh \
    --dest gs://lynxx-public/gateway \
    --name lynxx-gateway --dry-run
EOF
}

# os/arch → rust target triple. Single source of truth so callers never
# need to remember which triple maps to which slot.
target_for_platform() {
  case "$1" in
    linux/amd64)    echo "x86_64-unknown-linux-gnu" ;;
    linux/arm64)    echo "aarch64-unknown-linux-gnu" ;;
    darwin/amd64)   echo "x86_64-apple-darwin" ;;
    darwin/arm64)   echo "aarch64-apple-darwin" ;;
    windows/amd64)  echo "x86_64-pc-windows-gnu" ;;
    *)              return 1 ;;
  esac
}

DEST=""
NAME=""
VERSION=""
declare -a PLATFORMS=()
declare -a META=()
NO_BUILD=0
DRY_RUN=0
KEEP_GOING=0
SIGNING_KEY="${VEXARR_SIGNING_KEY:-gcp-secret://vexarr-release-signing-key}"
OPENSSL_BIN="${VEXARR_OPENSSL:-/opt/homebrew/opt/openssl@3/bin/openssl}"

while [[ $# -gt 0 ]]; do
  case "$1" in
    -d|--dest)         DEST="$2"; shift 2 ;;
    -n|--name)         NAME="$2"; shift 2 ;;
    -v|--version)      VERSION="$2"; shift 2 ;;
    -p|--platform)     PLATFORMS+=("$2"); shift 2 ;;
    -m|--meta)         META+=("$2"); shift 2 ;;
    --no-build)        NO_BUILD=1; shift ;;
    --dry-run)         DRY_RUN=1; shift ;;
    --keep-going)      KEEP_GOING=1; shift ;;
    --signing-key)     SIGNING_KEY="$2"; shift 2 ;;
    --openssl-bin)     OPENSSL_BIN="$2"; shift 2 ;;
    -h|--help)         usage; exit 0 ;;
    *) echo "unknown arg: $1" >&2; usage >&2; exit 1 ;;
  esac
done

[[ -n "$DEST" ]] || { echo "missing --dest" >&2; exit 1; }
[[ -f "Cargo.toml" ]] || { echo "must run from a Rust crate root (Cargo.toml not found)" >&2; exit 1; }

# Auto-derive --name from Cargo.toml [[bin]] name = "..." if not passed.
# Picks the FIRST [[bin]] entry — repos with multiple binaries should pass
# --name explicitly.
if [[ -z "$NAME" ]]; then
  NAME=$(awk '
    /^\[\[bin\]\]/ { in_bin=1; next }
    /^\[/         { in_bin=0 }
    in_bin && /^name[[:space:]]*=/ {
      sub(/^name[[:space:]]*=[[:space:]]*/, "")
      gsub(/"/, "")
      print
      exit
    }
  ' Cargo.toml)
  [[ -n "$NAME" ]] || { echo "couldn't derive --name from Cargo.toml; pass --name explicitly" >&2; exit 1; }
  echo "  (auto-derived --name from Cargo.toml: $NAME)"
fi

# Auto-derive --version as v<cargo-version>-<git-short-sha>.
if [[ -z "$VERSION" ]]; then
  CARGO_VER=$(grep -E '^version[[:space:]]*=' Cargo.toml | head -1 | sed 's/.*"\(.*\)".*/\1/')
  GIT_SHA=$(git rev-parse --short=8 HEAD 2>/dev/null || echo "nogit")
  VERSION="v${CARGO_VER}-${GIT_SHA}"
  echo "  (auto-derived --version: $VERSION)"
fi

# Default to all 5 platforms when none specified.
if [[ ${#PLATFORMS[@]} -eq 0 ]]; then
  PLATFORMS=(
    linux/amd64
    linux/arm64
    darwin/amd64
    darwin/arm64
    windows/amd64
  )
fi

# Validate every requested platform upfront so a typo doesn't waste a
# 15-minute build before we discover it.
for p in "${PLATFORMS[@]}"; do
  target_for_platform "$p" >/dev/null 2>&1 || {
    echo "unknown --platform: $p (allowed: linux/{amd64,arm64}, darwin/{amd64,arm64}, windows/amd64)" >&2
    exit 1
  }
done

# Toolchain probes.
if [[ "$NO_BUILD" -eq 0 ]]; then
  command -v cargo >/dev/null 2>&1 || { echo "cargo CLI required" >&2; exit 1; }
  command -v cargo-zigbuild >/dev/null 2>&1 || {
    echo "cargo-zigbuild required for cross-compilation: cargo install cargo-zigbuild" >&2
    exit 1
  }
fi

# upload-artifact.sh lives next to this script.
UPLOAD_SCRIPT="$(cd "$(dirname "$0")" && pwd)/upload-artifact.sh"
[[ -x "$UPLOAD_SCRIPT" ]] || { echo "upload-artifact.sh not found or not executable: $UPLOAD_SCRIPT" >&2; exit 1; }

# Build the shared upload arg list (signing + meta + dry-run flags).
UPLOAD_BASE_ARGS=(
  --dest          "$DEST"
  --name          "$NAME"
  --version       "$VERSION"
  --signing-key   "$SIGNING_KEY"
  --openssl-bin   "$OPENSSL_BIN"
)
for m in "${META[@]:-}"; do
  [[ -n "$m" ]] && UPLOAD_BASE_ARGS+=(--meta "$m")
done
[[ "$DRY_RUN" -eq 1 ]] && UPLOAD_BASE_ARGS+=(--dry-run)

declare -a FAILED=()

echo "=================================================================="
echo "release matrix"
echo "  name:     $NAME"
echo "  version:  $VERSION"
echo "  dest:     $DEST/<os>/<arch>"
echo "  platforms: ${PLATFORMS[*]}"
[[ "$NO_BUILD" -eq 1 ]] && echo "  no-build: skipping cargo build, expecting existing target/<…>/release/$NAME"
[[ "$DRY_RUN"  -eq 1 ]] && echo "  dry-run:  no GCS writes; transparency log skipped"
echo "=================================================================="

for platform in "${PLATFORMS[@]}"; do
  target=$(target_for_platform "$platform")
  ext=""
  [[ "$platform" == windows/* ]] && ext=".exe"
  bin_path="target/${target}/release/${NAME}${ext}"

  echo
  echo "─── $platform ($target) ───────────────────────────────────────────"

  if [[ "$NO_BUILD" -eq 0 ]]; then
    echo "==> cargo zigbuild --release --target $target --bin $NAME"
    if ! cargo zigbuild --release --target "$target" --bin "$NAME"; then
      echo "  BUILD FAILED for $platform" >&2
      FAILED+=("$platform (build)")
      [[ "$KEEP_GOING" -eq 1 ]] && continue || exit 1
    fi
  fi

  if [[ ! -f "$bin_path" ]]; then
    echo "  BINARY MISSING: $bin_path" >&2
    FAILED+=("$platform (missing-binary)")
    [[ "$KEEP_GOING" -eq 1 ]] && continue || exit 1
  fi

  echo "==> uploading $bin_path"
  if ! bash "$UPLOAD_SCRIPT" \
        --binary    "$bin_path" \
        --platform  "$platform" \
        "${UPLOAD_BASE_ARGS[@]}"; then
    echo "  UPLOAD FAILED for $platform" >&2
    FAILED+=("$platform (upload)")
    [[ "$KEEP_GOING" -eq 1 ]] && continue || exit 1
  fi
done

echo
echo "=================================================================="
if [[ ${#FAILED[@]} -eq 0 ]]; then
  echo "✅ all ${#PLATFORMS[@]} platforms shipped: $NAME $VERSION"
  echo "   stable pointers:"
  for platform in "${PLATFORMS[@]}"; do
    ext=""; [[ "$platform" == windows/* ]] && ext=".exe"
    echo "     ${DEST}/${platform}/${NAME}${ext}"
  done
  exit 0
else
  echo "⚠️  some platforms failed:"
  for f in "${FAILED[@]}"; do echo "     - $f"; done
  exit 1
fi
