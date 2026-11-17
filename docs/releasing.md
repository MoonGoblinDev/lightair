# Releasing Lightair

A release is a version tag: GitHub Actions builds, signs and publishes it
([.github/workflows/release.yml](../.github/workflows/release.yml)), and every Lightair that
checks for updates (Settings › About) offers it.

## Once: secrets

1. On the Mac that holds the keys, make the update key (kept in its Keychain; the public half
   goes into `crates/lightair-app/src/updater.rs`, commit that):

       cargo xtask update-key

2. Put this Mac's release secrets into the repository's GitHub secrets (the update key and
   `release.env`'s app ids):

       cargo xtask ci-secrets

3. Add the Apple ones yourself (GitHub › the repository › Settings › Secrets and variables ›
   Actions › New repository secret):

   | Secret | What |
   |---|---|
   | `MACOS_CERTIFICATE_P12` | Keychain Access › your "Developer ID Application" certificate (with its key) › Export as .p12, then `base64 -i cert.p12 \| pbcopy` and paste |
   | `MACOS_CERTIFICATE_PASSWORD` | the password you gave that export |
   | `APPLE_ID` | your Apple ID email |
   | `APPLE_TEAM_ID` | your team ID (developer.apple.com › Membership) |
   | `APPLE_APP_PASSWORD` | an app-specific password (appleid.apple.com › Sign-In and Security) |

## Each release

1. Raise the version in `Cargo.toml` (`[workspace.package] version`), commit, push.
2. Tag it with a one-line summary (the updater shows it) and push the tag:

       git tag -a v0.2.0 -m "Ko-fi without a server, official sign-in, updates"
       git push origin v0.2.0

3. The workflow checks the tag matches the version, builds the notarized macOS disk image and
   the Windows zip, writes and signs `latest.json`, and publishes the release with all of them.
   A run that fails publishes nothing; fix, delete the tag (`git push --delete origin v0.2.0`,
   `git tag -d v0.2.0`) and tag again.

## How updates reach people

Lightair reads `releases/latest/download/latest.json` (and its `.sig`) at launch and every six
hours. It acts only on a manifest the update key signed, downloads the file for its platform,
checks its SHA-256 against the manifest and, on macOS, that the new app's code signature is
valid and from the same team. It then swaps the app after quitting and opens the new one. A
copy in a folder Lightair can't write to gets the disk image opened instead.
