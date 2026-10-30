# MSIX package for the Microsoft Store: dist/deskMonk-<version>-x64.msix
#
# Store identity comes from Partner Center, so it cannot live in the repo. Pass it in:
#   $env:MSIX_IDENTITY_NAME    = "12345pelamx.deskMonk"            # Package/Identity/Name
#   $env:MSIX_PUBLISHER        = "CN=ABCD1234-..., O=..., C=..."   # Package/Identity/Publisher
#   $env:MSIX_PUBLISHER_DISPLAY= "pelamx"
# Without them the script still builds, but marks the package as a local test build that the
# Store will refuse — better than quietly producing something that looks submittable.
#
# The Store signs the package itself, so this produces an unsigned .msix. To install it locally,
# sign it with your own certificate and trust that certificate first:
#   signtool sign /fd SHA256 /a /f test.pfx /p <pw> dist\deskMonk-<version>-x64.msix

$ErrorActionPreference = "Stop"
Set-Location (Split-Path $PSScriptRoot -Parent)

$version = (Select-String -Path Cargo.toml -Pattern '^version = "(.*)"' | Select-Object -First 1).Matches[0].Groups[1].Value
# Store versions are four parts and the last must be 0; it reserves the revision for itself.
$msixVersion = "$version.0"

$name = $env:MSIX_IDENTITY_NAME
$publisher = $env:MSIX_PUBLISHER
$publisherDisplay = $env:MSIX_PUBLISHER_DISPLAY
if (-not $name -or -not $publisher) {
  Write-Warning "MSIX_IDENTITY_NAME / MSIX_PUBLISHER are not set: building a LOCAL TEST package."
  Write-Warning "The Store will reject it. Take the real values from Partner Center > Product identity."
  if (-not $name) { $name = "deskMonk.LocalTest" }
  if (-not $publisher) { $publisher = "CN=deskMonk Local Test, O=deskMonk, C=TR" }
}
# Must match Partner Center's PublisherDisplayName exactly, or the upload is rejected.
if (-not $publisherDisplay) { $publisherDisplay = "p3l4mx" }

$makeappx = Get-ChildItem "C:\Program Files (x86)\Windows Kits\10\bin" -Recurse -Filter makeappx.exe -ErrorAction SilentlyContinue |
  Where-Object { $_.FullName -match '\\(x64|arm64)\\' } | Select-Object -Last 1
if (-not $makeappx) { throw "makeappx.exe not found: install the Windows SDK." }

cargo build --release --bin deskmonk --bin deskmonk-tui
if ($LASTEXITCODE -ne 0) { throw "cargo build failed" }

$work = "dist\msix"
if (Test-Path $work) { Remove-Item -Recurse -Force $work }
New-Item -ItemType Directory -Force -Path "$work\Assets" | Out-Null

Copy-Item target\release\deskmonk.exe, target\release\deskmonk-tui.exe, README.md $work

# Every tile size from the one 1024 px icon. Scale-200 variants are what a high-DPI Start menu
# actually shows; without them Windows upscales the 100% asset and it looks soft.
Add-Type -AssemblyName System.Drawing
# icon-1024.png, not icon.png: the same framing, but 1024 px against 256. The largest tile is
# Square150x150Logo.scale-200 at 300 px, which the 256 px source could only upscale into something
# soft. Not dock-icon.png, which is the same drawing inside a wide macOS dock margin.
$icon = [System.Drawing.Image]::FromFile((Resolve-Path assets\icon-1024.png))
$tiles = @{ "Square44x44Logo" = 44; "Square71x71Logo" = 71; "Square150x150Logo" = 150; "StoreLogo" = 50 }
foreach ($tile in $tiles.GetEnumerator()) {
  foreach ($scale in 100, 200) {
    $px = [int]($tile.Value * $scale / 100)
    $bmp = New-Object System.Drawing.Bitmap $px, $px
    $g = [System.Drawing.Graphics]::FromImage($bmp)
    $g.InterpolationMode = "HighQualityBicubic"
    $g.DrawImage($icon, 0, 0, $px, $px)
    # scale-100 keeps the plain name as well: the manifest refers to the asset without a qualifier.
    $suffix = if ($scale -eq 100) { "" } else { ".scale-$scale" }
    $bmp.Save((Join-Path (Resolve-Path "$work\Assets") "$($tile.Key)$suffix.png"), [System.Drawing.Imaging.ImageFormat]::Png)
    $g.Dispose(); $bmp.Dispose()
  }
}
$icon.Dispose()

(Get-Content packaging\AppxManifest.xml -Raw).
  Replace("@NAME@", $name).
  Replace("@PUBLISHER@", $publisher).
  Replace("@PUBLISHER_DISPLAY@", $publisherDisplay).
  Replace("@VERSION@", $msixVersion) | Set-Content "$work\AppxManifest.xml" -Encoding UTF8

$out = "dist\deskMonk-$version-x64.msix"
if (Test-Path $out) { Remove-Item -Force $out }
& $makeappx.FullName pack /d $work /p $out /o
if ($LASTEXITCODE -ne 0) { throw "makeappx failed" }

Write-Host "built $out ($([math]::Round((Get-Item $out).Length / 1MB, 2)) MB), identity $name $msixVersion"
