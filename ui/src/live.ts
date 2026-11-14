// What the windows share to show what the shell says: something loaded from it, and listening to
// its events.

import type { UnlistenFn } from "@tauri-apps/api/event";

/** Something loaded from the shell: on its way, there, or the reason it could not be. */
export type Loaded<T> =
  | { status: "loading" }
  | { status: "ready"; value: T }
  | { status: "failed"; reason: string };

/** Stops `listening` once it has started; if it never did, there is nothing to stop. */
export function stopListening(listening: Promise<UnlistenFn>): void {
  listening.then(
    (stop) => stop(),
    () => {},
  );
}
