#!/usr/bin/env bash
set -euo pipefail

BIN_NAME="lynxx"
VERSION=$(grep '^version' Cargo.toml | head -1 | cut -d'"' -f2)
DIST="dist"

TARGETS=(
    "x86_64-unknown-linux-gnu:linux:amd64"
    "aarch64-unknown-linux-gnu:linux:arm64"
    "x86_64-apple-darwin:darwin:amd64"
    "aarch64-apple-darwin:darwin:arm64"
    "x86_64-pc-windows-gnu:windows:amd64"
)

command -v cross >/dev/null 2>&1 || { echo "error: 'cross' not found — install with: cargo install cross"; exit 1; }

rm -rf "$DIST"
mkdir -p "$DIST"

for entry in "${TARGETS[@]}"; do
    IFS=: read -r target os arch <<< "$entry"

    echo "==> building ${os}/${arch} (${target})"
    cross build --release --target "$target" --bin "$BIN_NAME"

    ext=""
    [[ "$os" == "windows" ]] && ext=".exe"

    src="target/${target}/release/${BIN_NAME}${ext}"
    artifact="${BIN_NAME}-v${VERSION}-${os}-${arch}${ext}"
    dest="${DIST}/${artifact}"

    cp "$src" "$dest"

    echo "==> uploading ${artifact}"
    cargo run --bin upload -- \
        --file "$dest" \
        --key "releases/v${VERSION}/${artifact}"

    cargo run --bin upload -- \
        --file "$dest" \
        --key "releases/latest/${BIN_NAME}-latest-${os}-${arch}${ext}"
done

echo "==> released v${VERSION}"
