# Microsoft Store release

The Store build is the **same binary as the website build** — no feature flag, no second code path:

- **No sandbox.** MSIX runs it as a full-trust desktop app (`Windows.FullTrustApplication`), so nothing is left
  out: the scan, the Vault and App Lock all work as they do in the zip. The opposite of the Mac App Store
  build, which loses App Lock to the sandbox.
- **Pro stays on Paddle.** Store policy lets a non-game app use its own commerce platform, so Pro is the same
  checkout and the same license keys as the website. Read the current wording of policy 10.8.1 before you
  submit: it is the one thing that could force a second payment path, the way StoreKit did on the Mac.
- **The Store signs the package.** That is the only reason to be here — a Store install shows no SmartScreen
  warning. So `packaging/msix.ps1` produces an *unsigned* .msix on purpose, and `packaging/sign.ps1` is not
  used for it.
- **No self-update.** The app has no in-app updater, so there is nothing to switch off. The Store ships updates.
- **x64 only.** ARM64 Windows keeps using the zip from deskmonk.app.

## Build

```powershell
packaging/msix.ps1        # dist/deskMonk-<version>-x64.msix
```

Needs the Windows SDK (`makeappx.exe`). On a tag the `msix` job in CI builds it and leaves it as an artifact;
it is deliberately not attached to the release, because nobody can install an unsigned .msix.

## One-time setup (Partner Center)

1. **Account.** Done: an Individual account, with the App Developer Agreement signed.
2. **Name.** Done: "deskMonk" is reserved as product `9MWKSX54SN2K`
   (partner.microsoft.com/dashboard/products/9MWKSX54SN2K).
3. **Identity.** Product → Product identity gives the two values the manifest needs. Put them in the repo as
   **variables**, not secrets (`.github/workflows/ci.yml` reads `vars.`):
   - `MSIX_IDENTITY_NAME` = `p3l4mx.deskMonk`
   - `MSIX_PUBLISHER` = `CN=5DBE5688-E229-42F5-8AEF-C3B5308BB590` — the identity, not your name
   - `MSIX_PUBLISHER_DISPLAY` = `p3l4mx`

   Set, 2026-10-30. Public listing: https://apps.microsoft.com/detail/9MWKSX54SN2K

   Without them `msix.ps1` still builds, but marks the package a local test build the Store will reject.
4. **Restricted capabilities.** `packaging/AppxManifest.xml` asks for two, and a human reads the justification.
   This is the one step that can sink the whole submission, so answer it carefully. Paste:

   > deskMonk is a disk cleaner, file encrypter and disk mapper. `broadFileSystemAccess` is what the product
   > does: the user picks a folder — or their whole home folder — and the app reads it to report duplicates,
   > app caches, old downloads and large files, then deletes only what the user ticks. The same access moves a
   > file into the encrypted Vault and back out again. A sandboxed per-file picker cannot express "scan my home
   > folder", which is the app's main action. `runFullTrust` is needed for the same reason, and because App Lock
   > closes distracting applications while a focus timer runs.
   >
   > Nothing is read without the user choosing the folder first, nothing is uploaded, and everything except
   > caches and logs is moved to the Recycle Bin rather than deleted, so the user can undo it.
   > Privacy policy: https://deskmonk.app/privacy

5. **Privacy.** Answer to match `docs/privacy.html`: no tracking, no ads, no personal data. After an explicit
   opt-in on first start the app sends anonymous counts, a random install ID, the app version, the OS and the
   hardware list. Privacy policy URL `https://deskmonk.app/privacy`.
6. **Properties.** Category **Utilities & tools**. Support contact `support@deskmonk.app`,
   website `https://deskmonk.app`. Age rating comes from the IARC questionnaire (no ads, no user content,
   no gambling → expect 3+).
7. **Pricing.** Free. Pro is bought in the app through Paddle, so declare the in-app offer but no Store price.
   Check whether the product declarations page asks about encryption export rules; the Vault uses
   ChaCha20-Poly1305 and Argon2id from open-source libraries, and like Apple's question your answer is a legal
   statement, not a form field to guess at.

## Each release

1. Tag, then download the `msix` artifact from the CI run (or run `packaging/msix.ps1` locally).
2. Partner Center → Packages → upload the .msix. It validates on upload and names what is wrong.
3. Fill in the listing. The site copy is already written for this — reuse `docs/index.html`:
   - **Short description:** "Free up space, encrypt private files, focus better. One calm app."
   - **Description:** the "What deskMonk does" section — Clean, Vault, Monk mode, Hardware, Disk map.
   - **Screenshots:** at least one, 1366×768 or larger.
   - **Notes for certification.** Paste:

     > deskMonk is free, with daily limits (2 GB of cleaning, 3 Vault encryptions of up to 200 MB, 1 locked
     > app per day). The Pro subscription removes them and is sold in the app through Paddle (star button, top
     > right); there is no account or login. To review the scan: press Scan, pick any folder, and the results
     > appear in four groups with the space each frees. Nothing is deleted until you tick it and confirm;
     > everything except caches and logs goes to the Recycle Bin.

4. Submit. Certification is usually hours, but a restricted capability means a human looks at it, so allow days.

## Test before you submit

The Store's copy is signed, yours is not, so to install it locally trust your own certificate first:

```powershell
$c = New-SelfSignedCertificate -Type Custom -Subject "CN=deskMonk Local Test, O=deskMonk, C=TR" `
  -KeyUsage DigitalSignature -CertStoreLocation "Cert:\CurrentUser\My" -TextExtension @("2.5.29.37={text}1.3.6.1.5.5.7.3.3")
# The Publisher in the test package must match this Subject exactly, or the install fails.
signtool sign /fd SHA256 /sha1 $c.Thumbprint dist\deskMonk-<version>-x64.msix
Export-Certificate -Cert $c -FilePath test.cer   # then: import it into LocalMachine\TrustedPeople (admin)
Add-AppxPackage dist\deskMonk-<version>-x64.msix
```

Check the Start menu tile and icon, that a scan finds files, that cleaning reaches the Recycle Bin, that the
Vault opens a file it encrypted, and that App Lock can close an app. Settings live in the app's MSIX-redirected
`%LOCALAPPDATA%`, so a first run in the package is a genuinely fresh install.
