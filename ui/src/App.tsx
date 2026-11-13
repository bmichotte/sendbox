import { type ReactNode, useEffect, useState } from "react";
import { EmailDetailPane } from "./EmailDetail";
import {
  clearAll,
  deleteEmail,
  type EmailRow,
  listEmails,
  listTabs,
  onInboxChanged,
  type Search,
  type SenderTabKey,
  type Tabs,
} from "./inbox";
import { useListeners } from "./listeners";

/** Something loaded from the shell, and loaded again whenever the Inbox changes. */
type Live<T> =
  | { status: "loading" }
  | { status: "ready"; value: T }
  | { status: "failed"; reason: string };

const NO_SEARCH: Search = { from: "", to: "", subject: "" };

export function App() {
  const tabs = useLive(listTabs, "tabs");
  // null for "All".
  const [tab, setTab] = useState<SenderTabKey | null>(null);
  // A Sender tab whose last Email left the Inbox is gone: back to "All".
  if (
    tab !== null &&
    tabs.status === "ready" &&
    !tabs.value.senders.some((sender) => sender.address === tab.address)
  ) {
    setTab(null);
  }
  const [search, setSearch] = useState<Search>(NO_SEARCH);
  const emails = useLive(() => listEmails(tab, search), JSON.stringify([tab, search]));
  const smtp = useListeners()?.smtp ?? null;
  const port = smtp?.state === "running" ? smtp.port : null;
  const [selected, setSelected] = useState<number | null>(null);
  // An Email that left the list is no longer open.
  const openId =
    emails.status === "ready" && emails.value.some((email) => email.id === selected)
      ? selected
      : null;
  const [actionError, setActionError] = useState<string | null>(null);
  // The list follows from the Inbox-changed event; only a failure needs showing.
  const run = (action: Promise<unknown>) =>
    action.then(
      () => setActionError(null),
      (reason: unknown) => setActionError(String(reason)),
    );
  const inboxIsEmpty = tabs.status !== "ready" || tabs.value.senders.length === 0;
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
          {emails.status === "ready" && (
            <span className="ml-2 font-normal text-neutral-500 tabular-nums">
              {emails.value.length}
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
            disabled={inboxIsEmpty}
            onClick={clearInbox}
            className="rounded border border-neutral-300 px-2 py-0.5 text-xs hover:bg-neutral-100 disabled:opacity-40 disabled:hover:bg-transparent dark:border-neutral-700 dark:hover:bg-neutral-800"
          >
            Clear all…
          </button>
        </div>
      </header>
      {tabs.status === "ready" && !inboxIsEmpty && (
        <>
          <TabBar tabs={tabs.value} current={tab} onChoose={setTab} />
          <SearchBar search={search} onChange={setSearch} />
        </>
      )}
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
            emails={emails}
            inboxIsEmpty={inboxIsEmpty}
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

/** "All" and one tab per sender, each with its Unread count. */
function TabBar({
  tabs,
  current,
  onChoose,
}: {
  tabs: Tabs;
  current: SenderTabKey | null;
  onChoose: (tab: SenderTabKey | null) => void;
}) {
  return (
    <nav
      role="tablist"
      aria-label="Senders"
      className="flex gap-1 overflow-x-auto border-b border-neutral-200 px-3 dark:border-neutral-800"
    >
      <TabButton selected={current === null} unread={tabs.unread} onClick={() => onChoose(null)}>
        All
      </TabButton>
      {tabs.senders.map((sender) => (
        <TabButton
          key={sender.address ?? ""}
          selected={current !== null && current.address === sender.address}
          unread={sender.unread}
          onClick={() => onChoose({ address: sender.address })}
        >
          {sender.address ?? <span className="italic">(no sender)</span>}
        </TabButton>
      ))}
    </nav>
  );
}

function TabButton({
  selected,
  unread,
  onClick,
  children,
}: {
  selected: boolean;
  unread: number;
  onClick: () => void;
  children: ReactNode;
}) {
  return (
    <button
      type="button"
      role="tab"
      aria-selected={selected}
      onClick={onClick}
      className={`-mb-px flex shrink-0 items-baseline gap-1.5 border-b-2 px-2 py-2 whitespace-nowrap ${
        selected
          ? "border-neutral-900 font-medium dark:border-neutral-100"
          : "border-transparent text-neutral-500 hover:text-neutral-900 dark:hover:text-neutral-100"
      }`}
    >
      {children}
      {unread > 0 && (
        <span className="rounded-full bg-neutral-200 px-1.5 text-xs font-semibold text-neutral-700 tabular-nums dark:bg-neutral-700 dark:text-neutral-200">
          <span className="sr-only">Unread: </span>
          {unread}
        </span>
      )}
    </button>
  );
}

/** Narrows the current tab by sender, recipient and subject. */
function SearchBar({ search, onChange }: { search: Search; onChange: (search: Search) => void }) {
  const fields: { field: keyof Search; label: string }[] = [
    { field: "from", label: "From" },
    { field: "to", label: "To" },
    { field: "subject", label: "Subject" },
  ];
  return (
    <div
      role="search"
      className="flex gap-2 border-b border-neutral-200 px-4 py-2 dark:border-neutral-800"
    >
      {fields.map(({ field, label }) => (
        <input
          key={field}
          type="search"
          aria-label={`Search by ${label.toLowerCase()}`}
          placeholder={label}
          value={search[field]}
          onChange={(event) => onChange({ ...search, [field]: event.target.value })}
          className="min-w-0 flex-1 rounded border border-neutral-300 bg-transparent px-2 py-1 text-xs placeholder:text-neutral-400 dark:border-neutral-700"
        />
      ))}
    </div>
  );
}

function InboxBody({
  emails,
  inboxIsEmpty,
  port,
  openId,
  onOpen,
  onDelete,
}: {
  emails: Live<EmailRow[]>;
  /** Whether the whole Inbox is empty, not only the list a tab or a search narrows. */
  inboxIsEmpty: boolean;
  port: number | null;
  openId: number | null;
  onOpen: (id: number) => void;
  onDelete: (id: number) => void;
}) {
  switch (emails.status) {
    case "loading":
      return null;
    case "failed":
      return <Notice title="Not Capturing">{emails.reason}</Notice>;
    case "ready":
      if (emails.value.length === 0 && !inboxIsEmpty) {
        return <Notice title="No matching Emails">No Email in this tab matches the search.</Notice>;
      }
      if (emails.value.length === 0) {
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
          {emails.value.map((email) => (
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

/**
 * What `load` answers, loaded again whenever the shell says the Inbox changed, and whenever `key`
 * changes: `key` names what `load` loads, so a new key means a new `load`.
 */
function useLive<T>(load: () => Promise<T>, key: string): Live<T> {
  const [live, setLive] = useState<Live<T>>({ status: "loading" });

  // `key` stands for `load`, a new function on every render.
  useEffect(() => {
    let active = true;
    let latest = 0;
    const failed = (reason: unknown): Live<T> => ({ status: "failed", reason: String(reason) });
    const refresh = () => {
      const request = ++latest;
      // Only the newest answer counts: an older, slower one would be stale.
      const settle = (next: Live<T>) => active && request === latest && setLive(next);
      load().then(
        (value) => settle({ status: "ready", value }),
        (reason: unknown) => settle(failed(reason)),
      );
    };
    const stopListening = onInboxChanged(refresh);
    // Load once listening, so that no Capture slips in between.
    stopListening.then(refresh, (reason: unknown) => active && setLive(failed(reason)));
    return () => {
      active = false;
      stopListening.then(
        (stop) => stop(),
        () => {},
      );
    };
  }, [key]);

  return live;
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
  // Unread Emails stand out in bold; the list follows live as they are read.
  const weight = email.read ? "font-normal" : "font-semibold";

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
          <span className={`min-w-0 flex-1 truncate ${weight}`}>
            {!email.read && <span className="sr-only">Unread: </span>}
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
        <div className={`truncate ${weight}`}>
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
