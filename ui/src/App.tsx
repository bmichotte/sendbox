import { type ReactNode, useEffect, useState } from "react";
import { EmailDetailPane } from "./EmailDetail";
import {
  clearAll,
  deleteEmail,
  type EmailRow,
  listEmails,
  onInboxChanged,
} from "./inbox";
import { useListeners } from "./listeners";

type InboxState =
  | { status: "loading" }
  | { status: "ready"; emails: EmailRow[] }
  | { status: "failed"; reason: string };

export function App() {
  const inbox = useInbox();
  const smtp = useListeners()?.smtp ?? null;
  const port = smtp?.state === "running" ? smtp.port : null;
  const [selected, setSelected] = useState<number | null>(null);
  // An Email that left the Inbox is no longer open.
  const openId =
    inbox.status === "ready" && inbox.emails.some((email) => email.id === selected)
      ? selected
      : null;
  const [actionError, setActionError] = useState<string | null>(null);
  // The list follows from the Inbox-changed event; only a failure needs showing.
  const run = (action: Promise<unknown>) =>
    action.then(
      () => setActionError(null),
      (reason: unknown) => setActionError(String(reason)),
    );
  const hasEmails = inbox.status === "ready" && inbox.emails.length > 0;
  // Deleting the open Email, or clearing the Inbox, closes it.
  const deleteOne = (id: number) =>
    run(
      deleteEmail(id).then(() => setSelected((current) => (current === id ? null : current))),
    );
  const clearInbox = () => run(clearAll().then((cleared) => cleared && setSelected(null)));

  return (
    <div className="flex h-screen flex-col bg-white text-sm text-neutral-900 dark:bg-neutral-900 dark:text-neutral-100">
      <header className="flex items-baseline justify-between gap-4 border-b border-neutral-200 px-4 py-3 dark:border-neutral-800">
        <h1 className="text-base font-semibold">
          Inbox
          {inbox.status === "ready" && (
            <span className="ml-2 font-normal text-neutral-500 tabular-nums">
              {inbox.emails.length}
            </span>
          )}
        </h1>
        <div className="flex items-baseline gap-4">
          {smtp?.state === "running" && (
            <p className="text-xs text-neutral-500">
              SMTP on <code className="font-mono">127.0.0.1:{smtp.port}</code>
            </p>
          )}
          {smtp?.state === "portInUse" && (
            <p role="alert" className="text-xs text-red-600 dark:text-red-400">
              SMTP port {smtp.port} in use: choose another in Settings
            </p>
          )}
          <button
            type="button"
            disabled={!hasEmails}
            onClick={clearInbox}
            className="rounded border border-neutral-300 px-2 py-0.5 text-xs hover:bg-neutral-100 disabled:opacity-40 disabled:hover:bg-transparent dark:border-neutral-700 dark:hover:bg-neutral-800"
          >
            Clear all…
          </button>
        </div>
      </header>
      {actionError !== null && (
        <p
          role="alert"
          className="border-b border-neutral-200 px-4 py-2 text-xs text-red-600 dark:border-neutral-800 dark:text-red-400"
        >
          {actionError}
        </p>
      )}
      <div className="flex min-h-0 flex-1">
        {/* With an Email open, the list narrows beside it, or hides on narrow windows. */}
        <main
          className={
            openId === null
              ? "min-h-0 flex-1 overflow-y-auto"
              : "hidden min-h-0 overflow-y-auto border-r border-neutral-200 md:block md:w-80 md:shrink-0 lg:w-96 dark:border-neutral-800"
          }
        >
          <InboxBody
            inbox={inbox}
            port={port}
            openId={openId}
            onOpen={setSelected}
            onDelete={deleteOne}
          />
        </main>
        {openId !== null && (
          <EmailDetailPane key={openId} id={openId} onClose={() => setSelected(null)} />
        )}
      </div>
    </div>
  );
}

