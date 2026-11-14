# Cutting a release

Releases are built by GitHub Actions (`.github/workflows/release.yml`) when a version tag is pushed. The Sendbox version lives in one place: `version` under `[workspace.package]` in the root `Cargo.toml`, and the tag must be `v` followed by it.

1. On an up-to-date `main` with green CI, set the new version in `Cargo.toml`:

   ```toml
   [workspace.package]
   version = "0.2.0"
   ```

2. Refresh `Cargo.lock` (CI builds with `--locked`) and commit:

   ```sh
   cargo check --workspace
   git commit -am "chore(release): v0.2.0"
   ```

3. Tag that commit and push both:

   ```sh
   git tag v0.2.0
   git push origin main v0.2.0
   ```

The Release workflow then:

1. checks that the tag matches the version in `Cargo.toml`, and stops before building anything if it doesn't;
2. creates a draft GitHub Release for the tag, with generated release notes;
3. builds the macOS universal `.dmg` (signed with the Developer ID, notarized and stapled) and the Windows x64 `-setup.exe` and attaches them to the draft;
4. publishes the release once both are attached.

If a build fails for a transient reason, use **Re-run failed jobs** on the workflow run; it uploads to the same draft. If the release needs a code change, delete the draft release on GitHub and the tag (`git push --delete origin v0.2.0 && git tag -d v0.2.0`), fix, and tag again. A tag that doesn't match the version only needs deleting and re-tagging: nothing was created.

## macOS signing secrets

The macOS build signs Sendbox with a Developer ID Application certificate and notarizes it with an App Store Connect API key. It stops before building, naming what's missing, unless these repository secrets are set (**Settings → Secrets and variables → Actions**):

| Secret | Holds |
| --- | --- |
| `APPLE_CERTIFICATE` | The Developer ID Application certificate and its private key, exported as a `.p12`, base64-encoded. |
| `APPLE_CERTIFICATE_PASSWORD` | The password the `.p12` was exported with. |
| `APPLE_SIGNING_IDENTITY` | The certificate's name, e.g. `Developer ID Application: Jane Doe (TEAMID)`. The build fails if it doesn't match the certificate. |
| `APPLE_API_ISSUER` | The App Store Connect issuer ID (**Users and Access → Integrations → App Store Connect API**). |
| `APPLE_API_KEY` | The ID of the App Store Connect API key. |
| `APPLE_API_PRIVATE_KEY` | The full contents of that key's `AuthKey_<key id>.p8` file. |

The workflow writes the `.p8` to a temporary file for the build and deletes it afterwards. `KEYCHAIN_PASSWORD` is not used: Tauri imports the certificate into a throwaway keychain with a random password of its own.

To create or renew them (the certificate expires after five years), the Apple Developer Account Holder runs this from the repository, on a Mac with `gh` logged in:

```sh
./scripts/setup-macos-signing.sh
```

It walks through creating the Developer ID Application certificate and the App Store Connect API key in the browser, packages and checks the certificate, and sets the six secrets with `gh secret set`. Secret values are never printed; working files stay in `~/.sendbox-signing/`.

## Dry run: a signed `.dmg` without a release

To check signing and notarization before tagging, run the Release workflow by hand on a pushed branch:

```sh
gh workflow run release.yml --ref <branch>
gh run watch   # pick the run that just started
```

A manual run builds only the macOS `.dmg`, signs, notarizes and staples it, and keeps it as a workflow artifact. It doesn't check the tag or create a release. Download the `.dmg` from the **Artifacts** section at the bottom of the run's summary page (`gh run view <run-id> --web`). Downloading it with a browser marks it as quarantined, like a colleague's download, so Gatekeeper checks it when it's opened.

## Verify a downloaded `.dmg`

On a Mac, for a `.dmg` from a release or a dry run:

```sh
spctl -a -vvv -t install Sendbox_0.2.0_universal.dmg   # "accepted", "source=Notarized Developer ID"
xcrun stapler validate Sendbox_0.2.0_universal.dmg     # "The validate action worked!"
```

Then open it, drag Sendbox into Applications and launch it: no Gatekeeper dialog should appear.
