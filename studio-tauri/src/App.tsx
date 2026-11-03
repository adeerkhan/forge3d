import { useEffect } from "react";

import { EmptyState } from "./components/EmptyState";
import { Inspector } from "./components/Inspector";
import { RecoveryBanner } from "./components/RecoveryBanner";
import { SceneTree } from "./components/SceneTree";
import { StatusBar } from "./components/StatusBar";
import { TopBar } from "./components/TopBar";
import { Viewport } from "./components/Viewport";
import { CloseIcon } from "./components/icons";
import { useSession } from "./state/session";

export default function App() {
  const { state, error, notice, actions } = useSession();

  useKeyboardShortcuts();

  return (
    <div className="app">
      <TopBar />

      <main className={`workspace ${state ? "" : "workspace--empty"}`}>
        {state ? (
          <>
            <aside className="workspace__panel workspace__panel--left">
              <SceneTree />
            </aside>
            <section className="workspace__center">
              <RecoveryBanner />
              <Viewport />
            </section>
            <aside className="workspace__panel workspace__panel--right">
              <Inspector />
            </aside>
          </>
        ) : (
          <>
            <RecoveryBanner />
            <EmptyState />
          </>
        )}
      </main>

      <StatusBar />

      <div className="toast-stack">
        {error && (
          <div className="toast toast--error" role="alert">
            <span>{error}</span>
            <button
              type="button"
              className="toast__close"
              aria-label="Dismiss error"
              onClick={actions.dismissError}
            >
              <CloseIcon size={12} />
            </button>
          </div>
        )}
        {notice && <div className="toast">{notice}</div>}
      </div>
    </div>
  );
}

function useKeyboardShortcuts() {
  const { actions } = useSession();

  useEffect(() => {
    const isEditable = (target: EventTarget | null) => {
      const element = target as HTMLElement | null;
      if (!element) return false;
      const tag = element.tagName;
      return (
        tag === "INPUT" ||
        tag === "TEXTAREA" ||
        tag === "SELECT" ||
        element.isContentEditable
      );
    };

    const onKeyDown = (event: KeyboardEvent) => {
      if (isEditable(event.target)) return;

      if (event.key === "Escape") {
        void actions.select([]);
        return;
      }

      if (!(event.ctrlKey || event.metaKey)) return;

      switch (event.key.toLowerCase()) {
        case "z":
          event.preventDefault();
          void (event.shiftKey ? actions.redo() : actions.undo());
          break;
        case "y":
          event.preventDefault();
          void actions.redo();
          break;
        case "s":
          event.preventDefault();
          void actions.saveProject(event.shiftKey);
          break;
        case "o":
          event.preventDefault();
          void actions.openProject();
          break;
        case "n":
          event.preventDefault();
          void actions.newProject();
          break;
      }
    };

    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [actions]);
}
