import { type FormEvent, type ReactNode, useEffect, useId, useState } from "react";
import {
  getSettings,
  launchAtLogin,
  onWindowFocused,
  type Settings,
  setLaunchAtLogin,
  setSettings,
} from "./settings";

/** The Settings window. Each change is saved and applied at once. */
export function SettingsWindow() {
  const [settings, setSaved] = useState<Settings | null>(null);
  const [atLogin, setAtLogin] = useState<boolean | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let active = true;
    const fail = (reason: unknown) => active && setError(String(reason));
    getSettings().then((saved) => active && setSaved(saved), fail);
    const refreshAtLogin = () =>
      launchAtLogin().then((enabled) => active && setAtLogin(enabled), fail);
    refreshAtLogin();
    // The window lives on, hidden, between uses; meanwhile the user may
    // change launch at login in the system settings.
    const stopListening = onWindowFocused(refreshAtLogin);
    return () => {
      active = false;
      stopListening.then(
        (stop) => stop(),
        () => {},
      );
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
    setLaunchAtLogin(enabled).then(
      () => setAtLogin(enabled),
      (reason: unknown) => setError(String(reason)),
    );
  };

  return (
    <div className="flex h-screen flex-col gap-5 overflow-y-auto bg-white p-5 text-sm text-neutral-900 dark:bg-neutral-900 dark:text-neutral-100">
      {settings !== null && (
        <>
          <RetentionLimitField
            // A new saved limit resets the field to it.
            key={settings.retentionLimit}
            saved={settings.retentionLimit}
            onApply={(retentionLimit) => save({ ...settings, retentionLimit })}
          />
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
        checked={atLogin ?? false}
        disabled={atLogin === null}
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

/** Applied only on Apply or Enter: a limit applied while typing ("1" on the way to "100") would prune the Inbox. */
function RetentionLimitField({
  saved,
  onApply,
}: {
  saved: number;
  onApply: (limit: number) => void;
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
        Retention limit
      </label>
      <div className="mt-1.5 flex items-center gap-2">
        <input
          id={id}
          type="number"
          required
          min={1}
          step={1}
          value={draft}
          onChange={(event) => setDraft(event.target.value)}
          className="w-24 rounded-md border border-neutral-300 bg-white px-2 py-1 tabular-nums dark:border-neutral-700 dark:bg-neutral-800"
        />
        <span>Emails</span>
        <button
          type="submit"
          disabled={draft === String(saved)}
          className="ml-auto rounded-md border border-neutral-300 px-3 py-1 font-medium enabled:hover:bg-neutral-100 disabled:opacity-40 dark:border-neutral-700 dark:enabled:hover:bg-neutral-800"
        >
          Apply
        </button>
      </div>
      <p className="mt-1.5 text-xs text-neutral-500">
        The Inbox keeps at most this many Emails: beyond it, the oldest are deleted, read or not.
        Lowering it deletes them right away.
      </p>
    </form>
  );
}

function Toggle({
  label,
  checked,
  disabled = false,
  onChange,
  children,
}: {
  label: string;
  checked: boolean;
  disabled?: boolean;
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
        <p className="text-xs text-neutral-500">{children}</p>
      </div>
    </div>
  );
}
