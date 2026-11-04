# Signs the .exe files inside a Windows release zip with the Certum Open Source Code Signing card,
# then repacks the zip in place:
#
#   packaging/sign.ps1 dist/deskmonk-windows-x86_64.zip
#
# shortcut: the key is on a physical card, so this cannot run in CI — build in CI, sign here, then
# upload. Move to a cloud signing service (Certum SimplySign, Azure Trusted Signing) to automate it.
#
# Needs the card plugged in, proCertum CardManager installed, and the certificate in the Windows
# store (certmgr.msc > Personal). With more than one, pick it: $env:SIGN_THUMBPRINT = "<thumbprint>".
param([Parameter(Mandatory)][string]$Zip)

$ErrorActionPreference = "Stop"
Set-Location (Split-Path $PSScriptRoot -Parent)

$signtool = Get-ChildItem "C:\Program Files (x86)\Windows Kits\10\bin" -Recurse -Filter signtool.exe -ErrorAction SilentlyContinue |
  Where-Object { $_.FullName -match '\\x64\\' } | Select-Object -Last 1
if (-not $signtool) { throw "signtool.exe not found: install the Windows SDK." }

$certs = @(Get-ChildItem Cert:\CurrentUser\My -CodeSigningCert)
if ($env:SIGN_THUMBPRINT) { $certs = @($certs | Where-Object { $_.Thumbprint -eq $env:SIGN_THUMBPRINT }) }
if ($certs.Count -ne 1) { throw "need exactly one code signing certificate, found $($certs.Count): set SIGN_THUMBPRINT" }

$work = Join-Path ([IO.Path]::GetTempPath()) "deskmonk-sign"
if (Test-Path $work) { Remove-Item -Recurse -Force $work }
Expand-Archive -Path $Zip -DestinationPath $work
$exes = @(Get-ChildItem "$work\*.exe")
if ($exes.Count -eq 0) { throw "no .exe in $Zip" }

# Timestamped, so the signature stays valid after the certificate expires.
& $signtool.FullName sign /sha1 $certs[0].Thumbprint /fd sha256 /tr http://time.certum.pl /td sha256 $exes.FullName
if ($LASTEXITCODE -ne 0) { throw "signtool sign failed" }
# Proves the chain validates as Windows will see it, not just that a signature was written.
& $signtool.FullName verify /pa /v $exes.FullName
if ($LASTEXITCODE -ne 0) { throw "signtool verify failed" }

Compress-Archive -Force -Path "$work\*" -DestinationPath $Zip
Write-Host "signed $($exes.Count) file(s) as $($certs[0].Subject), repacked $Zip"
