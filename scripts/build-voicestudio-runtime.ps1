param(
  [Parameter(Mandatory=$true)][string]$SourceDir,
  [string]$OutputDir = "runtime/voicestudio/backend",
  [string]$ExpectedVersion = "0.5.3"
)
$ErrorActionPreference = "Stop"
$SourceDir = (Resolve-Path $SourceDir).Path
$PyProject = Join-Path $SourceDir "pyproject.toml"
$Spec = Join-Path $SourceDir "backend.spec"
if (-not (Test-Path $PyProject) -or -not (Test-Path $Spec)) { throw "VoiceStudio checkout is incomplete" }
$VersionLine = Select-String -Path $PyProject -Pattern '^version\s*=\s*"([^"]+)"' | Select-Object -First 1
if (-not $VersionLine -or $VersionLine.Matches[0].Groups[1].Value -ne $ExpectedVersion) { throw "VoiceStudio checkout is not v$ExpectedVersion" }
Push-Location $SourceDir
try {
  uv sync --frozen --no-dev
  uv run pyinstaller backend.spec --noconfirm --clean
} finally { Pop-Location }
$Built = Join-Path $SourceDir "dist/omnivoice-backend"
if (-not (Test-Path (Join-Path $Built "omnivoice-backend.exe"))) { throw "VoiceStudio backend was not produced" }
Remove-Item $OutputDir -Recurse -Force -ErrorAction SilentlyContinue
New-Item $OutputDir -ItemType Directory -Force | Out-Null
Copy-Item (Join-Path $Built "*") $OutputDir -Recurse -Force

$Port = 3909
$env:OMNIVOICE_PORT = "$Port"
$env:OMNIVOICE_DISABLE_ANALYTICS = "1"
$env:OMNIVOICE_DISABLE_FILE_LOG = "1"
$env:HF_HOME = Join-Path $env:RUNNER_TEMP "voicestudio-smoke-hf"
$Process = Start-Process -FilePath (Join-Path (Resolve-Path $OutputDir) "omnivoice-backend.exe") -PassThru -WindowStyle Hidden
try {
  $Ready = $false
  for ($i=0; $i -lt 180; $i++) {
    if ($Process.HasExited) { throw "VoiceStudio backend exited during smoke" }
    try {
      $Info = Invoke-RestMethod "http://127.0.0.1:$Port/system/info" -TimeoutSec 2
      if ($Info) { $Ready = $true; break }
    } catch { Start-Sleep -Seconds 1 }
  }
  if (-not $Ready) { throw "VoiceStudio backend did not become ready" }
  $OpenApi = Invoke-RestMethod "http://127.0.0.1:$Port/openapi.json" -TimeoutSec 10
  foreach ($Path in @("/v1/audio/speech","/v1/audio/transcriptions","/v1/audio/voices","/models/install","/models/install/status","/engines/select")) {
    if ($OpenApi.paths.PSObject.Properties.Name -notcontains $Path) { throw "VoiceStudio API contract missing $Path" }
  }
} finally {
  if ($Process -and -not $Process.HasExited) { Stop-Process -Id $Process.Id -Force -ErrorAction SilentlyContinue }
}
Write-Host "VoiceStudio v$ExpectedVersion runtime passed API compatibility smoke"
