param(
  [string]$BackendDir = "runtime/voicestudio/backend",
  [string]$FfmpegDir = "runtime/ffmpeg/bin",
  [int]$Port = 3909
)
$ErrorActionPreference = "Stop"

$BackendDir = (Resolve-Path $BackendDir).Path
$Exe = Join-Path $BackendDir "omnivoice-backend.exe"
if (-not (Test-Path $Exe)) { throw "VoiceStudio runtime is missing $Exe" }

$FfmpegDir = (Resolve-Path $FfmpegDir).Path
$Ffmpeg = Join-Path $FfmpegDir "ffmpeg.exe"
$Ffprobe = Join-Path $FfmpegDir "ffprobe.exe"
if (-not (Test-Path $Ffmpeg) -or -not (Test-Path $Ffprobe)) {
  throw "Bundled FFmpeg/FFprobe must exist before VoiceStudio smoke"
}

$SmokeRoot = Join-Path $env:RUNNER_TEMP "tool-recap-voicestudio-smoke"
Remove-Item $SmokeRoot -Recurse -Force -ErrorAction SilentlyContinue
New-Item $SmokeRoot -ItemType Directory -Force | Out-Null
$Stdout = Join-Path $SmokeRoot "backend.out.log"
$Stderr = Join-Path $SmokeRoot "backend.err.log"

$env:OMNIVOICE_PORT = "$Port"
$env:OMNIVOICE_DATA_DIR = Join-Path $SmokeRoot "data"
$env:OMNIVOICE_DISABLE_ANALYTICS = "1"
$env:OMNIVOICE_DISABLE_FILE_LOG = "1"
$env:OMNIVOICE_PRELOAD_CAPTURE_ASR = "0"
$env:OMNIVOICE_PRELOAD_WATERMARK = "0"
$env:OMNIVOICE_STARTUP_WATCHDOG_S = "240"
$env:HF_HOME = Join-Path $SmokeRoot "hf"
$env:HF_HUB_CACHE = Join-Path $env:HF_HOME "hub"
$env:HF_HUB_DISABLE_SYMLINKS = "1"
$env:HF_HUB_DISABLE_SYMLINKS_WARNING = "1"
$env:TORCHDYNAMO_DISABLE = "1"
$env:FOR_DISABLE_CONSOLE_CTRL_HANDLER = "1"
$env:FFMPEG_PATH = $Ffmpeg
$env:OMNIVOICE_FFPROBE_PATH = $Ffprobe
$env:PATH = "$FfmpegDir;$env:PATH"

function Show-Diagnostics {
  param([string]$Reason)
  Write-Host "::group::VoiceStudio smoke diagnostics"
  Write-Host $Reason
  if (Test-Path $Stdout) {
    Write-Host "--- stdout tail ---"
    Get-Content $Stdout -Tail 120
  }
  if (Test-Path $Stderr) {
    Write-Host "--- stderr tail ---"
    Get-Content $Stderr -Tail 200
  }
  Write-Host "::endgroup::"
}

# VoiceStudio v0.5.3 explicitly exposes this frozen-backend release smoke mode.
& $Exe --health-check
if ($LASTEXITCODE -ne 0) {
  throw "VoiceStudio built-in --health-check failed with exit code $LASTEXITCODE"
}

# Launch normal server mode, matching the Tool Recap runtime seam.
$Process = Start-Process -FilePath $Exe -WorkingDirectory $BackendDir -PassThru -WindowStyle Hidden -RedirectStandardOutput $Stdout -RedirectStandardError $Stderr
try {
  $Ready = $false
  for ($i=0; $i -lt 300; $i++) {
    if ($Process.HasExited) {
      Show-Diagnostics "VoiceStudio exited during normal-server smoke with code $($Process.ExitCode)"
      throw "VoiceStudio backend exited during normal-server smoke with code $($Process.ExitCode)"
    }
    try {
      $Info = Invoke-RestMethod "http://127.0.0.1:$Port/system/info" -TimeoutSec 2
      if ($Info) {
        $Ready = $true
        break
      }
    } catch {
      if (($i % 10) -eq 0) {
        try {
          $Progress = Invoke-RestMethod "http://127.0.0.1:$Port/startup/progress" -TimeoutSec 2
          Write-Host "VoiceStudio startup: $($Progress.step) / ready=$($Progress.ready)"
        } catch {}
      }
      Start-Sleep -Seconds 1
    }
  }

  if (-not $Ready) {
    Show-Diagnostics "VoiceStudio /system/info did not become ready"
    throw "VoiceStudio backend did not become ready within 300 seconds"
  }

  $OpenApi = Invoke-RestMethod "http://127.0.0.1:$Port/openapi.json" -TimeoutSec 20
  foreach ($Path in @(
    "/v1/audio/speech",
    "/v1/audio/transcriptions",
    "/v1/audio/voices",
    "/models/install",
    "/models/install/status",
    "/engines/select"
  )) {
    if ($OpenApi.paths.PSObject.Properties.Name -notcontains $Path) {
      throw "VoiceStudio API contract missing $Path"
    }
  }
  Write-Host "VoiceStudio v0.5.3 runtime passed health, normal-server, and API compatibility smoke"
} finally {
  if ($Process -and -not $Process.HasExited) {
    taskkill /PID $Process.Id /T /F | Out-Null
  }
}