function InboxBody({
  inbox,
  port,
  openId,
  onOpen,
  onDelete,
}: {
  inbox: InboxState;
  port: number | null;
  openId: number | null;
  onOpen: (id: number) => void;
  onDelete: (id: number) => void;
}) {
  switch (inbox.status) {
    case "loading":
      return null;
    case "failed":
      return <Notice title="Not Capturing">{inbox.reason}</Notice>;
    case "ready":
      if (inbox.emails.length === 0) {
        return (
          <Notice title="No Emails yet">
            Point your app's SMTP transport at{" "}
            <code className="font-mono">127.0.0.1:{port ?? "…"}</code>, without TLS. Any sender,
            recipient and credentials are accepted, and nothing is ever delivered.
          </Notice>
        );
      }
      return (
        <ul>
          {inbox.emails.map((email) => (
            <EmailListItem
              key={email.id}
              email={email}
              open={email.id === openId}
              onOpen={() => onOpen(email.id)}
              onDelete={() => onDelete(email.id)}
            />
          ))}
        </ul>
      );
  }
}

/** The Inbox, listed again whenever the shell says it changed. */
function useInbox(): InboxState {
  const [inbox, setInbox] = useState<InboxState>({ status: "loading" });

  useEffect(() => {
    let active = true;
    let latest = 0;
    const failed = (reason: unknown): InboxState => ({ status: "failed", reason: String(reason) });
    const refresh = () => {
      const request = ++latest;
      // Only the newest answer counts: an older, slower one would be stale.
      const settle = (next: InboxState) => active && request === latest && setInbox(next);
      listEmails().then(
        (emails) => settle({ status: "ready", emails }),
        (reason: unknown) => settle(failed(reason)),
      );
    };
    const stopListening = onInboxChanged(refresh);
    // List once listening, so that no Capture slips in between.
    stopListening.then(refresh, (reason: unknown) => active && setInbox(failed(reason)));
    return () => {
      active = false;
      stopListening.then(
        (stop) => stop(),
        () => {},
      );
    };
  }, []);

  return inbox;
}

function EmailListItem({
  email,
  open,
  onOpen,
  onDelete,
}: {
  email: EmailRow;
  open: boolean;
  onOpen: () => void;
  onDelete: () => void;
}) {
  const receivedAt = new Date(email.receivedAt);

  return (
    // Delete sits beside the button that opens the Email, not inside it.
    <li
      className={`group flex items-baseline border-b border-neutral-100 dark:border-neutral-800 ${
        open
          ? "bg-neutral-100 dark:bg-neutral-800"
          : "hover:bg-neutral-50 dark:hover:bg-neutral-800/50"
      }`}
    >
      <button
        type="button"
        onClick={onOpen}
        aria-current={open || undefined}
        className="block min-w-0 flex-1 py-2.5 pr-3 pl-4 text-left"
      >
        <div className="flex items-baseline gap-3">
          <span className="min-w-0 flex-1 truncate font-medium">
            {email.sender ?? <span className="text-neutral-500 italic">(no sender)</span>}
          </span>
          <time
            className="shrink-0 text-xs text-neutral-500 tabular-nums"
            dateTime={receivedAt.toISOString()}
            title={receivedAt.toLocaleString()}
          >
            {formatReceivedAt(receivedAt)}
          </time>
        </div>
        <div className="truncate">
          {email.subject ?? <span className="text-neutral-500 italic">(no subject)</span>}
        </div>
        {email.to.length > 0 && (
          <div className="truncate text-xs text-neutral-500">To: {email.to.join(", ")}</div>
        )}
      </button>
      {/* Out of sight until the row is hovered or the button focused. */}
      <button
        type="button"
        onClick={onDelete}
        aria-label={`Delete ${email.subject ?? "this Email"}`}
        className="mr-4 shrink-0 rounded px-1.5 text-xs text-neutral-500 opacity-0 group-hover:opacity-100 hover:bg-neutral-200 hover:text-red-600 focus-visible:opacity-100 dark:hover:bg-neutral-700 dark:hover:text-red-400"
      >
        Delete
      </button>
    </li>
  );
}

function Notice({ title, children }: { title: string; children: ReactNode }) {
  return (
    <div className="mx-auto max-w-md px-6 py-16 text-center">
      <h2 className="mb-2 font-medium">{title}</h2>
      <p className="text-neutral-500">{children}</p>
    </div>
  );
}

/** The time alone for today's Emails, the day and time otherwise. */
function formatReceivedAt(date: Date): string {
  const today = new Date().toDateString() === date.toDateString();
  return today
    ? date.toLocaleTimeString(undefined, { hour: "2-digit", minute: "2-digit", second: "2-digit" })
    : date.toLocaleString(undefined, {
        year: date.getFullYear() === new Date().getFullYear() ? undefined : "numeric",
        month: "short",
        day: "numeric",
        hour: "2-digit",
        minute: "2-digit",
      });
}
