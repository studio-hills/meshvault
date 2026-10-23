# Packages a release build of Meshvault as a portable zip, with SHA256SUMS.txt and the
# release notes for its version taken from CHANGELOG.md.
#
#   cargo build --release -p meshvault --locked
#   cargo about generate -c packaging/about.toml --locked --fail -o target/THIRD_PARTY_LICENSES.html packaging/about.hbs
#   powershell -File packaging/package.ps1 [-OutDir target/dist] [-Release]
#
# -Release refuses a version whose CHANGELOG heading is still "Unreleased". Run it from
# the repository root. Works in Windows PowerShell 5.1 and PowerShell 7.
param(
	[string]$OutDir = "target/dist",
	[string]$TargetDir = "target",
	[switch]$Release
)
$ErrorActionPreference = "Stop"

# the version, from the app's Cargo.toml
$manifest = Get-Content "crates/meshvault/Cargo.toml" -Raw
if ($manifest -notmatch '(?m)^version\s*=\s*"([^"]+)"') { throw "no version in crates/meshvault/Cargo.toml" }
$version = $Matches[1]

# the CHANGELOG section for it, as the release notes
$changelog = Get-Content "CHANGELOG.md"
$start = -1
for ($i = 0; $i -lt $changelog.Count; $i++) {
	if ($changelog[$i] -match "^## \[$([regex]::Escape($version))\] - (.+)$") { $start = $i; $date = $Matches[1]; break }
}
if ($start -lt 0) { throw "CHANGELOG.md has no section for $version" }
if ($Release -and $date -match "Unreleased") { throw "CHANGELOG.md still lists ${version} as Unreleased; give it the release date" }
$end = $changelog.Count
for ($i = $start + 1; $i -lt $changelog.Count; $i++) {
	if ($changelog[$i] -match '^## \[' -or $changelog[$i] -match '^\[[^\]]+\]: ') { $end = $i; break }
}
$notes = ($changelog[($start + 1)..($end - 1)] -join "`n").Trim()

$exe = Join-Path $TargetDir "release/meshvault.exe"
$licences = Join-Path $TargetDir "THIRD_PARTY_LICENSES.html"
foreach ($f in @($exe, $licences)) { if (-not (Test-Path $f)) { throw "missing $f (see the header of this script)" } }
$exeVersion = (Get-Item $exe).VersionInfo.ProductVersion
if ($exeVersion -ne $version) { throw "meshvault.exe is version $exeVersion, Cargo.toml says ${version}: rebuild it" }

$name = "Meshvault-$version-windows-x64"
New-Item -ItemType Directory -Force $OutDir | Out-Null
$stage = Join-Path $OutDir $name
if (Test-Path $stage) { Remove-Item -Recurse -Force $stage }
New-Item -ItemType Directory -Force (Join-Path $stage "licenses") | Out-Null

Copy-Item $exe $stage
Copy-Item $licences $stage
foreach ($f in @("README.md", "LICENSE", "CHANGELOG.md", "THIRD_PARTY_NOTICES.md")) { Copy-Item $f $stage }
Copy-Item "crates/meshvault/assets/fonts/cal-sans/OFL.txt" (Join-Path $stage "licenses/Cal-Sans-OFL.txt")
Copy-Item "crates/meshvault/assets/fonts/phosphor/LICENSE.txt" (Join-Path $stage "licenses/Phosphor-LICENSE.txt")
Copy-Item "crates/rbx_mesh/LICENSE-MIT" (Join-Path $stage "licenses/rbx_mesh-LICENSE-MIT.txt")
Copy-Item "crates/rbx_mesh/LICENSE-APACHE" (Join-Path $stage "licenses/rbx_mesh-LICENSE-APACHE.txt")

$zip = Join-Path $OutDir "$name.zip"
if (Test-Path $zip) { Remove-Item -Force $zip }
Compress-Archive -Path $stage -DestinationPath $zip
$hash = (Get-FileHash -Algorithm SHA256 $zip).Hash.ToLower()
Set-Content -Path (Join-Path $OutDir "SHA256SUMS.txt") -Value "$hash  $name.zip" -Encoding ascii -NoNewline

$sums = "``````text`n$hash  $name.zip`n``````"
$body = "$notes`n`n## Download`n`nUnzip **$name.zip** anywhere and run ``meshvault.exe``. Windows 10 or 11, 64-bit.`n`nThe executable is not code-signed yet, so Windows SmartScreen may warn on first run: choose **More info**, then **Run anyway**.`n`nSHA-256:`n`n$sums`n"
Set-Content -Path (Join-Path $OutDir "release-notes.md") -Value $body -Encoding utf8

"version $version"
"zip     $zip ($([math]::Round((Get-Item $zip).Length / 1MB, 1)) MB)"
"sha256  $hash"
