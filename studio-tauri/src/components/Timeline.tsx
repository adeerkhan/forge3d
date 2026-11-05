import { useCallback, useEffect, useRef, useState } from "react";
import { open as openDialog } from "@tauri-apps/plugin-dialog";

import { bridge } from "../bridge/env";
import type { Channel, ChannelKeys, Interpolation, SceneObject } from "../bridge/types";
import { useSession } from "../state/session-context";
import { NumberField } from "./fields";
import { frameStepMs, nextPlayheadFrame, tickClock } from "../state/playback";

/** Channels the Key button writes for the selected object. */
function keyableChannels(object: SceneObject): Channel[] {
  const transform: Channel[] = [
    "translationX",
    "translationY",
    "translationZ",
    "rotationX",
    "rotationY",
    "rotationZ",
    "scaleX",
    "scaleY",
    "scaleZ",
  ];
  return object.kind.type === "camera" ? [...transform, "fovDeg"] : transform;
}

/** The object's current value for a channel (what "Key" captures). */
function currentValue(object: SceneObject, channel: Channel): number | null {
  const t = object.transform;
  switch (channel) {
    case "translationX":
      return t.translation.x;
    case "translationY":
      return t.translation.y;
    case "translationZ":
      return t.translation.z;
    case "rotationX":
      return t.rotationDeg.x;
    case "rotationY":
      return t.rotationDeg.y;
    case "rotationZ":
      return t.rotationDeg.z;
    case "scaleX":
      return t.scale.x;
    case "scaleY":
      return t.scale.y;
    case "scaleZ":
      return t.scale.z;
    case "fovDeg":
      return object.kind.type === "camera" ? object.kind.fovDeg : null;
  }
}

/**
 * The animation timeline (Phase 6): playback, scrubbing, keyframing, and
 * frame-sequence export.
 *
 * Playback and export share one evaluation function in studio-core — this
 * component only ever *asks* for a frame (`setPlayhead`) or asks the shell to
 * render one (`exportFrames`), so what you scrub is what renders.
 */
