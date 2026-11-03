import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useRef,
  useState,
  type ReactNode,
} from "react";
import { open as openDialog, save as saveDialog } from "@tauri-apps/plugin-dialog";

import { bridge } from "../bridge/env";
import type {
  AutosaveStatus,
  Command,
  RecoveryEntry,
  SessionState,
  ShellInfo,
} from "../bridge/types";

const PROJECT_FILTER = {
  name: "Forge3D Studio project",
  extensions: ["studio3d"],
};

interface SessionApi {
  bridgeKind: "tauri" | "mock";
  info: ShellInfo | null;
  state: SessionState | null;
  autosave: AutosaveStatus | null;
  recovery: RecoveryEntry[];
  error: string | null;
  notice: string | null;
  busy: boolean;
  actions: {
    newProject(): Promise<void>;
    openProject(): Promise<void>;
    saveProject(saveAs?: boolean): Promise<void>;
    closeProject(): Promise<void>;
    apply(command: Command): Promise<boolean>;
    undo(): Promise<void>;
    redo(): Promise<void>;
    select(ids: string[]): Promise<void>;
    restoreRecovery(id: string): Promise<void>;
    discardRecovery(id: string): Promise<void>;
    dismissError(): void;
  };
}

const SessionContext = createContext<SessionApi | null>(null);

export function SessionProvider({ children }: { children: ReactNode }) {
  const [info, setInfo] = useState<ShellInfo | null>(null);
  const [state, setState] = useState<SessionState | null>(null);
  const [autosave, setAutosave] = useState<AutosaveStatus | null>(null);
  const [recovery, setRecovery] = useState<RecoveryEntry[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const noticeTimer = useRef<number | null>(null);

  const flash = useCallback((message: string) => {
    setNotice(message);
    if (noticeTimer.current !== null) window.clearTimeout(noticeTimer.current);
    noticeTimer.current = window.setTimeout(() => setNotice(null), 2600);
  }, []);

  const fail = useCallback((cause: unknown) => {
    setError(cause instanceof Error ? cause.message : String(cause));
  }, []);

  const refreshRecovery = useCallback(async () => {
    try {
      setRecovery(await bridge.listRecovery());
    } catch (cause) {
      fail(cause);
    }
  }, [fail]);

  useEffect(() => {
    let cancelled = false;
    (async () => {
      try {
        const [shellInfo, snapshot, autosaveStatus, entries] = await Promise.all([
          bridge.info(),
          bridge.state(),
          bridge.autosaveStatus(),
          bridge.listRecovery(),
        ]);
        if (cancelled) return;
        setInfo(shellInfo);
        setState(snapshot);
        setAutosave(autosaveStatus);
        setRecovery(entries);
      } catch (cause) {
        if (!cancelled) fail(cause);
      }
    })();
    return () => {
      cancelled = true;
      if (noticeTimer.current !== null) window.clearTimeout(noticeTimer.current);
    };
  }, [fail]);

  // Keep "autosaved 14:03" honest without a websocket: one cheap poll.
  useEffect(() => {
    if (bridge.kind !== "tauri") return;
    const timer = window.setInterval(() => {
      bridge
        .autosaveStatus()
        .then(setAutosave)
        .catch(() => undefined);
    }, 10_000);
    return () => window.clearInterval(timer);
  }, []);

  const guardUnsaved = useCallback(
    (action: string): boolean => {
      if (!state?.dirty) return true;
      return window.confirm(
        `"${state.project.name}" has unsaved changes. Continue and ${action}?`,
      );
    },
    [state],
  );

  const actions = useMemo<SessionApi["actions"]>(
    () => ({
      async newProject() {
        if (!guardUnsaved("start a new project")) return;
        setBusy(true);
        try {
          setState(await bridge.newProject());
          await refreshRecovery();
          flash("New project");
        } catch (cause) {
          fail(cause);
        } finally {
          setBusy(false);
        }
      },

      async openProject() {
        if (!guardUnsaved("open another project")) return;
        try {
          let path: string | null = null;
          if (bridge.kind === "tauri") {
            const picked = await openDialog({
              multiple: false,
              filters: [PROJECT_FILTER],
            });
            path = typeof picked === "string" ? picked : null;
          } else {
            path = "/preview/example.studio3d";
          }
          if (!path) return;
          setBusy(true);
          setState(await bridge.openProject(path));
          await refreshRecovery();
          flash("Project opened");
        } catch (cause) {
          fail(cause);
        } finally {
          setBusy(false);
        }
      },

      async saveProject(saveAs = false) {
        if (!state) return;
        try {
          let path: string | null = null;
          if (saveAs || !state.path) {
            if (bridge.kind === "tauri") {
              const picked = await saveDialog({
                defaultPath: `${state.project.name || "untitled"}.studio3d`,
                filters: [PROJECT_FILTER],
              });
              path = picked ?? null;
            } else {
              path = "/preview/untitled.studio3d";
            }
            if (!path) return;
          }
          setBusy(true);
          const saved = await bridge.saveProject(path);
          setState(saved);
          await refreshRecovery();
          flash(`Saved ${saved.path ?? "project"}`);
        } catch (cause) {
          fail(cause);
        } finally {
          setBusy(false);
        }
      },

      async closeProject() {
        if (!guardUnsaved("close the project")) return;
        setBusy(true);
        try {
          setState(await bridge.closeProject());
          flash("Project closed");
        } catch (cause) {
          fail(cause);
        } finally {
          setBusy(false);
        }
      },

      async apply(command) {
        try {
          const result = await bridge.applyCommand(command);
          setState(result.state);
          return true;
        } catch (cause) {
          fail(cause);
          return false;
        }
      },

      async undo() {
        try {
          setState((await bridge.undo()).state);
        } catch (cause) {
          fail(cause);
        }
      },

      async redo() {
        try {
          setState((await bridge.redo()).state);
        } catch (cause) {
          fail(cause);
        }
      },

      async select(ids) {
        try {
          setState(await bridge.setSelection(ids));
        } catch (cause) {
          fail(cause);
        }
      },

      async restoreRecovery(id) {
        setBusy(true);
        try {
          setState(await bridge.restoreRecovery(id));
          await refreshRecovery();
          flash("Recovered unsaved work");
        } catch (cause) {
          fail(cause);
        } finally {
          setBusy(false);
        }
      },

      async discardRecovery(id) {
        try {
          await bridge.discardRecovery(id);
          await refreshRecovery();
          flash("Recovery snapshot discarded");
        } catch (cause) {
          fail(cause);
        }
      },

      dismissError() {
        setError(null);
      },
    }),
    [bridge, fail, flash, guardUnsaved, refreshRecovery, state],
  );

  const value: SessionApi = {
    bridgeKind: bridge.kind,
    info,
    state,
    autosave,
    recovery,
    error,
    notice,
    busy,
    actions,
  };

  return (
    <SessionContext.Provider value={value}>{children}</SessionContext.Provider>
  );
}

export function useSession(): SessionApi {
  const value = useContext(SessionContext);
  if (!value) throw new Error("useSession must be used inside SessionProvider");
  return value;
}
