import { type ReactNode, useEffect, useState } from "react";
import {
  clearAll,
  deleteEmail,
  type EmailRow,
  listEmails,
  onInboxChanged,
  smtpPort,
} from "./inbox";

type InboxState =
  | { status: "loading" }
  | { status: "ready"; emails: EmailRow[] }
  | { status: "failed"; reason: string };

export function App() {
  const inbox = useInbox();
  const port = useSmtpPort();
  const [actionError, setActionError] = useState<string | null>(null);
  // The list follows from the Inbox-changed event; only a failure needs showing.
  const run = (action: Promise<unknown>) =>
    action.then(
      () => setActionError(null),
      (reason: unknown) => setActionError(String(reason)),
    );
  const hasEmails = inbox.status === "ready" && inbox.emails.length > 0;

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
          {port !== null && (
            <p className="text-xs text-neutral-500">
              SMTP on <code className="font-mono">127.0.0.1:{port}</code>
            </p>
          )}
          <button
            type="button"
            disabled={!hasEmails}
            onClick={() => run(clearAll())}
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
      <main className="min-h-0 flex-1 overflow-y-auto">
        <InboxBody inbox={inbox} port={port} onDelete={(id) => run(deleteEmail(id))} />
      </main>
    </div>
  );
}

function InboxBody({
  inbox,
  port,
  onDelete,
}: {
  inbox: InboxState;
  port: number | null;
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
            <EmailListItem key={email.id} email={email} onDelete={() => onDelete(email.id)} />
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

function useSmtpPort(): number | null {
  const [port, setPort] = useState<number | null>(null);
  useEffect(() => {
    smtpPort().then(setPort, () => setPort(null));
  }, []);
  return port;
}

function EmailListItem({ email, onDelete }: { email: EmailRow; onDelete: () => void }) {
  const receivedAt = new Date(email.receivedAt);

  return (
    <li className="group border-b border-neutral-100 px-4 py-2.5 dark:border-neutral-800">
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
        {/* Out of sight until the row is hovered or the button focused. */}
        <button
          type="button"
          onClick={onDelete}
          aria-label={`Delete ${email.subject ?? "this Email"}`}
          className="shrink-0 rounded px-1.5 text-xs text-neutral-500 opacity-0 group-hover:opacity-100 hover:bg-neutral-100 hover:text-red-600 focus-visible:opacity-100 dark:hover:bg-neutral-800 dark:hover:text-red-400"
        >
          Delete
        </button>
      </div>
      <div className="truncate">
        {email.subject ?? <span className="text-neutral-500 italic">(no subject)</span>}
      </div>
      {email.to.length > 0 && (
        <div className="truncate text-xs text-neutral-500">To: {email.to.join(", ")}</div>
      )}
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
