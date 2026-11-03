import { createMockBridge } from "./mock";
import type { StudioBridge } from "./index";
import { tauriBridge } from "./tauri";

const isTauri =
  typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;

export const bridge: StudioBridge = isTauri ? tauriBridge : createMockBridge();

export type { StudioBridge };
export * from "./types";
