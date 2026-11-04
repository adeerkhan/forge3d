import type { RenderView } from "../bridge/types";

/**
 * The latest mapping of the viewport camera into the Forge3D renderer's
 * frame. The viewport writes this every frame; the render actions read it
 * when a frame or still is requested, so the engine reproduces the view the
 * user is actually looking at.
 *
 * `null` when the scene has no terrain to render (the renderer is a terrain
 * pipeline) — callers fall back to the shell's default framing.
 */
export const viewStore: { current: RenderView | null } = { current: null };
