import { type ReactNode, useEffect, useState } from "react";
import { type EmailRow, listEmails, onInboxChanged, smtpPort } from "./inbox";

type InboxState =
  | { status: "loading" }
  | { status: "ready"; emails: EmailRow[] }
  | { status: "failed"; reason: string };

export function App() {
  const inbox = useInbox();
  const port = useSmtpPort();

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
        {port !== null && (
          <p className="text-xs text-neutral-500">
            SMTP on <code className="font-mono">127.0.0.1:{port}</code>
          </p>
        )}
      </header>
      <main className="min-h-0 flex-1 overflow-y-auto">
        <InboxBody inbox={inbox} port={port} />
      </main>
    </div>
  );
}

function InboxBody({ inbox, port }: { inbox: InboxState; port: number | null }) {
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
            <EmailListItem key={email.id} email={email} />
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

function EmailListItem({ email }: { email: EmailRow }) {
  const receivedAt = new Date(email.receivedAt);

  return (
    <li className="border-b border-neutral-100 px-4 py-2.5 dark:border-neutral-800">
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
