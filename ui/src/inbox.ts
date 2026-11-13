// The UI's only way to the core: commands and events of the Tauri shell
// (crates/sendbox-app/src/inbox.rs).

import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

/** One row of the Inbox list. Mirrors `EmailRow` in the shell. */
export interface EmailRow {
  id: number;
  /** When the Email was Captured, in milliseconds since the Unix epoch. */
  receivedAt: number;
  /** The `From` header address, else the envelope sender; null for neither. */
  sender: string | null;
  /** Addresses in the `To` header. */
  to: string[];
  subject: string | null;
}

/** Every Email in the Inbox, newest first. Rejects with the reason if the core is not running. */
export function listEmails(): Promise<EmailRow[]> {
  return invoke("list_emails");
}

/** The port the SMTP listener is bound to. */
export function smtpPort(): Promise<number> {
  return invoke("smtp_port");
}

/** Deletes one Email; the Inbox-changed event follows. */
export function deleteEmail(id: number): Promise<void> {
  return invoke("delete_email", { id });
}

/** Asks the user to confirm, then deletes every Email. Resolves to whether the Inbox was cleared. */
export function clearAll(): Promise<boolean> {
  return invoke("clear_all");
}

/** Calls `handler` whenever the Inbox changes, until the returned function is called. */
export function onInboxChanged(handler: () => void): Promise<UnlistenFn> {
  return listen("inbox-changed", handler);
}
