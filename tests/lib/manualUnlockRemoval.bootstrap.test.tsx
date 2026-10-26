import { beforeEach, describe, expect, it, vi } from "vitest";
import { isValidElement, type ReactNode } from "react";
import App from "@/App";

const mocks = vi.hoisted(() => ({
  invalidateQueries: vi.fn(),
  invoke: vi.fn(),
  listen: vi.fn(),
  reportFrontendError: vi.fn(),
  render: vi.fn(),
}));

vi.mock("@/App", () => ({ default: () => null }));
vi.mock("@/components/DatabaseUpgrade", () => ({
  DatabaseUpgrade: () => null,
}));
vi.mock("@/components/FrontendErrorBoundary", () => ({
  FrontendErrorBoundary: ({ children }: { children: React.ReactNode }) =>
    children,
}));
vi.mock("@/components/theme-provider", () => ({
  ThemeProvider: ({ children }: { children: React.ReactNode }) => children,
}));
vi.mock("@/components/ui/sonner", () => ({ Toaster: () => null }));
vi.mock("@/contexts/UpdateContext", () => ({
  UpdateProvider: ({ children }: { children: React.ReactNode }) => children,
}));
vi.mock("@/lib/frontendLogger", () => ({
  installConsoleLogBridge: vi.fn(),
  installGlobalErrorHandlers: vi.fn(),
  reportFrontendError: (...args: unknown[]) =>
    mocks.reportFrontendError(...args),
}));
vi.mock("@/lib/query", () => ({
  // 该套件动态 import src/main，bootstrap 会经 queryClient 拉一次保存的语言
  queryClient: {
    invalidateQueries: mocks.invalidateQueries,
    fetchQuery: async () => ({ language: null }),
  },
}));
vi.mock("@/lib/query/usage", () => ({
  usageKeys: {
    all: ["usage"],
    modelsDevSyncConfig: () => ["models-dev-sync-config"],
  },
}));
vi.mock("@tauri-apps/api/core", () => ({
  // main.tsx（上游 initializeWindowActivity）会探测宿主环境
  isTauri: () => false,
  invoke: (...args: unknown[]) => mocks.invoke(...args),
}));
vi.mock("@tauri-apps/api/event", () => ({
  listen: (...args: unknown[]) => mocks.listen(...args),
}));
vi.mock("@tauri-apps/plugin-dialog", () => ({ message: vi.fn() }));
vi.mock("@tauri-apps/plugin-process", () => ({ exit: vi.fn() }));
vi.mock("react-dom/client", () => ({
  default: { createRoot: () => ({ render: mocks.render }) },
}));

function containsType(node: ReactNode, type: unknown): boolean {
  if (Array.isArray(node))
    return node.some((child) => containsType(child, type));
  if (!isValidElement<{ children?: ReactNode }>(node)) return false;
  return node.type === type || containsType(node.props.children, type);
}
describe("manual unlock removal: actual startup entry", () => {
  beforeEach(() => {
    vi.resetModules();
    mocks.render.mockReset();
    mocks.listen.mockResolvedValue(() => undefined);
    mocks.invoke.mockReset();
  });
  it.each(["secret_locked", "secret_initialization_failed"])(
    "%s must mount the existing App, not a full-screen unlock gate",
    async (kind) => {
      mocks.invoke.mockImplementation(async (command: string) =>
        command === "get_init_error" ? { kind, error: "secret.locked" } : null,
      );
      await import("../../src/main");
      await vi.waitFor(() => expect(mocks.render).toHaveBeenCalled());
      const tree = mocks.render.mock.calls.at(-1)![0];
      const { SecretUnlock } = await import("@/components/SecretUnlock");
      // Both expectations express the requested product contract. Current code
      // must fail here; do not replace them with an assertion that the lock exists.
      expect.soft(containsType(tree, App)).toBe(true);
      expect.soft(containsType(tree, SecretUnlock)).toBe(false);
      expect(mocks.invoke).not.toHaveBeenCalledWith(
        "unlock_secret_vault",
        expect.anything(),
      );
    },
  );
  it("retains the normal App entry when initialization has no error", async () => {
    mocks.invoke.mockResolvedValue(null);
    await import("../../src/main");
    await vi.waitFor(() => expect(mocks.render).toHaveBeenCalled());
    expect(containsType(mocks.render.mock.calls.at(-1)![0], App)).toBe(true);
  });
});

it("retains the original database-version upgrade boundary", async () => {
  vi.resetModules();
  mocks.render.mockReset();
  mocks.listen.mockResolvedValue(() => undefined);
  mocks.invoke.mockImplementation(async (command: string) =>
    command === "get_init_error"
      ? { kind: "db_version_too_new", error: "newer database" }
      : null,
  );
  await import("../../src/main");
  await vi.waitFor(() => expect(mocks.render).toHaveBeenCalled());
  const { DatabaseUpgrade } = await import("@/components/DatabaseUpgrade");
  const tree = mocks.render.mock.calls.at(-1)![0];
  expect(containsType(tree, DatabaseUpgrade)).toBe(true);
  expect(containsType(tree, App)).toBe(false);
});

it("does not relabel an unrelated configuration failure as credential-unavailable", async () => {
  vi.resetModules();
  mocks.render.mockReset();
  mocks.listen.mockResolvedValue(() => undefined);
  mocks.invoke.mockImplementation(async (command: string) =>
    command === "get_init_error"
      ? {
          kind: "config_error",
          path: "synthetic-config.json",
          error: "invalid synthetic configuration",
        }
      : null,
  );
  await import("../../src/main");
  const { exit } = await import("@tauri-apps/plugin-process");
  await vi.waitFor(() => expect(exit).toHaveBeenCalledWith(1));
  expect(mocks.render).not.toHaveBeenCalled();
});
