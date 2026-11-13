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

/** One Email in full, as the detail pane shows it. Mirrors `EmailDetail` in the shell. */
export interface EmailDetail extends EmailRow {
  /** Addresses in the `Cc` header. */
  cc: string[];
  /** The SMTP `MAIL FROM` address; null for a null sender (`<>`). */
  envelopeSender: string | null;
  /** The SMTP `RCPT TO` addresses, Bcc included. */
  envelopeRecipients: string[];
  /** The plain-text body; null if the Email has none. */
  text: string | null;
  /** The HTML body with its inline images embedded; null if the Email has none. */
  html: string | null;
  /** Every header field, in order, decoded. */
  headers: Header[];
  attachments: Attachment[];
  /** The Email exactly as received, as text. */
  rawSource: string;
}

export interface Header {
  name: string;
  value: string;
}

export interface Attachment {
  /** The file name the sender gave, if any. */
  name: string | null;
  contentType: string;
  /** Size in bytes, once decoded. */
  size: number;
}

/** Every Email in the Inbox, newest first. Rejects with the reason if the core is not running. */
export function listEmails(): Promise<EmailRow[]> {
  return invoke("list_emails");
}

/** One Email in full. Rejects with the reason if it is no longer in the Inbox. */
export function getEmail(id: number): Promise<EmailDetail> {
  return invoke("get_email", { id });
}

/**
 * Asks the user where to save attachment `index` (in `EmailDetail.attachments`) of an Email,
 * then saves it there. Resolves to false if the user cancelled.
 */
export function saveAttachment(emailId: number, index: number): Promise<boolean> {
  return invoke("save_attachment", { emailId, index });
}

/** The port the SMTP listener is bound to. */
export function smtpPort(): Promise<number> {
  return invoke("smtp_port");
}

/** Calls `handler` whenever the Inbox changes, until the returned function is called. */
export function onInboxChanged(handler: () => void): Promise<UnlistenFn> {
  return listen("inbox-changed", handler);
}
