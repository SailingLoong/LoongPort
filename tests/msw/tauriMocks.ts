import { vi } from "vitest";
import { server } from "./server";

const TAURI_ENDPOINT = "http://tauri.local";

// 曾经这里用 cross-fetch 的 ponyfill 全局替换 fetch/Headers/Request/Response。
// MSW 3 的 interceptors 克隆响应时要求原生 WHATWG Response（流式 body 有
// getReader），ponyfill 的 Response 没有，会成片抛 "body.getReader is not a
// function"。Node 22+ 原生自带全套 WHATWG fetch，MSW 3 直接拦原生 fetch，
// 不再需要任何替换。

vi.mock("@tauri-apps/api/core", () => ({
  // main.tsx 的 initializeWindowActivity（上游）会探测宿主环境；测试里恒为非 Tauri。
  isTauri: () => false,
  invoke: async (command: string, payload: Record<string, unknown> = {}) => {
    const response = await fetch(`${TAURI_ENDPOINT}/${command}`, {
      method: "POST",
      headers: {
        "Content-Type": "application/json",
      },
      body: JSON.stringify(payload ?? {}),
    });

    if (!response.ok) {
      const text = await response.text();
      throw new Error(text || `Invoke failed for ${command}`);
    }

    const text = await response.text();
    if (!text) return undefined;
    try {
      return JSON.parse(text);
    } catch {
      return text;
    }
  },
}));

const listeners = new Map<string, Set<(event: { payload: unknown }) => void>>();

const ensureListenerSet = (event: string) => {
  if (!listeners.has(event)) {
    listeners.set(event, new Set());
  }
  return listeners.get(event)!;
};

export const emitTauriEvent = (event: string, payload: unknown) => {
  const handlers = listeners.get(event);
  handlers?.forEach((handler) => handler({ payload }));
};

vi.mock("@tauri-apps/api/event", () => ({
  listen: async (
    event: string,
    handler: (event: { payload: unknown }) => void,
  ) => {
    const set = ensureListenerSet(event);
    set.add(handler);
    return () => {
      set.delete(handler);
    };
  },
}));

// Ensure the MSW server is referenced so tree shaking doesn't remove imports
void server;

vi.mock("@tauri-apps/api/path", () => ({
  homeDir: async () => "/home/mock",
  join: async (...segments: string[]) => segments.join("/"),
}));
