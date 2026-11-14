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
3. builds the macOS universal `.dmg` and the Windows x64 `-setup.exe` and attaches them to the draft;
4. publishes the release once both are attached.

If a build fails for a transient reason, use **Re-run failed jobs** on the workflow run; it uploads to the same draft. If the release needs a code change, delete the draft release on GitHub and the tag (`git push --delete origin v0.2.0 && git tag -d v0.2.0`), fix, and tag again. A tag that doesn't match the version only needs deleting and re-tagging: nothing was created.
