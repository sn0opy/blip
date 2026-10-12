# Builds the Windows packages into dist\:
#
# - blip-<version>-windows-x64-setup.exe, the installer
# - blip-<version>-windows-x64.zip with just blip.exe and the license files, no setup
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
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

$version = (cargo pkgid) -replace '.*[@#]', ''
# cargo-packager names the installer blip_<version>_x64-setup.exe.
Move-Item "dist\*-setup.exe" "dist\blip-$version-windows-x64-setup.exe" -Force
$stage = "target\windows-portable"
Remove-Item -Recurse -Force $stage -ErrorAction SilentlyContinue
New-Item -ItemType Directory $stage | Out-Null
Copy-Item target\release\blip.exe, LICENSE, target\THIRD_PARTY_LICENSES.txt $stage
Compress-Archive -Path "$stage\*" -DestinationPath "dist\blip-$version-windows-x64.zip" -Force
