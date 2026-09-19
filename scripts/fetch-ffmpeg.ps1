param([string]$OutputDir = "runtime/ffmpeg")
$ErrorActionPreference = "Stop"
$Version = "9.0.1"
$Asset = "ffmpeg-$Version-essentials_build.zip"
$Base = "https://github.com/GyanD/codexffmpeg/releases/download/$Version"
$Work = Join-Path ([IO.Path]::GetTempPath()) "tool-recap-ffmpeg-$Version"
Remove-Item $Work -Recurse -Force -ErrorAction SilentlyContinue
New-Item $Work -ItemType Directory -Force | Out-Null
$Archive = Join-Path $Work $Asset
$ChecksumFile = "$Archive.sha256"
Invoke-WebRequest "$Base/$Asset" -OutFile $Archive
Invoke-WebRequest "$Base/$Asset.sha256" -OutFile $ChecksumFile
$Expected = ((Get-Content $ChecksumFile -Raw).Trim() -split '\s+')[0].ToLowerInvariant()
$Actual = (Get-FileHash $Archive -Algorithm SHA256).Hash.ToLowerInvariant()
if ($Expected -ne $Actual) { throw "FFmpeg SHA-256 mismatch" }
$Expanded = Join-Path $Work "expanded"
Expand-Archive -LiteralPath $Archive -DestinationPath $Expanded -Force
$Root = Get-ChildItem $Expanded -Directory | Select-Object -First 1
if (-not $Root) { throw "FFmpeg archive did not contain a root directory" }
$Target = New-Item $OutputDir -ItemType Directory -Force
$Bin = Join-Path $Target.FullName "bin"
New-Item $Bin -ItemType Directory -Force | Out-Null
foreach ($Name in @("ffmpeg.exe","ffprobe.exe","ffplay.exe")) {
  $Source = Join-Path $Root.FullName "bin/$Name"
  if (-not (Test-Path $Source)) { throw "FFmpeg archive is missing $Name" }
  Copy-Item $Source (Join-Path $Bin $Name) -Force
}
$License = Get-ChildItem $Root.FullName -File | Where-Object { $_.Name -match '^LICENSE(\.txt)?$' } | Select-Object -First 1
if (-not $License) { throw "FFmpeg archive is missing its license" }
Copy-Item $License.FullName (Join-Path $Target.FullName "LICENSE.txt") -Force
@("vendor=GyanD/codexffmpeg","version=$Version","asset=$Asset","sha256=$Actual") |
  Set-Content (Join-Path $Target.FullName "VERSION.txt") -Encoding UTF8
