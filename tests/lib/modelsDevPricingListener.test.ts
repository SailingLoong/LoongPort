import { beforeEach, describe, expect, it, vi } from "vitest";

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
  queryClient: { invalidateQueries: mocks.invalidateQueries },
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

describe("models.dev pricing listener bootstrap", () => {
  beforeEach(() => {
    vi.resetModules();
    mocks.invalidateQueries.mockReset();
    mocks.invoke.mockResolvedValue(null);
    mocks.listen.mockReset();
    mocks.reportFrontendError.mockReset();
    mocks.render.mockReset();
  });

  it("reports an asynchronous listener-registration rejection", async () => {
    const error = new Error("listener registration failed");
    const rejectedRegistration = Promise.reject(error);
    // Prevent the injected rejection from becoming a test-runner global error;
    // the assertion below verifies that the bootstrap adds its own handler.
    void rejectedRegistration.catch(() => undefined);
    mocks.listen.mockImplementation((eventName: string) =>
      eventName === "models-dev-pricing-updated"
        ? rejectedRegistration
        : Promise.resolve(() => undefined),
    );

    await import("../../src/main");

    await vi.waitFor(() => {
      expect(mocks.reportFrontendError).toHaveBeenCalledWith(
        "models_dev_pricing_updated_listener",
        error,
      );
    });
  });
});
