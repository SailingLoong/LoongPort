import { StrictMode, type PropsWithChildren } from "react";
import { act, renderHook } from "@testing-library/react";
import { afterEach, beforeAll, describe, expect, it, vi } from "vitest";
import { useCcSwitchImport } from "@/hooks/useCcSwitchImport";
import { ccSwitchImportApi } from "@/lib/api";
import type {
  CcSwitchImportPreview,
  CcSwitchImportReport,
} from "@/lib/api/ccSwitchImport";
import { server } from "../msw/server";

const { toastError } = vi.hoisted(() => ({ toastError: vi.fn() }));
vi.mock("sonner", () => ({ toast: { error: toastError } }));

const preview: CcSwitchImportPreview = {
  sourceExists: true,
  canImport: true,
  providers: {
    willImport: 1,
    skipped: [],
    mergedToRelay: [],
    cannotFingerprint: 0,
  },
  mcpServers: 0,
  prompts: 0,
  skills: 0,
  notes: [],
};
const report: CcSwitchImportReport = {
  success: true,
  backupId: "backup-1",
  providersImported: 1,
  providersSkipped: [],
  relaysMerged: [],
  mcpImported: 0,
  promptsImported: 0,
  skillsImported: 0,
  warnings: [],
};

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (reason: Error) => void;
  const promise = new Promise<T>((resolvePromise, rejectPromise) => {
    resolve = resolvePromise;
    reject = rejectPromise;
  });
  return { promise, resolve, reject };
}

// Reproduce the CI lifecycle deterministically: the component is unmounted,
// then its environment disappears before the native request settles.
async function afterWindowTeardown<T>(settle: () => Promise<T>): Promise<T> {
  const descriptor = Object.getOwnPropertyDescriptor(globalThis, "window")!;
  Reflect.deleteProperty(globalThis, "window");
  try {
    return await settle();
  } finally {
    Object.defineProperty(globalThis, "window", descriptor);
  }
}

beforeAll(() => {
  // A missing fixture must fail here instead of issuing a real DNS request.
  server.close();
  // MSW 3：onUnhandledRequest → onUnhandledFrame，语义不变。
  server.listen({ onUnhandledFrame: "error" });
});

afterEach(() => {
  vi.restoreAllMocks();
});

describe("useCcSwitchImport", () => {
  it("provides an offline no-source preview through the default MSW handlers", async () => {
    await expect(ccSwitchImportApi.getPreview()).resolves.toEqual({
      sourceExists: false,
      canImport: false,
      providers: {
        willImport: 0,
        skipped: [],
        mergedToRelay: [],
        cannotFingerprint: 0,
      },
      mcpServers: 0,
      prompts: 0,
      skills: 0,
      notes: [],
    });
  });

  it("loads a preview after StrictMode replays mount effects", async () => {
    vi.spyOn(ccSwitchImportApi, "getPreview").mockResolvedValue(preview);
    const { result } = renderHook(() => useCcSwitchImport(), {
      wrapper: ({ children }: PropsWithChildren) => (
        <StrictMode>{children}</StrictMode>
      ),
    });

    await act(async () => {
      await result.current.loadPreview();
    });

    expect(result.current.preview).toEqual(preview);
    expect(result.current.status).toBe("idle");
    expect(result.current.errorMessage).toBeNull();
  });

  it("shows preview failures while mounted", async () => {
    vi.spyOn(ccSwitchImportApi, "getPreview").mockRejectedValue(
      new Error("preview unavailable"),
    );
    vi.spyOn(console, "error").mockImplementation(() => {});
    const { result } = renderHook(() => useCcSwitchImport());

    await act(async () => {
      await result.current.loadPreview();
    });

    expect(result.current.status).toBe("error");
    expect(result.current.errorMessage).toBe("preview unavailable");
  });

  it.each(["resolve", "reject"] as const)(
    "settles a preview that %ss after unmount without accessing the removed window",
    async (outcome) => {
      const request = deferred<CcSwitchImportPreview>();
      vi.spyOn(ccSwitchImportApi, "getPreview").mockReturnValue(
        request.promise,
      );
      vi.spyOn(console, "error").mockImplementation(() => {});
      const { result, unmount } = renderHook(() => useCcSwitchImport());
      let pending!: Promise<void>;
      act(() => {
        pending = result.current.loadPreview();
      });
      expect(result.current.status).toBe("loading");
      unmount();

      await expect(
        afterWindowTeardown(() => {
          if (outcome === "resolve") request.resolve(preview);
          else request.reject(new Error("late preview failure"));
          return pending;
        }),
      ).resolves.toBeUndefined();
    },
  );

  it("keeps the completed import report while mounted", async () => {
    vi.spyOn(ccSwitchImportApi, "import").mockResolvedValue(report);
    const { result } = renderHook(() => useCcSwitchImport());

    await act(async () => {
      await expect(result.current.runImport()).resolves.toEqual(report);
    });

    expect(result.current.report).toEqual(report);
    expect(result.current.status).toBe("success");
    expect(result.current.isImporting).toBe(false);
  });

  it("shows import failures while mounted", async () => {
    vi.spyOn(ccSwitchImportApi, "import").mockRejectedValue(
      new Error("import unavailable"),
    );
    vi.spyOn(console, "error").mockImplementation(() => {});
    const { result } = renderHook(() => useCcSwitchImport());

    await act(async () => {
      await expect(result.current.runImport()).resolves.toBeNull();
    });

    expect(result.current.status).toBe("error");
    expect(result.current.errorMessage).toBe("import unavailable");
    expect(result.current.isImporting).toBe(false);
    expect(toastError).toHaveBeenCalledWith(
      expect.stringContaining("import unavailable"),
    );
  });

  it.each(["resolve", "reject"] as const)(
    "settles an import that %ss after unmount without accessing the removed window",
    async (outcome) => {
      const request = deferred<CcSwitchImportReport>();
      vi.spyOn(ccSwitchImportApi, "import").mockReturnValue(request.promise);
      vi.spyOn(console, "error").mockImplementation(() => {});
      const { result, unmount } = renderHook(() => useCcSwitchImport());
      let pending!: Promise<CcSwitchImportReport | null>;
      act(() => {
        pending = result.current.runImport();
      });
      expect(result.current.isImporting).toBe(true);
      unmount();

      await expect(
        afterWindowTeardown(() => {
          if (outcome === "resolve") request.resolve(report);
          else request.reject(new Error("late import failure"));
          return pending;
        }),
      ).resolves.toEqual(outcome === "resolve" ? report : null);
      expect(toastError).not.toHaveBeenCalled();
    },
  );
});
