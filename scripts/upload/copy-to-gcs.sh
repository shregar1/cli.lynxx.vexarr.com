#!/usr/bin/env bash
# Stage 3/4 of the upload pipeline.
#
# Thin wrapper around `gcloud storage cp` that:
#   * pipes through --dry-run when set (prints the command instead)
#   * attaches caller-supplied custom-metadata
#   * lets the caller set a non-default content-type (we use application/octet-
#     stream for binaries, text/plain for .sha256, application/json for log
#     entries, application/octet-stream for .sig blobs)
#
# Stateless re: context — doesn't read from or write to the context file.
# This is intentional; orchestrators call it many times (binary, .sha256,
# .sig, versioned URI, stable URI, etc.) and each call is independent.
#
# Existing as its own stage gives you:
#   * one place to add retries / parallelism / progress reporting later
#   * a single point to test against a fake gs:// (e.g. fake-gcs-server)
#   * cleaner separation between "decide WHAT to upload" (orchestrator) and
#     "actually upload it" (this stage)

set -euo pipefail

usage() {
  cat <<'EOF'
copy-to-gcs.sh — stage 3/4 of the upload pipeline.

A thin wrapper around `gcloud storage cp` with --dry-run support and
typed content-type handling.

Usage:
  scripts/upload/copy-to-gcs.sh \
    --source       <local-path>          file to upload (required)
    --destination  <gs://bucket/path>    target URI (required)
    --content-type <mime>                e.g. application/octet-stream,
                                         text/plain, application/json
                                         (default: gcloud's auto-detection)
    --metadata     <comma,joined,kvs>    --custom-metadata for gcloud
                                         (optional; values cannot contain ',')
    --dry-run                            print the gcloud command, don't run
    -h|--help

Exits 0 on success or in dry-run mode. Exits non-zero on gcloud failure.
EOF
}

SOURCE=""; DESTINATION=""; CONTENT_TYPE=""; METADATA=""; DRY_RUN=0

while [[ $# -gt 0 ]]; do
  case "$1" in
    --source)        SOURCE="$2"; shift 2 ;;
    --destination)   DESTINATION="$2"; shift 2 ;;
    --content-type)  CONTENT_TYPE="$2"; shift 2 ;;
    --metadata)      METADATA="$2"; shift 2 ;;
    --dry-run)       DRY_RUN=1; shift ;;
    -h|--help)       usage; exit 0 ;;
    *) echo "copy-to-gcs: unknown arg: $1" >&2; usage >&2; exit 1 ;;
  esac
done

[[ -n "$SOURCE"      ]] || { echo "copy-to-gcs: --source required"      >&2; exit 1; }
[[ -n "$DESTINATION" ]] || { echo "copy-to-gcs: --destination required" >&2; exit 1; }
[[ -f "$SOURCE"      ]] || { echo "copy-to-gcs: source not found: $SOURCE" >&2; exit 1; }
[[ "$DESTINATION" == gs://* ]] || { echo "copy-to-gcs: --destination must start with gs://" >&2; exit 1; }

# Build the gcloud arg list. Order: gcloud storage cp [opts] SOURCE DEST.
declare -a CMD=(gcloud storage cp)
[[ -n "$CONTENT_TYPE" ]] && CMD+=("--content-type=$CONTENT_TYPE")
[[ -n "$METADATA"     ]] && CMD+=("--custom-metadata=$METADATA")
CMD+=("$SOURCE" "$DESTINATION")

if [[ "$DRY_RUN" -eq 1 ]]; then
  printf '  [dry-run]'
  printf ' %q' "${CMD[@]}"
  echo
  exit 0
fi

"${CMD[@]}"
