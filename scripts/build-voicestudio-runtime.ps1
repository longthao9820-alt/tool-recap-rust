param(
  [Parameter(Mandatory=$true)][string]$SourceDir,
  [string]$OutputDir = "runtime/voicestudio/backend",
  [string]$ExpectedVersion = "0.5.3"
)
$ErrorActionPreference = "Stop"
$SourceDir = (Resolve-Path $SourceDir).Path
$PyProject = Join-Path $SourceDir "pyproject.toml"
$Spec = Join-Path $SourceDir "backend.spec"
if (-not (Test-Path $PyProject) -or -not (Test-Path $Spec)) {
  throw "VoiceStudio checkout is incomplete"
}
$VersionLine = Select-String -Path $PyProject -Pattern '^version\s*=\s*"([^"]+)"' | Select-Object -First 1
if (-not $VersionLine -or $VersionLine.Matches[0].Groups[1].Value -ne $ExpectedVersion) {
  throw "VoiceStudio checkout is not v$ExpectedVersion"
}

Push-Location $SourceDir
try {
  uv sync --frozen --no-dev
  uv run pyinstaller backend.spec --noconfirm --clean
} finally {
  Pop-Location
}

$Built = Join-Path $SourceDir "dist/omnivoice-backend"
$BuiltExe = Join-Path $Built "omnivoice-backend.exe"
if (-not (Test-Path $BuiltExe)) {
  throw "VoiceStudio backend was not produced"
}

Remove-Item $OutputDir -Recurse -Force -ErrorAction SilentlyContinue
New-Item $OutputDir -ItemType Directory -Force | Out-Null
Copy-Item (Join-Path $Built "*") $OutputDir -Recurse -Force
Write-Host "VoiceStudio v$ExpectedVersion frozen runtime built at $OutputDir"
