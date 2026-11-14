<p align="center">
  <img src="crates/sendbox-app/icons/128x128@2x.png" width="112" alt="">
</p>

<h1 align="center">Sendbox</h1>

<p align="center">
  <strong>A local SMTP inbox in your menu bar, for you and your coding agent.</strong>
</p>

<p align="center">
  <a href="https://github.com/bmichotte/sendbox/releases/latest"><img src="https://img.shields.io/github/v/release/bmichotte/sendbox?label=release" alt="Latest release"></a>
  <a href="https://github.com/bmichotte/sendbox/actions/workflows/ci.yml"><img src="https://github.com/bmichotte/sendbox/actions/workflows/ci.yml/badge.svg" alt="CI"></a>
  <img src="https://img.shields.io/badge/platform-macOS%20%7C%20Windows-lightgrey" alt="macOS | Windows">
  <a href="LICENSE"><img src="https://img.shields.io/badge/license-MIT-blue" alt="MIT license"></a>
</p>

Sendbox catches the emails your app sends while you develop it. Point your SMTP settings at `127.0.0.1:1025`, and every Email lands in one Inbox on your machine. Nothing is ever delivered to anyone. Click the menu bar (or notification area) icon to see each Email the way a recipient would, with its headers, raw source and attachments.

Sendbox also embeds an [MCP](https://modelcontextprotocol.io) server. Claude Code, or any other MCP client, can read the same Emails: wait for the sign-up Email your app just sent, open its verification link, and test the whole flow end to end.

<!-- Screenshot: add docs/screenshot.png and reference it here. -->

## Why Sendbox?

- **Always there, never in the way.** It's a small native app in the menu bar, not a daemon to remember to start or a browser tab to find. Launch it at login and forget about it.
- **Built for agents.** The MCP server is part of the app. `wait_for_email` lets an agent trigger an action, then get the resulting Email in one call, even if it arrived a moment before.
- **Zero-config SMTP.** Any sender, any recipient, any credentials, no TLS. Existing nodemailer, Django or Rails dev configs work unchanged.
- **Faithful.** Each Email's raw source is kept byte for byte, along with the SMTP envelope, so Bcc recipients show up too.
- **Local by design.** It listens on loopback only, never relays, and sends no telemetry.

## Features

- **Capture**: SMTP on `127.0.0.1:1025`, with UTF-8 and 8-bit content, Emails up to 25 MB, and null senders (`MAIL FROM:<>`).
- **One persistent Inbox**: it survives restarts and keeps the newest 1,000 Emails by default.
- **Sender tabs**: an "All" tab plus one tab per sender. Each shows its Unread count, and tabs appear and disappear on their own.
- **Search** across sender, recipients and subject.
- **Email detail**:
  - rendered HTML in a sandboxed frame, with inline `cid:` images;
  - plain text;
  - every header plus the envelope;
  - the raw source;
  - attachments you can save to disk.
- **Unread state**: Unread Emails are bold, and the Unread count shows on the tray icon.
- **Tray menu**: Open, Clear all, Settings, Quit. A left-click toggles the window.
- **Settings**: ports, Retention limit, launch at login, and whether agent reads mark Emails as read. Changes apply live, without a restart.
- **MCP tools**: `list_emails`, `get_email`, `wait_for_email` and `delete_emails`.

## Install

Download the installer from the [latest release](https://github.com/bmichotte/sendbox/releases/latest).

### macOS (13 Ventura or later)

1. Download `Sendbox_<version>_universal.dmg`. It runs on both Apple Silicon and Intel Macs.
2. Open it and drag **Sendbox** into **Applications**.
3. Open Sendbox from Applications. Its icon appears in the menu bar; there is no Dock icon.

### Windows (10 or 11, x64)

1. Download `Sendbox_<version>_x64-setup.exe`. If your browser warns that the file isn't commonly downloaded, choose **Keep**. In Edge: **…** → **Keep** → **Show more** → **Keep anyway**.
2. Run it. The installer is not code-signed, so Microsoft Defender SmartScreen shows "Windows protected your PC". Click **More info**, then **Run anyway**.
3. Follow the installer. It installs for your user only, without administrator rights.

Sendbox starts in the notification area. If you don't see its icon, open the **^** overflow next to the clock; you can drag the icon onto the taskbar to keep it visible.

## Quick start

Launch Sendbox, then send it something. With nothing but `curl`:

```sh
printf 'From: app@example.com\r\nTo: you@example.com\r\nSubject: Hello Sendbox\r\n\r\nIt works.\r\n' \
  | curl smtp://127.0.0.1:1025 --mail-from app@example.com --mail-rcpt you@example.com -T -
```

The Email appears in the window immediately, and the tray shows one Unread Email.

### Point your app at Sendbox

Use host `127.0.0.1`, port `1025`, and no TLS. Sendbox accepts any sender, recipient and credentials, so you can leave a `user`/`pass` in your config.

**Node.js (nodemailer)**

```js
const transport = nodemailer.createTransport({ host: "127.0.0.1", port: 1025 });
```

**Python**

```python
import smtplib
from email.message import EmailMessage

msg = EmailMessage()
msg["From"], msg["To"], msg["Subject"] = "app@example.com", "you@example.com", "Hello"
msg.set_content("It works.")

with smtplib.SMTP("127.0.0.1", 1025) as smtp:
    smtp.send_message(msg)
```

<details>
<summary><strong>Django, Rails, Rust (lettre)</strong></summary>

**Django** (`settings.py`)

```python
EMAIL_BACKEND = "django.core.mail.backends.smtp.EmailBackend"
EMAIL_HOST = "127.0.0.1"
EMAIL_PORT = 1025
```

**Rails** (`config/environments/development.rb`)

```ruby
config.action_mailer.delivery_method = :smtp
config.action_mailer.smtp_settings = { address: "127.0.0.1", port: 1025 }
```

**Rust** ([lettre](https://crates.io/crates/lettre))

```rust
let mailer = SmtpTransport::builder_dangerous("127.0.0.1").port(1025).build();
```

</details>

## Using Sendbox

- **Left-click** the tray icon to show or hide the window. **Right-click** it for **Open**, **Clear all…**, **Settings…** and **Quit**.
- **Closing the window hides it**, whether by the close button, Cmd+W or Alt+F4. Sendbox keeps capturing until you choose **Quit** in the tray menu.
- The **Unread count** shows next to the menu bar icon on macOS. On Windows it is drawn on the tray icon and repeated in its tooltip.
- **Opening an Email marks it read.** Emails without HTML open on the Text tab.
- **Delete** an Email from its row, or empty the Inbox with **Clear all…**. Both the window and the tray menu ask for confirmation first.
- Launching Sendbox again while it's running just brings up the existing window.

## Let your agent read Emails (MCP)

While Sendbox runs, it serves an MCP endpoint at `http://127.0.0.1:1026/mcp` (Streamable HTTP). Add it to Claude Code with:

```sh
claude mcp add --transport http sendbox http://127.0.0.1:1026/mcp
```

To share the setup with your team, commit a project-level `.mcp.json` instead:

```json
{
  "mcpServers": {
    "sendbox": { "type": "http", "url": "http://127.0.0.1:1026/mcp" }
  }
}
```

Any other MCP client that supports Streamable HTTP can use the same URL. If you changed the MCP port in Settings, use that port instead of `1026`.

### Tools

| Tool | Parameters | Returns |
| --- | --- | --- |
| `list_emails` | `from`, `to`, `subject`, `since`, `unread_only`, `limit` (default 20) | Summaries, newest first: id, from, to, subject, date, read. Never changes read state. |
| `get_email` | `id` | Text, HTML, headers, envelope, the links the Email contains, and attachment metadata (name, type, size; never the bytes). |
| `wait_for_email` | `from`, `to`, `subject`, `since`, `timeout_seconds` (default 30, max 120) | The oldest matching Unread Email already in the Inbox, otherwise the next matching Capture. On timeout it returns "no Email received" rather than an error. Cancelling the call stops the wait. |
| `delete_emails` | `ids`, *or* the `list_emails` filters, *or* `all: true` | The number of Emails deleted. A call with no selector is refused. |

- **Filters** are case-insensitive substrings. `to` matches the To, Cc and envelope recipients, so Bcc'd test addresses are found.
- **`since`** is an RFC 3339 time, and only Emails received strictly after it match. Pass an Email's `date` to skip that Email and the ones before it.
- **Reading marks Emails as read.** `get_email` and `wait_for_email` mark the Email they return read, so two waits in a row never return the same Email. You can turn this off in Settings.

### Example

> Sign up on http://localhost:3000 with `test+1@example.com`. Then use Sendbox to wait for the verification Email, open its link, and check that the account is confirmed.

## Settings

Open **Settings…** from the tray menu. Changes apply immediately; nothing needs a restart.

| Setting | Default | Notes |
| --- | --- | --- |
| SMTP port | `1025` | Changing it moves the listener live. |
| MCP port | `1026` | Same. |
| Retention limit | `1000` | Above the limit, the oldest Emails are pruned, read or not. Lowering it prunes immediately. |
| MCP reads mark Emails as read | on | Turn it off if agent activity shouldn't touch your Unread state. |
| Launch at login | off | macOS adds Sendbox under **Open at Login**; Windows uses the standard startup entry. |

If another tool (Mailpit, MailCatcher…) already holds a port, Sendbox keeps running. Its icon shows an error state, the menu says "Port 1025 in use", and Settings shows the error next to the port. Pick a free port and the error clears.

## SMTP details

- Listens on `127.0.0.1` only and never relays or delivers anything.
- EHLO advertises `AUTH PLAIN LOGIN`, `8BITMIME`, `SMTPUTF8` and `SIZE 26214400`.
- There is no `STARTTLS`, on purpose: clients like nodemailer treat a failed upgrade as fatal, and plain SMTP on loopback needs no certificates. There is no `PIPELINING` either.
- Any credentials, any sender (including the null sender `<>`) and any recipients are accepted.
- Emails up to 25 MiB are accepted. Larger ones are rejected with a `552` reply.
- The raw source is stored exactly as received, together with the SMTP envelope.

## Where your data lives

| | macOS | Windows |
| --- | --- | --- |
| Inbox (`inbox.sqlite3`) | `~/Library/Application Support/be.michotte.sendbox/` | `%LOCALAPPDATA%\be.michotte.sendbox\` |
| Settings (`settings.json`) | `~/Library/Application Support/be.michotte.sendbox/` | `%APPDATA%\be.michotte.sendbox\` |

To start from scratch, quit Sendbox and delete these folders.

## Security and privacy

- **Nothing leaves your machine.** Both SMTP and MCP listen on loopback only, and Sendbox never forwards an Email.
- **Protected from your browser.** The MCP endpoint rejects requests whose `Host` or `Origin` isn't local, so a web page can't read your Inbox through DNS rebinding. Pages served from `localhost` are allowed.
- **No auth token.** Any process running as you on this machine can reach the SMTP and MCP ports, as with any local dev server. Don't send real secrets through it.
- **HTML runs no scripts.** Email HTML renders in a sandboxed frame with scripts disabled. Remote images, stylesheets and fonts *are* loaded when you open an Email, as in a regular mail client, so tracking pixels in it will fire.
- **No telemetry, no auto-update.**

## Troubleshooting

- **The icon shows an error / "Port 1025 in use".** Another tool holds the port. Quit it, or choose another port in Settings.
- **My app says the connection was refused.** Check that Sendbox is running and that your app uses `127.0.0.1` with the port shown in Settings. Since Sendbox listens on loopback only, apps on other machines can't reach it.
- **My client insists on TLS.** Disable TLS/STARTTLS in its development config. Sendbox speaks plain SMTP only.
- **The agent says it can't connect.** Sendbox isn't running, or the MCP port changed. Launch Sendbox, or update the URL.
- **Linux?** Not supported for now.

## Development

Requirements: Rust (stable, 1.98 or newer) and Node 24.

```sh
npm ci
npm run tauri dev     # run the app, with the UI hot-reloading
npm run tauri build   # build the installer for this machine
```

The checks CI runs, on macOS and Windows:

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
npm run typecheck
npm test              # the release scripts
```

### Layout

| Path | What it is |
| --- | --- |
| `crates/sendbox-core` | All behaviour: SMTP Capture, the SQLite Inbox, Email views, Sender tabs, Retention and the MCP server. It has no Tauri dependency. |
| `crates/sendbox-app` | The thin Tauri 2 shell: tray, windows, settings persistence and plugins. |
| `ui/` | The React + TypeScript + Tailwind UI. It renders what the core gives it and holds no business logic. |
| `GLOSSARY.md` | The domain vocabulary: Capture, Email, Inbox, Sender tab, Unread, Retention limit. |
| `docs/adr/` | Architecture decisions: [MCP embedded over HTTP](docs/adr/0001-mcp-embedded-over-http.md), [Tauri and Rust over Electron](docs/adr/0002-tauri-and-rust-over-electron.md). |

Tests drive a real core the way the outside world does. They send Emails over SMTP (with `lettre` and raw TCP sessions), then check results over MCP (with an `rmcp` client) and through the core handle the shell uses. They never look at SQLite tables or private modules, so storage and parsing can change freely. The shell and UI stay thin and are checked by hand.

Maintainers: see [docs/releasing.md](docs/releasing.md) to cut a release.

## Contributing

Issues and pull requests are welcome. For anything bigger than a small fix, please open an issue first to discuss it. Before sending a PR, run the checks above, and keep to the glossary's vocabulary in code, UI text and docs.

## Acknowledgements

Sendbox is built on [Tauri](https://tauri.app). It uses Stalwart's [`smtp-proto`](https://github.com/stalwartlabs/smtp-proto) and [`mail-parser`](https://github.com/stalwartlabs/mail-parser), and the official Rust MCP SDK [`rmcp`](https://github.com/modelcontextprotocol/rust-sdk). It was inspired by [MailCrab](https://github.com/tweedegolf/mailcrab), [Mailpit](https://github.com/axllent/mailpit) and NodemailerApp.

## License

[MIT](LICENSE) © 2026 Benjamin Michotte
