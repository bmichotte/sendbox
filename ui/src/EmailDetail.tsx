import { type ReactNode, useEffect, useState } from "react";
import { type Attachment, type EmailDetail, getEmail, saveAttachment } from "./inbox";

type DetailState =
  | { status: "loading" }
  | { status: "ready"; email: EmailDetail }
  | { status: "failed"; reason: string };

type Tab = "html" | "text" | "headers" | "raw";

const TABS: { tab: Tab; label: string }[] = [
  { tab: "html", label: "HTML" },
  { tab: "text", label: "Text" },
  { tab: "headers", label: "Headers" },
  { tab: "raw", label: "Raw" },
];

/** The full detail of one Email. Give it a `key` per Email so each opens on its default tab. */
export function EmailDetailPane({ id, onClose }: { id: number; onClose: () => void }) {
  const detail = useEmailDetail(id);
  const [chosenTab, setChosenTab] = useState<Tab | null>(null);

  if (detail.status !== "ready") {
    return (
      <Pane onClose={onClose}>
        {detail.status === "failed" && (
          <p className="px-4 py-16 text-center text-neutral-500">{detail.reason}</p>
        )}
      </Pane>
    );
  }

  const { email } = detail;
  // Emails without HTML open on the Text tab.
  const tab = chosenTab ?? (email.html !== null ? "html" : "text");
  const receivedAt = new Date(email.receivedAt);

  return (
    <Pane onClose={onClose}>
      <div className="space-y-0.5 border-b border-neutral-200 px-4 py-3 dark:border-neutral-800">
        <h2 className="text-base font-semibold break-words">
          {email.subject ?? <span className="text-neutral-500 italic">(no subject)</span>}
        </h2>
        <AddressLine label="From">{email.sender ?? "(no sender)"}</AddressLine>
        {email.to.length > 0 && <AddressLine label="To">{email.to.join(", ")}</AddressLine>}
        {email.cc.length > 0 && <AddressLine label="Cc">{email.cc.join(", ")}</AddressLine>}
        <p className="text-xs text-neutral-500">
          <time dateTime={receivedAt.toISOString()}>{receivedAt.toLocaleString()}</time>
        </p>
      </div>
      <div
        role="tablist"
        className="flex gap-1 border-b border-neutral-200 px-3 dark:border-neutral-800"
      >
        {TABS.map(({ tab: each, label }) => (
          <button
            key={each}
            type="button"
            role="tab"
            aria-selected={each === tab}
            onClick={() => setChosenTab(each)}
            className={`-mb-px border-b-2 px-2 py-1.5 text-xs font-medium ${
              each === tab
                ? "border-neutral-900 text-neutral-900 dark:border-neutral-100 dark:text-neutral-100"
                : "border-transparent text-neutral-500 hover:text-neutral-900 dark:hover:text-neutral-100"
            }`}
          >
            {label}
          </button>
        ))}
      </div>
      <div role="tabpanel" className="min-h-0 flex-1 overflow-y-auto">
        <TabBody tab={tab} email={email} />
      </div>
      {email.attachments.length > 0 && (
        <Attachments emailId={email.id} attachments={email.attachments} />
      )}
    </Pane>
  );
}

function Pane({ onClose, children }: { onClose: () => void; children: ReactNode }) {
  return (
    <section className="flex min-h-0 min-w-0 flex-1 flex-col">
      {/* On narrow windows the detail replaces the list: this goes back to it. */}
      <button
        type="button"
        onClick={onClose}
        className="border-b border-neutral-200 px-4 py-2 text-left text-xs text-neutral-500 hover:text-neutral-900 md:hidden dark:border-neutral-800 dark:hover:text-neutral-100"
      >
        ← Inbox
      </button>
      {children}
    </section>
  );
}

function AddressLine({ label, children }: { label: string; children: ReactNode }) {
  return (
    <p className="text-xs break-words">
      <span className="text-neutral-500">{label}: </span>
      {children}
    </p>
  );
}