export function Timeline() {
  const { state, actions, exporting, setTimelineOpen } = useSession();
  const [playing, setPlaying] = useState(false);
  const [interpolation, setInterpolation] = useState<Interpolation>("linear");

  const project = state?.project;
  const timeline = project?.timeline;
  const selection = project?.selection ?? [];
  const selected =
    project && selection.length === 1
      ? project.scene.objects.find((object) => object.id === selection[0])
      : undefined;

  const frame = timeline?.currentFrame ?? 0;
  const length = timeline?.length ?? 96;
  const frameRate = timeline?.frameRate ?? 24;

  // Playback: advance one frame per 1/frameRate seconds, wrapping at the end.
  // Each step is a setPlayhead IPC — the same evaluation the export uses.
  //
  // The effect depends on `playing` ONLY. `timeline` and `actions` get a new
  // identity on every snapshot, so depending on them restarted the loop (and
  // reset its clock) on every frame the playhead produced: the accumulator
  // never reached one frame's worth of time and playback froze. The clock
  // arithmetic lives in `state/playback.ts`, pinned by its own test.
  const playheadRef = useRef(frame);
  playheadRef.current = frame;
  const actionsRef = useRef(actions);
  actionsRef.current = actions;
  const settingsRef = useRef({ frameRate, length });
  settingsRef.current = { frameRate, length };
  /** A scrub during playback restarts the loop's counter from the new value. */
  const resync = useRef<number | null>(null);
  useEffect(() => {
    if (!playing) return;
    let handle = 0;
    let last = performance.now();
    let rest = 0;
    let inFlight = false;
    let next = resync.current ?? Math.round(playheadRef.current);
    resync.current = null;
    const tick = (now: number) => {
      handle = requestAnimationFrame(tick);
      const clock = tickClock(rest, now - last, frameStepMs(settingsRef.current.frameRate));
      last = now;
      rest = clock.rest;
      // One setPlayhead in flight at a time: the shell answers out of order
      // otherwise, and a fast machine could stack frames on a slow one.
      if (!clock.due || inFlight) return;
      if (resync.current !== null) {
        next = resync.current;
        resync.current = null;
      }
      next = nextPlayheadFrame(next, settingsRef.current.length);
      inFlight = true;
      void actionsRef.current.setPlayhead(next).finally(() => {
        inFlight = false;
      });
    };
    handle = requestAnimationFrame(tick);
    return () => {
      cancelAnimationFrame(handle);
      resync.current = null;
    };
  }, [playing]);

  // Stop playback when the project closes.
  useEffect(() => {
    if (!state) setPlaying(false);
  }, [state]);

  const scrub = useCallback(
    (next: number) => {
      resync.current = next;
      void actions.setPlayhead(next);
    },
    [actions],
  );

  const commitConfig = useCallback(
    (patch: { frameRate?: number; length?: number }) => {
      void actions.apply({
        type: "setTimelineConfig",
        frameRate: patch.frameRate ?? frameRate,
        length: patch.length ?? length,
      });
    },
    [actions, frameRate, length],
  );

  /** Insert keys at the playhead from the object's current values. */
  const keySelected = useCallback(() => {
    if (!selected || !timeline) return;
    const frameIndex = Math.round(frame);
    const existing =
      timeline.tracks.find((track) => track.objectId === selected.id)?.channels ?? [];
    const merged: ChannelKeys[] = keyableChannels(selected).flatMap((channel) => {
      const value = currentValue(selected, channel);
      if (value === null) return [];
      const keys = (existing.find((entry) => entry.channel === channel)?.keys ?? []).filter(
        (key) => key.frame !== frameIndex,
      );
      keys.push({ frame: frameIndex, value, interpolation });
      keys.sort((a, b) => a.frame - b.frame);
      return [{ channel, keys }];
    });
    void actions.apply({ type: "setChannelKeys", id: selected.id, channels: merged });
  }, [selected, timeline, frame, interpolation, actions]);

  const clearSelected = useCallback(() => {
    if (!selected) return;
    void actions.apply({ type: "setChannelKeys", id: selected.id, channels: [] });
  }, [selected, actions]);

  /** Pick a folder and hand it to the session's export loop. */
  const pickAndExport = useCallback(async () => {
    const directory = await openDialog({ directory: true, multiple: false });
    if (typeof directory !== "string") return;
    await actions.exportFrames(directory);
  }, [actions]);

  if (!state || !timeline) return null;

  const track = selected
    ? timeline.tracks.find((entry) => entry.objectId === selected.id)
    : undefined;
  // Union of key frames across the selected object's channels, for the dots.
  const keyFrames = [
    ...new Set((track?.channels ?? []).flatMap((entry) => entry.keys.map((key) => key.frame))),
  ].sort((a, b) => a - b);
  const busy = exporting !== null;

  return (
    <div className="timeline">
      <div className="timeline__transport">
        <button
          type="button"
          className="timeline__play"
          aria-label={playing ? "Stop playback" : "Start playback"}
          disabled={busy}
          onClick={() => setPlaying((value) => !value)}
        >
          {playing ? "■" : "▶"}
        </button>
        <input
          className="timeline__scrub"
          type="range"
          min={0}
          max={length}
          step={1}
          value={Math.round(frame)}
          disabled={busy}
          onChange={(event) => scrub(Number(event.target.value))}
          aria-label="Playhead"
        />
        <span className="timeline__frame mono">
          {Math.round(frame)} / {length}
        </span>
        <button
          type="button"
          className="text-button"
          title="Hide the timeline (T)"
          onClick={() => setTimelineOpen(false)}
        >
          Hide
        </button>
      </div>

      {keyFrames.length > 0 && (
        <div className="timeline__keys" aria-hidden="true">
          {keyFrames.map((keyFrame) => (
            <span
              key={keyFrame}
              className="timeline__key-dot"
              style={{ left: `${length === 0 ? 0 : (keyFrame / length) * 100}%` }}
              title={`key @ ${keyFrame}`}
            />
          ))}
        </div>
      )}

      <div className="timeline__config">
        <NumberField
          label="FPS"
          value={timeline.frameRate}
          step={1}
          min={1}
          max={240}
          onCommit={(frameRate) => commitConfig({ frameRate })}
        />
        <NumberField
          label="Frames"
          value={timeline.length}
          step={1}
          min={1}
          max={100_000}
          onCommit={(length) => commitConfig({ length })}
        />
        <label className="field">
          <span className="field-label">Interpolation</span>
          <select
            className="input input--select"
            value={interpolation}
            onChange={(event) => setInterpolation(event.target.value as Interpolation)}
          >
            <option value="linear">Linear</option>
            <option value="constant">Constant</option>
            <option value="smooth">Smooth</option>
          </select>
        </label>
      </div>

      <div className="timeline__actions">
        <button
          type="button"
          className="button"
          disabled={!selected || selected.kind.type === "terrain" || busy}
          title={
            selected
              ? `Key ${selected.name} at frame ${Math.round(frame)} with its current values`
              : "Select one object to key"
          }
          onClick={keySelected}
        >
          Key selected
        </button>
        <button
          type="button"
          className="button"
          disabled={!track || busy}
          onClick={clearSelected}
        >
          Clear keys
        </button>
        <button
          type="button"
          className="button button--primary"
          disabled={bridge.kind !== "tauri" || busy}
          title={
            bridge.kind === "tauri"
              ? `Render frames 0..${length} at ${project.render.width}×${project.render.height} into a chosen folder`
              : "Frame export needs the desktop shell"
          }
          onClick={() => void pickAndExport()}
        >
          {exporting ? `Exporting ${exporting.done}/${exporting.total}…` : "Export frames…"}
        </button>
        {busy && (
          <button type="button" className="button" onClick={actions.cancelExport}>
            Stop
          </button>
        )}
      </div>

      <p className="hint">
        Playback and Export evaluate the same timeline in studio-core, so the
        sequence matches what you scrub. Keying captures the selected object's
        current values at the playhead; terrain cannot be animated (its
        transform is georeferenced).
      </p>
    </div>
  );
}
