import { useEffect, useRef, useState } from "react";

import type { PrimitiveShape, Tool } from "../bridge/types";
import { useSession } from "../state/session";
import {
  CopyIcon,
  CursorIcon,
  ImportIcon,
  MoveIcon,
  PlusIcon,
  RotateIcon,
  ScaleIcon,
  TrashIcon,
} from "./icons";

const TOOLS: Array<{ tool: Tool; label: string; key: string; Icon: typeof CursorIcon }> = [
  { tool: "select", label: "Select", key: "V", Icon: CursorIcon },
  { tool: "move", label: "Move", key: "G", Icon: MoveIcon },
  { tool: "rotate", label: "Rotate", key: "R", Icon: RotateIcon },
  { tool: "scale", label: "Scale", key: "S", Icon: ScaleIcon },
];

const PRIMITIVES: Array<{ shape: PrimitiveShape; label: string }> = [
  { shape: "cube", label: "Cube" },
  { shape: "sphere", label: "Sphere" },
  { shape: "plane", label: "Plane" },
];

/** Tool strip floating over the viewport: the Blender-style tool palette. */
export function ViewportToolbar() {
  const { state, actions, tool, setTool, space, setSpace, bridgeKind } = useSession();
  const [addOpen, setAddOpen] = useState(false);
  const addRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (!addOpen) return;
    const close = (event: MouseEvent) => {
      if (!addRef.current?.contains(event.target as Node)) setAddOpen(false);
    };
    const escape = (event: KeyboardEvent) => {
      if (event.key === "Escape") setAddOpen(false);
    };
    window.addEventListener("mousedown", close);
    window.addEventListener("keydown", escape);
    return () => {
      window.removeEventListener("mousedown", close);
      window.removeEventListener("keydown", escape);
    };
  }, [addOpen]);

  const selection = state?.project.selection ?? [];
  const hasSelection = selection.length > 0;
  const canTransform = hasSelection;
  const importing = bridgeKind === "mock";

  return (
    <div className="viewport-toolbar">
      <div className="toolbar-group" role="group" aria-label="Transform tool">
        {TOOLS.map(({ tool: value, label, key, Icon }) => (
          <button
            key={value}
            type="button"
            className={`tool-button ${tool === value ? "is-active" : ""}`}
            title={`${label} (${key})`}
            aria-pressed={tool === value}
            onClick={() => setTool(value)}
          >
            <Icon size={15} />
          </button>
        ))}
      </div>

      <span className="toolbar-divider" />

      <div className="toolbar-segment" role="group" aria-label="Gizmo orientation">
        <button
          type="button"
          className={`segment-button ${space === "world" ? "is-active" : ""}`}
          disabled={!canTransform || tool === "select"}
          onClick={() => setSpace("world")}
          title="World-aligned axes"
        >
          Global
        </button>
        <button
          type="button"
          className={`segment-button ${space === "local" ? "is-active" : ""}`}
          disabled={!canTransform || tool === "select"}
          onClick={() => setSpace("local")}
          title="Object-aligned axes"
        >
          Local
        </button>
      </div>

      <span className="toolbar-divider" />

      <div className="toolbar-group">
        <div className="add-menu" ref={addRef}>
          <button
            type="button"
            className={`tool-button tool-button--wide ${addOpen ? "is-active" : ""}`}
            onClick={() => setAddOpen((open) => !open)}
            aria-haspopup="menu"
            aria-expanded={addOpen}
            title="Add object (Shift+A opens this menu)"
          >
            <PlusIcon size={15} />
            <span>Add</span>
          </button>
          {addOpen && (
            <div className="add-menu__dropdown" role="menu">
              {PRIMITIVES.map(({ shape, label }) => (
                <button
                  key={shape}
                  type="button"
                  role="menuitem"
                  onClick={() => {
                    setAddOpen(false);
                    void actions.addPrimitive(shape);
                  }}
                >
                  {label}
                </button>
              ))}
              <div className="menu__separator" />
              <button
                type="button"
                role="menuitem"
                disabled={importing}
                title={
                  importing
                    ? "glTF import needs the desktop shell"
                    : "Import a .gltf or .glb file"
                }
                onClick={() => {
                  setAddOpen(false);
                  void actions.importMesh();
                }}
              >
                <ImportIcon size={14} /> Import glTF/GLB…
              </button>
            </div>
          )}
        </div>

        <button
          type="button"
          className="tool-button"
          disabled={!hasSelection}
          title="Duplicate (Shift+D)"
          onClick={() => void actions.duplicateSelection()}
        >
          <CopyIcon size={15} />
        </button>
        <button
          type="button"
          className="tool-button"
          disabled={!hasSelection}
          title="Delete (X or Del)"
          onClick={() => void actions.deleteSelection()}
        >
          <TrashIcon size={15} />
        </button>
      </div>
    </div>
  );
}
