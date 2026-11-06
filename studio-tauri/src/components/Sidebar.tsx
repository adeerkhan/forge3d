import { useState } from "react";

import { useSession } from "../state/session-context";
import {
  CubeIcon,
  MountainIcon,
  SlidersIcon,
  SunIcon,
} from "./icons";
import {
  EnvironmentSettings,
  LightRig,
  ObjectProperties,
  ProjectFacts,
  RenderSettingsEditor,
} from "./Inspector";

/**
 * The properties sidebar: one tab per area of the project, each a stack of
 * collapsible sections — the Blender pattern the app borrows elsewhere (N to
 * hide, tools on G/R/S, numpad 0 for the camera).
 *
 * Tabs rather than one long scroll: the render output, the sun and the selected
 * object are different questions, and a 1000px scroll mixes them.
 */

type PropsTab = "render" | "scene" | "light" | "object";

const TABS: {
  id: PropsTab;
  label: string;
  Icon: typeof SlidersIcon;
  title: string;
}[] = [
  {
    id: "render",
    label: "Render",
    Icon: SlidersIcon,
    title: "Render: output size, exposure, shadows, quality and still extras",
  },
  {
    id: "scene",
    label: "Scene",
    Icon: MountainIcon,
    title: "Scene: project and scene facts, and the asset list",
  },
  {
    id: "light",
    label: "Lighting",
    Icon: SunIcon,
    title: "Lighting: the sun, solar placement and the environment",
  },
  {
    id: "object",
    label: "Object",
    Icon: CubeIcon,
    title: "Object: the selected object's transform, material and settings",
  },
];

const STORAGE_KEY = "studio.propsTab";

export function SettingsSidebar() {
  const { state } = useSession();
  const [tab, setTab] = useState<PropsTab>(() => {
    try {
      const stored = window.localStorage.getItem(STORAGE_KEY);
      return TABS.some((entry) => entry.id === stored) ? (stored as PropsTab) : "render";
    } catch {
      return "render";
    }
  });

  const select = (id: PropsTab) => {
    setTab(id);
    try {
      window.localStorage.setItem(STORAGE_KEY, id);
    } catch {
      // Storage disabled: the tab still switches, it just forgets.
    }
  };

  const active = TABS.find((entry) => entry.id === tab) ?? TABS[0];

  return (
    <div className="props">
      <div className="props__rail" role="tablist" aria-orientation="vertical" aria-label="Properties">
        {TABS.map(({ id, label, Icon, title }) => (
          <button
            key={id}
            type="button"
            role="tab"
            aria-selected={tab === id}
            aria-label={label}
            title={title}
            className={`props__tab${tab === id ? " is-active" : ""}`}
            onClick={() => select(id)}
          >
            <Icon size={16} />
          </button>
        ))}
      </div>

      <div className="props__panel">
        <header className="props__header">
          <h2>{active.label}</h2>
          <span>{state ? `rev ${state.revision}` : "no project"}</span>
        </header>
        <div className="inspector-body">
          {!state ? (
            <p className="hint props__note">
              Open or create a project to edit its properties.
            </p>
          ) : (
            <>
              {tab === "render" && <RenderSettingsEditor />}
              {tab === "scene" && <ProjectFacts />}
              {tab === "light" && (
                <>
                  <LightRig />
                  <EnvironmentSettings />
                </>
              )}
              {tab === "object" && <ObjectProperties />}
            </>
          )}
          {tab === "object" && (
            <p className="hint props__note">
              Press <kbd>N</kbd> to hide this sidebar.
            </p>
          )}
        </div>
      </div>
    </div>
  );
}
