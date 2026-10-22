#!/usr/bin/env bash
# Builds the macOS app bundle and disk image (blip-<version>-macos-universal.dmg) into dist/ as a universal binary
# (Apple Silicon + Intel):
#
#   ./scripts/package-macos.sh
#
# Needs Rust and the Xcode command line tools (for lipo). cargo-packager and
# cargo-about are installed on first use.

set -euo pipefail
cd "$(dirname "$0")/.."

if ! command -v cargo-packager >/dev/null; then
    cargo install cargo-packager --version 0.11.8 --locked
fi
if ! command -v cargo-about >/dev/null; then
    cargo install cargo-about --version 0.9.2 --locked --features cli
fi

targets=(aarch64-apple-darwin x86_64-apple-darwin)
rustup target add "${targets[@]}"
for target in "${targets[@]}"; do
    cargo build --release --target "$target"
done

universal=target/universal-apple-darwin/release
mkdir -p "$universal"
lipo -create -output "$universal/blip" \
    "target/aarch64-apple-darwin/release/blip" \
    "target/x86_64-apple-darwin/release/blip"

cargo about generate about.hbs -o target/THIRD_PARTY_LICENSES.txt

cargo packager --release --target universal-apple-darwin --formats app,dmg

version=$(cargo pkgid | sed 's/.*[@#]//')
# cargo-packager names the disk image blip_<version>_<arch>.dmg.
mv -f "dist/blip_${version}_universal.dmg" "dist/blip-${version}-macos-universal.dmg"
