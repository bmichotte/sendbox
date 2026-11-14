import { type FormEvent, type ReactNode, useEffect, useId, useState } from "react";
import { type ListenerStatus, useListeners } from "./listeners";
import { stopListening } from "./live";
import {
  getSettings,
  type LaunchAtLogin,
  launchAtLogin,
  onWindowFocused,
  type Settings,
  setLaunchAtLogin,
  setSettings,
} from "./settings";

/** The Settings window. Each change is saved and applied at once. */
export function SettingsWindow() {
  const [settings, setSaved] = useState<Settings | null>(null);
  const [atLogin, setAtLogin] = useState<LaunchAtLogin | null>(null);
  const [error, setError] = useState<string | null>(null);
  const listeners = useListeners();

  useEffect(() => {
    let active = true;
    const fail = (reason: unknown) => active && setError(String(reason));
    getSettings().then((saved) => active && setSaved(saved), fail);
    const refreshAtLogin = () =>
      launchAtLogin().then((state) => active && setAtLogin(state), fail);
    refreshAtLogin();
    // The window lives on, hidden, between uses; meanwhile the user may
    // change launch at login in the system settings.
    const listening = onWindowFocused(refreshAtLogin);
    return () => {
      active = false;
      stopListening(listening);
    };
  }, []);

  const save = (next: Settings) => {
    setError(null);
    setSettings(next).then(
      () => setSaved(next),
      (reason: unknown) => setError(String(reason)),
    );
  };
  const saveAtLogin = (enabled: boolean) => {
    setError(null);
    // Read back from the OS: macOS may wait for the user to allow it.
    setLaunchAtLogin(enabled)
      .then(launchAtLogin)
      .then(setAtLogin, (reason: unknown) => setError(String(reason)));
  };

  return (
    <div className="flex h-screen flex-col gap-5 overflow-y-auto bg-white p-5 text-sm text-neutral-900 dark:bg-neutral-900 dark:text-neutral-100">
      {settings !== null && (
        <>
          <NumberField
            // A new saved value resets the field to it.
            key={`smtp-port-${settings.smtpPort}`}
            label="SMTP port"
            saved={settings.smtpPort}
            max={65535}
            error={portInUse(listeners?.smtp)}
            onApply={(smtpPort) => save({ ...settings, smtpPort })}
          >
            Apps send their Emails to <code className="font-mono">127.0.0.1</code> on this port,
            without TLS.
          </NumberField>
          <NumberField
            key={`mcp-port-${settings.mcpPort}`}
            label="MCP port"
            saved={settings.mcpPort}
            max={65535}
            error={portInUse(listeners?.mcp)}
            onApply={(mcpPort) => save({ ...settings, mcpPort })}
          >
            Agents connect to{" "}
            <code className="font-mono">http://127.0.0.1:{settings.mcpPort}/mcp</code>.
          </NumberField>
          <NumberField
            key={`retention-limit-${settings.retentionLimit}`}
            label="Retention limit"
            unit="Emails"
            saved={settings.retentionLimit}
            onApply={(retentionLimit) => save({ ...settings, retentionLimit })}
          >
            The Inbox keeps at most this many Emails: beyond it, the oldest are deleted, read or
            not. Lowering it deletes them right away.
          </NumberField>
          <Toggle
            label="MCP reads mark Emails as read"
            checked={settings.mcpMarksRead}
            onChange={(mcpMarksRead) => save({ ...settings, mcpMarksRead })}
          >
            An Email an agent reads over MCP is no longer Unread.
          </Toggle>
        </>
      )}
      <Toggle
        label="Launch at login"
        checked={atLogin === "on"}
        disabled={atLogin === null}
        error={
          atLogin === "needsApproval"
            ? "macOS waits for you to allow Sendbox in System Settings › General › Login Items."
            : null
        }
        onChange={saveAtLogin}
      >
        Start Sendbox when you log in.
      </Toggle>
      {error !== null && (
        <p role="alert" className="text-xs text-red-600 dark:text-red-400">
          {error}
        </p>
      )}
    </div>
  );
}

/** What the Settings window says under a port another program holds; null if it listens. */
function portInUse(status: ListenerStatus | undefined): string | null {
  return status?.state === "portInUse"
    ? `Port ${status.port} is in use. Choose another port, or Apply again once it is free.`
    : null;
}

/**
 * A whole number of at least 1, applied only on Apply or Enter: a value applied while typing
 * ("1" on the way to "100") would prune the Inbox or move a listener.
 */
function NumberField({
  label,
  unit,
  saved,
  max,
  error = null,
  onApply,
  children,
}: {
  label: string;
  unit?: string;
  saved: number;
  max?: number;
  error?: string | null;
  onApply: (value: number) => void;
  children: ReactNode;
}) {
  const [draft, setDraft] = useState(String(saved));
  const id = useId();
  const apply = (event: FormEvent) => {
    event.preventDefault();
    onApply(Number(draft));
  };

  return (
    <form onSubmit={apply}>
      <label htmlFor={id} className="font-medium">
        {label}
      </label>
      <div className="mt-1.5 flex items-center gap-2">
        <input
          id={id}
          type="number"
          required
          min={1}
          max={max}
          step={1}
          value={draft}
          aria-invalid={error !== null}
          onChange={(event) => setDraft(event.target.value)}
          className="w-24 rounded-md border border-neutral-300 bg-white px-2 py-1 tabular-nums aria-[invalid=true]:border-red-500 dark:border-neutral-700 dark:bg-neutral-800"
        />
        {unit !== undefined && <span>{unit}</span>}
        <button
          type="submit"
          // With an error, applying the same value tries it again.
          disabled={draft === String(saved) && error === null}
          className="ml-auto rounded-md border border-neutral-300 px-3 py-1 font-medium enabled:hover:bg-neutral-100 disabled:opacity-40 dark:border-neutral-700 dark:enabled:hover:bg-neutral-800"
        >
          Apply
        </button>
      </div>
      {error !== null && (
        <p role="alert" className="mt-1.5 text-xs text-red-600 dark:text-red-400">
          {error}
        </p>
      )}
      <p className="mt-1.5 text-xs text-neutral-500">{children}</p>
    </form>
  );
}

function Toggle({
  label,
  checked,
  disabled = false,
  error = null,
  onChange,
  children,
}: {
  label: string;
  checked: boolean;
  disabled?: boolean;
  error?: string | null;
  onChange: (checked: boolean) => void;
  children: ReactNode;
}) {
  const id = useId();

  return (
    <div className="flex items-start gap-3">
      <input
        id={id}
        type="checkbox"
        checked={checked}
        disabled={disabled}
        onChange={(event) => onChange(event.target.checked)}
        className="mt-0.5 size-4 shrink-0"
      />
      <div>
        <label htmlFor={id} className="font-medium">
          {label}
        </label>
        {error !== null && (
          <p role="alert" className="text-xs text-red-600 dark:text-red-400">
            {error}
          </p>
        )}
        <p className="text-xs text-neutral-500">{children}</p>
      </div>
    </div>
  );
}
