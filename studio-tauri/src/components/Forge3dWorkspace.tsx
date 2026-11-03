import { useEffect, useRef } from "react";

import type { CapabilityLevel, Forge3dStatus } from "../bridge/types";
import { useSession } from "../state/session";
import { AlertIcon, CameraIcon, ImportIcon } from "./icons";

const LEVEL_LABEL: Record<CapabilityLevel, string> = {
  full: "supported",
  partial: "partial",
  unsupported: "unsupported",
};

function ViewerStatusLine({ status }: { status: Forge3dStatus }) {
  return (
    <span className={`viewer-state ${status.running ? "is-running" : ""}`}>
      {status.running
        ? status.ready
          ? `viewer running · pid ${status.pid ?? "?"}`
          : "viewer starting…"
        : "viewer stopped"}
    </span>
  );
}

/**
 * The Forge3D workspace.
 *
 * The viewer is a separate window driven over IPC, so this panel is the bridge
 * cockpit: what can be carried, what cannot, what the engine is, and what the
 * viewer has said so far. Studio's project stays untouched throughout.
 */
export function Forge3dWorkspace() {
  const { state, engine, forgeStatus, handoff, actions, bridgeKind } = useSession();
  const probed = useRef(false);

  useEffect(() => {
    if (probed.current) return;
    probed.current = true;
    void actions.refreshForge3d();
    // Mount-time probe only; `actions` is intentionally not a dependency.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  if (!state) return null;

  const report = handoff?.report;
  const counts = {
    full: report?.objects.filter((object) => object.level === "full").length ?? 0,
    partial: report?.objects.filter((object) => object.level === "partial").length ?? 0,
    unsupported: report?.objects.filter((object) => object.level === "unsupported").length ?? 0,
  };
  const canLaunch = Boolean(engine?.available) && bridgeKind === "tauri";
  const running = Boolean(forgeStatus?.running);

  return (
    <div className="forge3d">
      <header className="forge3d__header">
        <div>
          <h1>Forge3D workspace</h1>
          <p className="hint">
            The viewer opens as its own window. Studio keeps the project, the
            canvas, and every edit; nothing here is destructive.
          </p>
        </div>
        <div className="forge3d__actions">
          <button
            type="button"
            className="button button--primary"
            disabled={!canLaunch || running}
            title={
              canLaunch
                ? "Open the scene in the Forge3D viewer"
                : (engine?.detail ?? "Probing the engine…")
            }
            onClick={() => void actions.openForge3d()}
          >
            <ImportIcon size={14} /> Open in Forge3D
          </button>
          <button
            type="button"
            className="button"
            disabled={!running}
            title="Send the project's camera and sun to the running viewer"
            onClick={() => void actions.syncForge3dCamera()}
          >
            <CameraIcon size={14} /> Send camera
          </button>
          <button
            type="button"
            className="button"
            disabled={!running}
            onClick={() => void actions.stopForge3d()}
          >
            Stop viewer
          </button>
          <button
            type="button"
            className="button"
            onClick={() => void actions.setWorkspace("studio")}
          >
            Back to Studio
          </button>
        </div>
      </header>

      <div className={`engine-banner ${engine?.available ? "is-ready" : "is-missing"}`}>
        {engine?.available ? (
          <>
            <strong>Engine ready</strong>
            <span>python {engine.python}</span>
            {engine.packageVersion && <span>forge3d {engine.packageVersion}</span>}
            {engine.viewerBinary && (
              <span className="mono" title={engine.viewerBinary}>
                {engine.viewerBinary}
              </span>
            )}
          </>
        ) : (
          <>
            <AlertIcon size={14} />
            <strong>Engine unavailable</strong>
            <span>{engine?.detail ?? "Probing for a Forge3D engine…"}</span>
          </>
        )}
      </div>

      <div className="forge3d__grid">
        <section className="forge3d__card">
          <header>
            <h2>Capability report</h2>
            {report && (
              <span className="mono">
                {counts.full} supported · {counts.partial} partial · {counts.unsupported}{" "}
                unsupported
              </span>
            )}
          </header>

          {!handoff ? (
            <p className="hint">Open a project to compute what Forge3D can receive.</p>
          ) : (
            <>
              <table className="report-table">
                <thead>
                  <tr>
                    <th>Object</th>
                    <th>Kind</th>
                    <th>Forge3D</th>
                  </tr>
                </thead>
                <tbody>
                  {report?.objects.map((object) => (
                    <tr key={object.id} title={object.detail}>
                      <td>{object.name}</td>
                      <td className="mono">{object.kind}</td>
                      <td>
                        <span className={`level-badge level-badge--${object.level}`}>
                          {LEVEL_LABEL[object.level]}
                        </span>
                      </td>
                    </tr>
                  ))}
                </tbody>
              </table>

              {report && report.degradations.length > 0 && (
                <ul className="degradations">
                  {report.degradations.map((note) => (
                    <li key={note}>{note}</li>
                  ))}
                </ul>
              )}

              <p className="hint">
                Camera sync: {report?.supportsCameraSync ? "available" : "no camera in scene"}
                {" · "}
                {handoff.assets.length} asset
                {handoff.assets.length === 1 ? "" : "s"} in this handoff
                {handoff.camera ? ` · ${handoff.camera.fovDeg.toFixed(0)}° field of view` : ""}
              </p>
              <p className="hint mono">handoff {handoff.idempotencyKey.slice(-24)}</p>
            </>
          )}
        </section>

        <section className="forge3d__card">
          <header>
            <h2>Viewer</h2>
            {forgeStatus && <ViewerStatusLine status={forgeStatus} />}
          </header>

          {forgeStatus?.error && (
            <p className="forge3d__error">{forgeStatus.error}</p>
          )}

          <dl className="readout readout--compact">
            <dt>Payload</dt>
            <dd className="mono" title={forgeStatus?.payloadPath ?? undefined}>
              {forgeStatus?.payloadPath?.split(/[\\/]/).pop() ?? "—"}
            </dd>
            <dt>Handoff</dt>
            <dd className="mono">
              {forgeStatus?.lastHandoffKey?.slice(-12) ?? "—"}
            </dd>
          </dl>

          <h3 className="forge3d__subhead">Diagnostics</h3>
          <pre className="diagnostics">
            {(forgeStatus?.diagnostics ?? []).length === 0
              ? "No viewer output yet."
              : forgeStatus?.diagnostics.slice(-30).join("\n")}
          </pre>
        </section>
      </div>
    </div>
  );
}
