# Tauri 2 with a Rust core, not Electron

Sendbox is built with Tauri 2: a Rust core (SMTP capture, Inbox storage, MCP server) and a React + Tailwind + TypeScript webview UI, targeting macOS and Windows. The obvious path for a Node-centric developer was Electron with nodemailer's `smtp-server` and `mailparser` (NodemailerApp itself was built that way), but Sendbox runs permanently in the menu bar / system tray, so Tauri's much smaller memory and disk footprint pays off every day, and learning Rust is an explicit goal of the project.

## Consequences

- The Rust core lives in its own crate, independent of Tauri, and is covered by integration tests that send real Emails over SMTP and assert through the Inbox and an MCP client; the Tauri crate stays a thin shell (tray, window, settings, UI events).
- The SMTP receiving side relies on the Rust ecosystem, which is less battle-tested than nodemailer's `smtp-server`.
