$ErrorActionPreference = "Stop"
$Pin = Get-Content "runtime/voicestudio/version.json" -Raw | ConvertFrom-Json
if ($Pin.version -ne "0.5.3" -or $Pin.source_ref -ne "v0.5.3") {
    throw "VoiceStudio runtime pin drifted"
}
if ((Get-Content "Cargo.toml" -Raw) -notmatch 'rust-version\s*=\s*"1\.95"') {
    throw "Rust toolchain metadata drifted"
}
if (Test-Path ".gitmodules") {
    $Submodules = Get-Content ".gitmodules" -Raw
    if ($Submodules -match 'longthao9820-alt/Tool-REcap') {
        throw "The original Tool-REcap repository must remain an external reference, not a submodule."
    }
}
Write-Host "Source contract checks passed"
