#!/usr/bin/env bash
# Builds the Linux release for the current architecture (x86_64 or aarch64) into dist/:
#
# - blip-<version>-linux-<arch>.flatpak, a single-file Flatpak bundle (sh.sascha.blip)
# - blip-<version>-linux-<arch>.tar.gz with the standalone binary and the license files
#   (the archive keeps the executable bit)
#
#   ./scripts/package-linux.sh
#
# Needs Rust plus flatpak and flatpak-builder with the Flathub remote, for the
# org.freedesktop.Platform//24.08 runtime and SDK. cargo-about is installed on first use.
# The binary links against glibc alone and loads the X11/Wayland, xkbcommon and Vulkan/EGL
# libraries at runtime; the Flatpak runtime provides them.

set -euo pipefail
cd "$(dirname "$0")/.."

if ! command -v cargo-about >/dev/null; then
    cargo install cargo-about --version 0.9.2 --locked --features cli
fi

cargo build --release --locked
cargo about generate about.hbs -o target/THIRD_PARTY_LICENSES.txt

version=$(cargo pkgid | sed 's/.*[@#]//')
arch=$(uname -m)
mkdir -p dist

flatpak remote-add --user --if-not-exists flathub https://dl.flathub.org/repo/flathub.flatpakrepo
rm -rf target/flatpak-repo target/flatpak-build
flatpak-builder --user --force-clean --install-deps-from=flathub \
    --repo=target/flatpak-repo target/flatpak-build packaging/flatpak/sh.sascha.blip.yml
flatpak build-bundle --arch="$arch" target/flatpak-repo "dist/blip-${version}-linux-${arch}.flatpak" sh.sascha.blip

stage=target/linux-tar
rm -rf "$stage"
mkdir -p "$stage"
cp target/release/blip LICENSE target/THIRD_PARTY_LICENSES.txt "$stage"
tar -czf "dist/blip-${version}-linux-${arch}.tar.gz" -C "$stage" .
