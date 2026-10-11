# Builds the Windows installer: dist\blip_<version>_x64-setup.exe
#
#   .\scripts\package-windows.ps1
#
# Needs Rust with the MSVC toolchain. cargo-packager and cargo-about are installed on first
# use; cargo-packager downloads NSIS by itself.

$ErrorActionPreference = "Stop"
Set-Location (Join-Path $PSScriptRoot "..")

if (-not (Get-Command cargo-packager -ErrorAction SilentlyContinue)) {
    cargo install cargo-packager --version 0.11.8 --locked
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
}
if (-not (Get-Command cargo-about -ErrorAction SilentlyContinue)) {
    cargo install cargo-about --version 0.9.2 --locked --features cli
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
}

cargo build --release
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

cargo about generate about.hbs -o target/THIRD_PARTY_LICENSES.txt
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

cargo packager --release --formats nsis
exit $LASTEXITCODE
