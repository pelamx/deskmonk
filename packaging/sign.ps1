# Signs the .exe files inside the Windows release archives, then repacks each one in place:
#
#   packaging/sign.ps1 dist/deskMonk-0.5.12-windows-x86_64.zip dist/deskmonk-windows-x86_64.tar.gz
#
# Both archives carry the same programs and both must be signed: the .zip is what the website hands a
# visitor, the .tar.gz is what an installed copy updates itself from. Signing only the first leaves
# every self-updated copy running unsigned binaries, where SmartScreen starts again from nothing.
#
# Run it BEFORE packaging/publish-update.sh: that records the sha256 of these bytes, and signing
# changes them. Signed afterwards, every update downloads a file whose hash no longer matches and is
# thrown away without a word.
#
# shortcut: the key is on a physical card, so this cannot run in CI — build in CI, sign here, then
# publish. Azure Trusted Signing would move it into the workflow and need no card.
#
# Needs the certificate in the Windows store (certmgr.msc > Personal) with its private key available:
# a card plugged in with proCertum CardManager, or a cloud signing provider's CSP installed.
# With more than one certificate, pick it: $env:SIGN_THUMBPRINT = "<thumbprint>".
param([Parameter(Mandatory)][string[]]$Archives)

$ErrorActionPreference = "Stop"
Set-Location (Split-Path $PSScriptRoot -Parent)

$signtool = Get-ChildItem "C:\Program Files (x86)\Windows Kits\10\bin" -Recurse -Filter signtool.exe -ErrorAction SilentlyContinue |
  Where-Object { $_.FullName -match '\\x64\\' } | Select-Object -Last 1
if (-not $signtool) { throw "signtool.exe not found: install the Windows SDK." }

$certs = @(Get-ChildItem Cert:\CurrentUser\My -CodeSigningCert)
if ($env:SIGN_THUMBPRINT) { $certs = @($certs | Where-Object { $_.Thumbprint -eq $env:SIGN_THUMBPRINT }) }
if ($certs.Count -ne 1) { throw "need exactly one code signing certificate, found $($certs.Count): set SIGN_THUMBPRINT" }
# A self-signed certificate signs happily and warns users exactly as much as no signature at all; the
# MSIX identity certificate is one of those, and sits in the same store.
if ($certs[0].Subject -eq $certs[0].Issuer) { throw "$($certs[0].Thumbprint) is self-signed (likely the MSIX identity certificate); Windows trusts it no more than an unsigned file" }

foreach ($archive in $Archives) {
  if (-not (Test-Path -LiteralPath $archive)) { throw "no file $archive" }
  $full = (Resolve-Path -LiteralPath $archive).Path
  $work = Join-Path ([IO.Path]::GetTempPath()) ("deskmonk-sign-" + [IO.Path]::GetRandomFileName())
  New-Item -ItemType Directory -Force $work | Out-Null
  $tar = $full.EndsWith(".tar.gz")

  if ($tar) { & tar.exe -xzf $full -C $work; if ($LASTEXITCODE -ne 0) { throw "could not unpack $archive" } }
  else { Expand-Archive -LiteralPath $full -DestinationPath $work }

  $exes = @(Get-ChildItem $work -Recurse -Filter *.exe)
  if ($exes.Count -eq 0) { throw "no .exe in $archive" }

  # Timestamped, so the signature stays valid after the certificate expires.
  & $signtool.FullName sign /sha1 $certs[0].Thumbprint /fd sha256 /tr http://time.certum.pl /td sha256 $exes.FullName
  if ($LASTEXITCODE -ne 0) { throw "signtool sign failed for $archive" }
  # Proves the chain validates as Windows will see it, not just that a signature was written.
  & $signtool.FullName verify /pa /v $exes.FullName
  if ($LASTEXITCODE -ne 0) { throw "signtool verify failed for $archive" }

  # Repacked the way it came: the website's zip stays a zip, the update archive stays tar + gzip, which
  # is the only thing src/update.rs can unpack.
  Remove-Item -LiteralPath $full -Force
  if ($tar) {
    Push-Location $work
    & tar.exe -czf $full *
    Pop-Location
    if ($LASTEXITCODE -ne 0) { throw "could not repack $archive" }
  } else {
    Compress-Archive -Path "$work\*" -DestinationPath $full
  }
  Remove-Item -Recurse -Force $work
  Write-Host "signed $($exes.Count) file(s) in $archive as $($certs[0].Subject)"
}
