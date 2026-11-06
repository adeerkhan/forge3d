import { useEffect, useState } from "react";

import { EmptyState } from "./components/EmptyState";
import { SettingsSidebar } from "./components/Sidebar";
import { NewProjectDialog } from "./components/NewProjectDialog";
import { RecoveryBanner } from "./components/RecoveryBanner";
import { RenderQueue } from "./components/RenderQueue";
import { SceneTree } from "./components/SceneTree";
import { StatusBar } from "./components/StatusBar";
import { Timeline } from "./components/Timeline";
import { TopBar } from "./components/TopBar";
import { Viewport3D } from "./viewport/Viewport3D";
import { CloseIcon } from "./components/icons";
import { useSession } from "./state/session-context";

export default function App() {
  const { state, error, notice, actions, timelineOpen } = useSession();
  // Properties sidebar visibility (N). UI state, like the timeline.
  const [sidebarOpen, setSidebarOpen] = useState(true);

  useKeyboardShortcuts({ sidebarOpen, setSidebarOpen });

  return (
    <div className="app">
      <TopBar />

      <main
        className={`workspace${state ? "" : " workspace--empty"}${
          state && !sidebarOpen ? " workspace--no-props" : ""
        }`}
      >
        {state ? (
          <>
            <aside className="workspace__panel workspace__panel--left">
              <SceneTree />
            </aside>
            <section className="workspace__center">
              <RecoveryBanner />
              <Viewport3D />
              <RenderQueue />
            </section>
            {sidebarOpen && (
              <aside className="workspace__panel workspace__panel--right">
                <SettingsSidebar />
              </aside>
            )}
            {timelineOpen && <Timeline />}
          </>
        ) : (
          <>
            <RecoveryBanner />
            <EmptyState />
          </>
        )}
      </main>

      <StatusBar />
      <NewProjectDialog />

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

function useKeyboardShortcuts({
  sidebarOpen,
  setSidebarOpen,
}: {
  sidebarOpen: boolean;
  setSidebarOpen: (open: boolean) => void;
}) {
  const {
    actions,
    setTool,
    cameraView,
    setCameraView,
    timelineOpen,
    setTimelineOpen,
  } = useSession();

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

      const mod = event.ctrlKey || event.metaKey;
      const key = event.key.toLowerCase();

      if (!mod) {
        if (event.key === "Escape") {
          if (cameraView) setCameraView(false);
          else void actions.select([]);
          return;
        }
        if (event.key === "0") {
          // Blender's camera-view toggle; numpad 0 reports the same key.
          setCameraView(!cameraView);
          return;
        }
        if (key === "t") {
          setTimelineOpen(!timelineOpen);
          return;
        }
        if (key === "n") {
          // Blender's properties sidebar toggle.
          setSidebarOpen(!sidebarOpen);
          return;
        }
        // Blender-style manipulation tools and object actions.
        switch (key) {
          case "v":
            setTool("select");
            return;
          case "g":
            setTool("move");
            return;
          case "r":
            setTool("rotate");
            return;
          case "s":
            setTool("scale");
            return;
          case "x":
          case "delete":
          case "backspace":
            event.preventDefault();
            void actions.deleteSelection();
            return;
          case "d":
            if (event.shiftKey) {
              event.preventDefault();
              void actions.duplicateSelection();
            }
            return;
          default:
            return;
        }
      }

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
          actions.requestNewProject();
          break;
      }
    };

    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [
    actions,
    cameraView,
    setCameraView,
    setTool,
    timelineOpen,
    setTimelineOpen,
    sidebarOpen,
    setSidebarOpen,
  ]);
}
