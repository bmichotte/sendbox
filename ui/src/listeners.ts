// Whether the SMTP listener and the MCP endpoint are listening, from the shell
// (crates/sendbox-app/src/listeners.rs).

import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { useEffect, useState } from "react";
import { stopListening } from "./live";

/** Mirrors `Status` in the shell. */
export type ListenerStatus =
  /** Listening on `port`. */
  | { state: "running"; port: number }
  /** Not listening: another program holds `port`. */
  | { state: "portInUse"; port: number };

/** Mirrors `Listeners` in the shell. */
export interface Listeners {
  smtp: ListenerStatus;
  mcp: ListenerStatus;
}

/** Rejects with the reason if the core is not running. */
export function getListeners(): Promise<Listeners> {
  return invoke("listeners");
}

/** Calls `handler` whenever a listener's status changes, until the returned function is called. */
export function onListenersChanged(handler: (listeners: Listeners) => void): Promise<UnlistenFn> {
  return listen<Listeners>("listeners-changed", (event) => handler(event.payload));
}

/** The listeners' status, kept up to date; null until known, or while the core is not running. */
export function useListeners(): Listeners | null {
  const [listeners, setListeners] = useState<Listeners | null>(null);

  useEffect(() => {
    let active = true;
    let changes = 0;
    const listening = onListenersChanged((next) => {
      changes++;
      if (active) setListeners(next);
    });
    // Asked once listening, so that no change slips in between; a change
    // heard while asking is newer than the answer.
    listening
      .then(() => {
        const heard = changes;
        return getListeners().then((current) => {
          if (active && changes === heard) setListeners(current);
        });
      })
      .catch(() => {});
    return () => {
      active = false;
      stopListening(listening);
    };
  }, []);

  return listeners;
}
