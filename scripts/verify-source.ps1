$ErrorActionPreference = "Stop"
$Pin = Get-Content "runtime/voicestudio/version.json" -Raw | ConvertFrom-Json
if ($Pin.version -ne "0.5.3" -or $Pin.source_ref -ne "v0.5.3") { throw "VoiceStudio runtime pin drifted" }
if ((Get-Content "Cargo.toml" -Raw) -notmatch 'rust-version\s*=\s*"1\.95"') { throw "Rust toolchain metadata drifted" }
$Bad = Get-ChildItem src -Recurse -File | Select-String -Pattern 'Tool-REcap' -SimpleMatch
if ($Bad) { throw "Original repository code/name leaked into new source tree: $($Bad.Path -join ', ')" }
Write-Host "Source contract checks passed"
