// The Settings window's way to the shell (crates/sendbox-app/src/settings.rs).

import { invoke } from "@tauri-apps/api/core";
import type { UnlistenFn } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";

/** What the user sets in the Settings window. Mirrors `Settings` in the shell. */
export interface Settings {
  /** The port apps send Emails to: 1 to 65535. */
  smtpPort: number;
  /** The port agents connect to over MCP: 1 to 65535. */
  mcpPort: number;
  /** The most Emails the Inbox keeps: a whole number, at least 1. */
  retentionLimit: number;
  /** Whether an Email an agent reads over MCP becomes read. */
  mcpMarksRead: boolean;
}

/** Whether this page is the Settings window rather than the Inbox window. */
export function isSettingsWindow(): boolean {
  // The label of the window declared in tauri.conf.json.
  return getCurrentWindow().label === "settings";
}

/** The saved settings. */
export function getSettings(): Promise<Settings> {
  return invoke("get_settings");
}

/**
 * Saves `settings` and applies them at once: a new port moves its listener, a lower Retention
 * limit prunes the Inbox right away. A port in use is no error here: the listeners' status says so.
 */
export function setSettings(settings: Settings): Promise<void> {
  return invoke("set_settings", { settings });
}

/**
 * Whether Sendbox starts when the user logs in. "needsApproval": turned on, but macOS waits for
 * the user to allow it in System Settings first. Mirrors `LaunchAtLogin` in the shell.
 */
export type LaunchAtLogin = "off" | "on" | "needsApproval";

/** Whether Sendbox starts when the user logs in, as the OS has it. */
export function launchAtLogin(): Promise<LaunchAtLogin> {
  return invoke("launch_at_login");
}

export function setLaunchAtLogin(enabled: boolean): Promise<void> {
  return invoke("set_launch_at_login", { enabled });
}

/** Calls `handler` each time this window comes to the front, until the returned function is called. */
export function onWindowFocused(handler: () => void): Promise<UnlistenFn> {
  return getCurrentWindow().onFocusChanged(({ payload: focused }) => {
    if (focused) handler();
  });
}
