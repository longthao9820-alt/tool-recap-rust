param([string]$Configuration = "release",[string]$OutputDir = "dist/Tool-Recap-Rust")
$ErrorActionPreference = "Stop"
$Root = (Resolve-Path ".").Path
$Out = Join-Path $Root $OutputDir
Remove-Item $Out -Recurse -Force -ErrorAction SilentlyContinue
New-Item $Out -ItemType Directory -Force | Out-Null
$Target = Join-Path $Root "target/$Configuration"
foreach ($Name in @("tool-recap-rust.exe","tool-recap-updater.exe")) {
  $Source = Join-Path $Target $Name
  if (-not (Test-Path $Source)) { throw "Missing $Name" }
  Copy-Item $Source $Out -Force
}
Copy-Item "README.md" $Out -Force
if (Test-Path "LICENSE") { Copy-Item "LICENSE" $Out -Force }
Copy-Item "LICENSE-NOTICE.md" $Out -Force
Copy-Item "THIRD_PARTY_NOTICES.md" $Out -Force
Copy-Item "runtime" $Out -Recurse -Force
New-Item (Join-Path $Out "data") -ItemType Directory -Force | Out-Null
& (Join-Path $Out "tool-recap-rust.exe") --self-check
if ($LASTEXITCODE -ne 0) { throw "Portable self-check failed" }
Write-Host "Portable folder verified at $Out"
