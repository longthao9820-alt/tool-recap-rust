param([string]$OutputDir = "runtime/ffmpeg")
$ErrorActionPreference = "Stop"
$Version = "9.0.1"
$AssetName = "ffmpeg-$Version-essentials_build.zip"
$ReleaseApi = "https://api.github.com/repos/GyanD/codexffmpeg/releases/tags/$Version"
$Headers = @{
  "User-Agent" = "tool-recap-rust-build"
  "Accept" = "application/vnd.github+json"
  "X-GitHub-Api-Version" = "2022-11-28"
}

$Release = Invoke-RestMethod -Uri $ReleaseApi -Headers $Headers
$Asset = $Release.assets | Where-Object { $_.name -eq $AssetName } | Select-Object -First 1
if (-not $Asset) { throw "FFmpeg release $Version does not contain $AssetName" }
if (-not $Asset.digest -or -not $Asset.digest.StartsWith("sha256:")) {
  throw "GitHub Release API did not provide a SHA-256 digest for $AssetName"
}
$Expected = $Asset.digest.Substring(7).ToLowerInvariant()

$Work = Join-Path ([IO.Path]::GetTempPath()) "tool-recap-ffmpeg-$Version"
Remove-Item $Work -Recurse -Force -ErrorAction SilentlyContinue
New-Item $Work -ItemType Directory -Force | Out-Null
$Archive = Join-Path $Work $AssetName

Invoke-WebRequest -Uri $Asset.browser_download_url -Headers $Headers -OutFile $Archive
$Actual = (Get-FileHash $Archive -Algorithm SHA256).Hash.ToLowerInvariant()
if ($Expected -ne $Actual) {
  throw "FFmpeg SHA-256 mismatch. Expected $Expected, got $Actual"
}

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

@(
  "vendor=GyanD/codexffmpeg",
  "version=$Version",
  "asset=$AssetName",
  "sha256=$Actual",
  "verified_from=GitHub Release API asset digest"
) | Set-Content (Join-Path $Target.FullName "VERSION.txt") -Encoding UTF8
