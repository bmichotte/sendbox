# Sendbox

A local email-capture tool for developers, for macOS and Windows. While it runs, Sendbox Captures every Email your apps send to `127.0.0.1:1025` over SMTP and keeps it in its Inbox, so you can inspect it. Nothing is ever delivered to anyone. It lives in the menu bar (macOS) or the notification area (Windows).

## Installing Sendbox

Download the installer for your system from the [latest release](https://github.com/bmichotte/sendbox/releases/latest).

### macOS

1. Download `Sendbox_<version>_universal.dmg` (it runs on both Apple Silicon and Intel Macs).
2. Open it and drag **Sendbox** into **Applications**.
3. Open Sendbox from Applications. Its icon appears in the menu bar; there is no Dock icon.

The app is not signed with an Apple Developer ID or notarized yet, so macOS blocks the first launch with "Apple could not verify “Sendbox” is free of malware". To allow it, once:

1. Click **Done** on that dialog.
2. Open **System Settings → Privacy & Security**, scroll to **Security**, and click **Open Anyway** next to "“Sendbox” was blocked".
3. Confirm with your password or Touch ID, then click **Open Anyway** again.

On macOS 14 or earlier, Control-click Sendbox in Applications, choose **Open**, then click **Open** in the dialog instead.

### Windows

1. Download `Sendbox_<version>_x64-setup.exe`. If your browser warns that the file isn't commonly downloaded, choose **Keep** (in Edge: **…** → **Keep** → **Show more** → **Keep anyway**).
2. Run it. The installer is not code-signed, so Microsoft Defender SmartScreen shows "Windows protected your PC". Click **More info**, then **Run anyway**.
3. Follow the installer. It installs for your user only, without administrator rights.

Sendbox starts in the notification area. If you don't see its icon, open the **^** overflow next to the clock; you can drag the icon onto the taskbar to keep it visible.

### Sending Email to Sendbox

Point your app's SMTP settings at host `127.0.0.1`, port `1025`, without TLS. Any sender, recipient and credentials are accepted. For example, with nodemailer:

```js
nodemailer.createTransport({ host: "127.0.0.1", port: 1025 });
```

### Letting an agent read your Emails

Sendbox serves an MCP endpoint at `http://127.0.0.1:1026/mcp` while it runs, so a coding agent can find the Emails your app sent and follow their links (to verify a sign-up, say). With Claude Code:

```sh
claude mcp add --transport http sendbox http://127.0.0.1:1026/mcp
```

The agent can list Emails (`list_emails`, filtered by sender, recipient, subject, time or Unread) and read one (`get_email`: text, HTML, headers, envelope, links and attachment names; never the attachments themselves). Reading an Email marks it read. The endpoint only listens on this machine, and refuses requests that websites open in your browser try to send it (pages served from `localhost` excepted).

To drive a flow end to end, the agent makes your app send an Email (a sign-up, a password reset), then calls `wait_for_email` with the same filters: it gets the oldest matching Unread Email already there, or else the next one your app sends, waiting 30 s by default (up to 120 s) before saying no Email was received. Like reading, waiting marks the Email it returns read, so the next wait gets the next Email.

## Development

Requirements: Rust (stable) and Node 24.

```sh
npm ci
npm run tauri dev     # run the app with the UI hot-reloading
npm run tauri build   # build the installer for this machine
```

The same checks as CI:

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
npm run typecheck
npm test              # the release scripts
```

`crates/sendbox-core` holds all behaviour and its integration tests; `crates/sendbox-app` is the thin Tauri shell (tray, window); the React UI lives in `ui/`.

## Cutting a release

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
