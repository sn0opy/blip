#!/usr/bin/env bash
# Builds the Linux release into dist/:
#
# - blip_<version>_amd64.deb for Debian/Ubuntu and derivatives
# - blip-<version>-x86_64-linux.tar.gz with the standalone binary for every other
#   distribution, with the license files (the archive keeps the executable bit)
#
#   ./scripts/package-linux.sh
#
# Needs only Rust: the binary links against glibc alone and loads the X11/Wayland,
# xkbcommon and Vulkan/EGL libraries at runtime. cargo-packager and cargo-about are installed
# on first use.

set -euo pipefail
cd "$(dirname "$0")/.."

if ! command -v cargo-packager >/dev/null; then
    cargo install cargo-packager --version 0.11.8 --locked
fi
if ! command -v cargo-about >/dev/null; then
    cargo install cargo-about --version 0.9.2 --locked --features cli
fi

cargo build --release --locked
cargo about generate about.hbs -o target/THIRD_PARTY_LICENSES.txt
cargo packager --release --formats deb

version=$(cargo pkgid | sed 's/.*[@#]//')
stage=target/linux-tar
rm -rf "$stage"
mkdir -p "$stage"
cp target/release/blip LICENSE target/THIRD_PARTY_LICENSES.txt "$stage"
tar -czf "dist/blip-${version}-x86_64-linux.tar.gz" -C "$stage" .
