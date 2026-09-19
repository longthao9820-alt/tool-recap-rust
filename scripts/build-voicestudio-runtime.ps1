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

# VoiceStudio v0.5.3 assumes PyInstaller strip=True is a no-op on Windows.
# On GitHub's windows-2022 image GNU strip is available and PyInstaller
# actually strips PE DLLs, including python311.dll, producing a bundle that
# fails at LoadLibrary with "Invalid access to memory location."
# Patch only the disposable checkout used for this Windows build; upstream
# remains untouched and the pinned source/version contract is preserved.
if ($IsWindows) {
  $SpecText = Get-Content $Spec -Raw
  $Matches = [regex]::Matches($SpecText, '(?m)^\s*strip=True,').Count
  if ($Matches -ne 2) {
    throw "VoiceStudio v$ExpectedVersion backend.spec strip contract changed (expected 2 strip=True entries, found $Matches)"
  }
  $SpecText = [regex]::Replace($SpecText, '(?m)^(\s*)strip=True,', '$1strip=False,')

  $OptimizeMatches = [regex]::Matches($SpecText, '(?m)^\s*optimize=2,').Count
  if ($OptimizeMatches -ne 1) {
    throw "VoiceStudio v$ExpectedVersion backend.spec optimize contract changed (expected 1 optimize=2 entry, found $OptimizeMatches)"
  }
  # -OO removes docstrings. NumPy's frozen startup calls add_docstring() while
  # importing its C API and requires those strings; on Windows this otherwise
  # fails in pyi_rth_numpy_compat before VoiceStudio can start.
  $SpecText = [regex]::Replace($SpecText, '(?m)^(\s*)optimize=2,', '$1optimize=1,')

  Set-Content -Path $Spec -Value $SpecText -Encoding UTF8
  Write-Host "Windows compatibility override: disabled binary stripping and preserved runtime docstrings"
}
$VersionLine = Select-String -Path $PyProject -Pattern '^version\s*=\s*"([^"]+)"' | Select-Object -First 1
if (-not $VersionLine -or $VersionLine.Matches[0].Groups[1].Value -ne $ExpectedVersion) {
  throw "VoiceStudio checkout is not v$ExpectedVersion"
}

Push-Location $SourceDir
try {
  uv sync --frozen --no-dev

  if ($IsWindows) {
    # VoiceStudio upstream selects CUDA 12.8 PyTorch wheels on Windows.
    # Tool Recap requires RTX for video rendering, not VoiceStudio inference.
    # Replace only the pinned torch trio with official CPU wheels to avoid
    # shipping several gigabytes of duplicate CUDA runtime.
    $env:UV_NO_CONFIG = "1"
    uv pip uninstall --python .venv torch torchaudio torchvision
    uv pip install --python .venv --index-url https://download.pytorch.org/whl/cpu "torch==2.8.0+cpu" "torchaudio==2.8.0+cpu" "torchvision==0.23.0+cpu"
    if ($LASTEXITCODE -ne 0) {
      throw "Failed to install the pinned CPU PyTorch runtime"
    }
    & ".venv\Scripts\python.exe" -c "import torch, torchaudio, torchvision; assert not torch.cuda.is_available(); print('Tool Recap VoiceStudio torch runtime:', torch.__version__)"
    if ($LASTEXITCODE -ne 0) {
      throw "Pinned VoiceStudio CPU torch runtime validation failed"
    }
    Remove-Item Env:UV_NO_CONFIG -ErrorAction SilentlyContinue
  }

  & ".venv\Scripts\python.exe" -m PyInstaller backend.spec --noconfirm --clean
  if ($LASTEXITCODE -ne 0) {
    throw "PyInstaller failed"
  }
} finally {
  Remove-Item Env:UV_NO_CONFIG -ErrorAction SilentlyContinue
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