function TabBody({ tab, email }: { tab: Tab; email: EmailDetail }) {
  switch (tab) {
    case "html":
      if (email.html === null) {
        return <Missing>This Email has no HTML version.</Missing>;
      }
      return (
        // No allow-scripts: the Email's scripts never run, and the frame is cut off from the
        // app. Its inline images are already embedded as data: URIs by the core.
        <iframe
          title="HTML version"
          sandbox=""
          srcDoc={email.html}
          referrerPolicy="no-referrer"
          className="block h-full w-full border-0 bg-white scheme-light"
        />
      );
    case "text":
      if (email.text === null) {
        return <Missing>This Email has no text version.</Missing>;
      }
      return <pre className="p-4 font-sans whitespace-pre-wrap break-words">{email.text}</pre>;
    case "headers":
      return <Headers email={email} />;
    case "raw":
      return (
        <pre className="p-4 font-mono text-xs whitespace-pre-wrap break-all">{email.rawSource}</pre>
      );
  }
}

function Headers({ email }: { email: EmailDetail }) {
  const envelope = [
    { name: "Envelope sender", value: email.envelopeSender ?? "<> (null sender)" },
    { name: "Envelope recipients", value: email.envelopeRecipients.join(", ") },
  ];
  return (
    <div className="space-y-4 p-4 text-xs">
      <HeaderList title="SMTP envelope" headers={envelope} />
      <HeaderList title="Headers" headers={email.headers} />
    </div>
  );
}

function HeaderList({ title, headers }: { title: string; headers: { name: string; value: string }[] }) {
  return (
    <section>
      <h3 className="mb-1 font-medium text-neutral-500">{title}</h3>
      <dl className="grid grid-cols-[minmax(0,10rem)_1fr] gap-x-3 gap-y-1">
        {headers.map(({ name, value }, index) => (
          <div key={index} className="contents">
            <dt className="truncate font-mono text-neutral-500" title={name}>
              {name}
            </dt>
            <dd className="font-mono break-all">{value}</dd>
          </div>
        ))}
      </dl>
    </section>
  );
}

function Attachments({ emailId, attachments }: { emailId: number; attachments: Attachment[] }) {
  const [error, setError] = useState<string | null>(null);

  const save = (index: number) => {
    setError(null);
    saveAttachment(emailId, index).catch((reason: unknown) => setError(String(reason)));
  };

  return (
    <div className="max-h-40 shrink-0 overflow-y-auto border-t border-neutral-200 px-4 py-2 dark:border-neutral-800">
      <h3 className="mb-1 text-xs font-medium text-neutral-500">
        Attachments <span className="tabular-nums">{attachments.length}</span>
      </h3>
      <ul className="space-y-1">
        {attachments.map((attachment, index) => (
          <li key={index} className="flex items-center gap-3 text-xs">
            <span className="min-w-0 flex-1 truncate" title={attachment.name ?? undefined}>
              {attachment.name ?? <span className="text-neutral-500 italic">(no name)</span>}
            </span>
            <span className="shrink-0 text-neutral-500">{attachment.contentType}</span>
            <span className="w-16 shrink-0 text-right text-neutral-500 tabular-nums">
              {formatSize(attachment.size)}
            </span>
            <button
              type="button"
              onClick={() => save(index)}
              className="shrink-0 rounded border border-neutral-300 px-2 py-0.5 hover:bg-neutral-100 dark:border-neutral-700 dark:hover:bg-neutral-800"
            >
              Save…
            </button>
          </li>
        ))}
      </ul>
      {error !== null && <p className="mt-1 text-xs text-red-600 dark:text-red-400">{error}</p>}
    </div>
  );
}

function Missing({ children }: { children: ReactNode }) {
  return <p className="px-4 py-16 text-center text-neutral-500">{children}</p>;
}

/** One Email in full, fetched again when `id` changes. */
function useEmailDetail(id: number): DetailState {
  const [detail, setDetail] = useState<DetailState>({ status: "loading" });

  useEffect(() => {
    let active = true;
    setDetail({ status: "loading" });
    getEmail(id).then(
      (email) => active && setDetail({ status: "ready", email }),
      (reason: unknown) => active && setDetail({ status: "failed", reason: String(reason) }),
    );
    return () => {
      active = false;
    };
  }, [id]);

  return detail;
}

/** Bytes as people read them: 512 B, 3.4 KB, 1.2 MB. */
function formatSize(bytes: number): string {
  if (bytes < 1024) {
    return `${bytes} B`;
  }
  const units = ["KB", "MB", "GB"];
  let size = bytes / 1024;
  let unit = 0;
  while (size >= 1024 && unit < units.length - 1) {
    size /= 1024;
    unit += 1;
  }
  return `${size.toFixed(size < 10 ? 1 : 0)} ${units[unit]}`;
}
