import React from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import {
  act,
  cleanup,
  render,
  renderHook,
  waitFor,
} from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { useVendorServiceConfiguration } from "@/components/relay/onboarding/useServiceConfiguration";
import { vendorApi } from "@/lib/api/vendor";
import { applicationRoutingApi } from "@/lib/api/applicationRouting";
import { applicationOverviewApi } from "@/lib/api/applicationOverview";
import { serviceOnboardingApi } from "@/lib/api/serviceOnboarding";

const runtime = vi.hoisted(() => ({
  completed: true,
  choices: [
    { app: "codex", id: "plan", providerId: "provider", name: "Plan" },
    { app: "claude", id: "plan2", providerId: "provider2", name: "Plan 2" },
  ],
}));

vi.mock("react-i18next", () => ({
  useTranslation: () => ({ t: (key: string) => key }),
}));
vi.mock("sonner", () => ({ toast: { warning: vi.fn() } }));
vi.mock("@/components/relay/onboarding/useServiceOnboarding", () => ({
  useVendorConfigurationChoices: () => ({
    data: runtime.choices,
  }),
  useServiceOnboardingStatus: () => ({
    data: { completed: runtime.completed },
  }),
  serviceOnboardingKey: ["onboarding"],
}));

const account = { rowId: 1, name: "Vendor" };
const confirmation = {
  status: "confirmationRequired",
  targetName: "Plan",
  quitRequired: false,
  catalog: { choice: "keep", revision: "r" },
} as const;
const snapshot = {
  configurationRevision: "rev1",
  isAdditive: false,
  configurations: [
    {
      selection: { kind: "vendor", rowId: 1, planId: "plan" },
      presentation: { isCurrent: true },
    },
  ],
};
function setup() {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  const onDone = vi.fn();
  const wrapper = ({ children }: { children: React.ReactNode }) => (
    <QueryClientProvider client={client}>{children}</QueryClientProvider>
  );
  return {
    ...renderHook(
      () => useVendorServiceConfiguration(account, "codex", onDone),
      { wrapper },
    ),
    onDone,
  };
}
beforeEach(() => {
  vi.restoreAllMocks();
  runtime.completed = true;
  const records = new Map<string, any>();
  vi.spyOn(vendorApi, "prepareConnection").mockImplementation(async (input) => {
    const record = {
      operationId: input.operationId,
      app: input.app,
      rowId: input.rowId,
      plan: input.plan,
      providerId: runtime.choices.find(
        (c) => c.app === input.app && c.id === input.plan,
      )?.providerId,
      receiptRevision: 1,
      status: "prepared",
      operationState: "prepared",
      completedConfigurationRevision: null,
    };
    records.set(input.operationId, record);
    return record as any;
  });
  vi.spyOn(vendorApi, "queryConnection").mockImplementation(
    async (app, operationId) => {
      if (operationId === null) return { status: "none" };
      const calls = vi.mocked(vendorApi.switchPreparedConnection).mock;
      const index =
        calls?.calls
          .map(([input]) => input.operationId === operationId)
          .lastIndexOf(true) ?? -1;
      if (index >= 0) {
        try {
          const outcome = await calls.results[index].value;
          if (outcome?.status === "switched")
            return {
              ...records.get(operationId),
              status: "completed",
              operationState: "completed",
              completedConfigurationRevision: "rev1",
            };
        } catch {
          /* lost outcome stays unknown */
        }
      }
      return { status: "unknown", operationId, app };
    },
  );
  vi.spyOn(vendorApi, "previewConnectionReview").mockImplementation(
    async (input) => ({
      ...input,
      status: "reviewRequired",
      receiptRevision: 1,
      observedRevision: "observed",
    }),
  );
  vi.spyOn(vendorApi, "reviewConnection").mockImplementation(async (input) => ({
    ...records.get(input.operationId),
    status: "failedBeforeWrite",
    operationState: "failedBeforeWrite",
    receiptRevision: 2,
  }));
  vi.spyOn(vendorApi, "previewConnectionCatalog").mockResolvedValue(
    confirmation as any,
  );
  vi.spyOn(vendorApi, "previewNewConnectionIntent").mockImplementation(
    async (input) => ({
      ...input,
      status: "newIntentReviewRequired",
      receiptRevision: 1,
      originalRowId: 1,
      originalPlan: input.targetPlan,
      targetProviderId: runtime.choices.find(
        (c) => c.app === input.app && c.id === input.targetPlan,
      )!.providerId,
      reviewToken: "a".repeat(64),
    }),
  );
  vi.spyOn(vendorApi, "confirmNewConnectionIntent").mockImplementation(
    async (input) => ({ status: "newIntentPermitted", app: input.app }),
  );

  runtime.choices = [
    { app: "codex", id: "plan", providerId: "provider", name: "Plan" },
    { app: "claude", id: "plan2", providerId: "provider2", name: "Plan 2" },
  ];
});
afterEach(() => cleanup());

describe("vendor setup unknown-result safety", () => {
  it("does not replay a switch whose write completed before rejection", async () => {
    let writes = 0;
    vi.spyOn(vendorApi, "switchPreparedConnection").mockImplementation(
      async () => {
        writes++;
        throw new Error("response lost after write");
      },
    );
    vi.spyOn(applicationOverviewApi, "state").mockResolvedValue(
      snapshot as any,
    );
    const { result } = setup();
    await act(async () => {
      await result.current.finish();
    });
    expect(result.current.results.codex?.state).toBe("unverified");
    await act(async () => {
      await result.current.finish();
    });
    expect(writes).toBe(1);
  });

  it("does not replay a confirmed catalog apply whose write completed before rejection", async () => {
    let writes = 0;
    vi.spyOn(vendorApi, "switchPreparedConnection")
      .mockResolvedValueOnce(confirmation as any)
      .mockImplementation(async () => {
        writes++;
        throw new Error("post-write read failed");
      });
    const { result } = setup();
    let run: Promise<void>;
    act(() => {
      run = result.current.finish();
    });
    await waitFor(() => expect(result.current.confirmation).toBe("Plan"));
    await act(async () => {
      result.current.resolveConfirmation(false, {
        choice: "keep",
        revision: "r",
      });
      await run!;
    });
    expect(result.current.results.codex?.state).toBe("unverified");
    await act(async () => {
      await result.current.finish();
    });
    expect(writes).toBe(1);
  });

  it("does not replay an uncertain write through a retained finish callback", async () => {
    const switcher = vi
      .spyOn(vendorApi, "switchPreparedConnection")
      .mockRejectedValue(new Error("unknown"));
    const { result } = setup();
    const originalFinish = result.current.finish;
    await act(async () => {
      await originalFinish();
    });
    await act(async () => {
      await originalFinish();
    });
    expect(switcher).toHaveBeenCalledTimes(1);
  });

  it("rejects a retained reapply callback that never displayed the uncertain result", async () => {
    const switcher = vi
      .spyOn(vendorApi, "switchPreparedConnection")
      .mockRejectedValue(new Error("unknown"));
    const { result } = setup();
    const oldReapply = result.current.reapply;
    await act(async () => {
      await result.current.finish();
    });
    await act(async () => {
      await oldReapply("codex");
    });
    await act(async () => {
      await result.current.finish();
    });
    expect(switcher).toHaveBeenCalledTimes(1);
    expect(result.current.results.codex?.state).toBe("unverified");
  });

  it("rechecks an earlier success and blocks a later uncertain target", async () => {
    const switcher = vi
      .spyOn(vendorApi, "switchPreparedConnection")
      .mockImplementation(async ({ app }) => {
        if (app === "codex") throw new Error("unknown");
        return { status: "switched", warnings: [] } as any;
      });
    const claudeSnapshot = {
      ...snapshot,
      configurations: [
        {
          ...snapshot.configurations[0],
          selection: { kind: "vendor", rowId: 1, planId: "plan2" },
        },
      ],
    };
    const read = vi
      .spyOn(applicationOverviewApi, "state")
      .mockResolvedValueOnce(claudeSnapshot as any)
      .mockResolvedValueOnce({
        ...claudeSnapshot,
        configurationRevision: "externally-changed",
      } as any);
    const { result } = setup();
    act(() => result.current.setSelection({ claude: "plan2" }));
    await act(async () => {
      await result.current.finish();
    });
    expect(result.current.results.claude?.state).toBe("success");
    expect(result.current.results.codex?.state).toBe("unverified");
    await act(async () => {
      await result.current.finish();
    });
    expect(result.current.results.claude?.state).toBe("changed");
    expect(result.current.results.codex?.state).toBe("unverified");
    expect(read).toHaveBeenCalledTimes(2);
    expect(switcher).toHaveBeenCalledTimes(2);
  });

  it("rejects callbacks retained across Activity hide and reveal", async () => {
    const switcher = vi
      .spyOn(vendorApi, "switchPreparedConnection")
      .mockRejectedValue(new Error("unknown"));
    const client = new QueryClient();
    let latest!: ReturnType<typeof useVendorServiceConfiguration>;
    function Owner() {
      latest = useVendorServiceConfiguration(account, "codex", vi.fn());
      return null;
    }
    function Tree({ visible }: { visible: boolean }) {
      return (
        <QueryClientProvider client={client}>
          <React.Activity mode={visible ? "visible" : "hidden"}>
            <Owner />
          </React.Activity>
        </QueryClientProvider>
      );
    }
    const view = render(<Tree visible />);
    await act(async () => {
      await latest.finish();
    });
    const oldReapply = latest.reapply;
    const oldFinish = latest.finish;
    view.rerender(<Tree visible={false} />);
    view.rerender(<Tree visible />);
    await act(async () => {
      await oldReapply("codex");
    });
    await act(async () => {
      await oldFinish();
    });
    await act(async () => {
      await latest.finish();
    });
    expect(switcher).toHaveBeenCalledTimes(1);
    await act(async () => {
      await latest.reapply("codex");
    });
    await act(async () => {
      await latest.confirmReview();
    });
    expect(switcher).toHaveBeenCalledTimes(2);
  });

  it("rejects a finish callback from the previous source app", async () => {
    const switcher = vi
      .spyOn(vendorApi, "switchPreparedConnection")
      .mockRejectedValue(new Error("unknown"));
    const client = new QueryClient();
    const wrapper = ({ children }: { children: React.ReactNode }) => (
      <QueryClientProvider client={client}>{children}</QueryClientProvider>
    );
    const { result, rerender } = renderHook(
      ({ app }: { app: "codex" | "claude" }) =>
        useVendorServiceConfiguration(account, app, vi.fn()),
      { initialProps: { app: "codex" as "codex" | "claude" }, wrapper },
    );
    const oldFinish = result.current.finish;
    rerender({ app: "claude" });
    await act(async () => {
      await oldFinish();
    });
    expect(switcher).not.toHaveBeenCalled();
    await act(async () => {
      await result.current.finish();
    });
    expect(switcher).toHaveBeenCalledTimes(1);
    expect(switcher.mock.calls[0][0].app).toBe("claude");
  });

  it("does not let an old confirmation answer resolve a new visible dialog", async () => {
    const switcher = vi
      .spyOn(vendorApi, "switchPreparedConnection")
      .mockResolvedValueOnce(confirmation as any)
      .mockResolvedValueOnce(confirmation as any)
      .mockResolvedValue({ status: "switched", warnings: [] } as any);
    const apply = vi.spyOn(applicationRoutingApi, "apply");
    vi.spyOn(applicationOverviewApi, "state").mockResolvedValue(
      snapshot as any,
    );
    const client = new QueryClient();
    let latest!: ReturnType<typeof useVendorServiceConfiguration>;
    function Owner() {
      latest = useVendorServiceConfiguration(account, "codex", vi.fn());
      return null;
    }
    function Tree({ visible }: { visible: boolean }) {
      return (
        <QueryClientProvider client={client}>
          <React.Activity mode={visible ? "visible" : "hidden"}>
            <Owner />
          </React.Activity>
        </QueryClientProvider>
      );
    }
    const view = render(<Tree visible />);
    let first!: Promise<void>;
    act(() => {
      first = latest.finish();
    });
    await waitFor(() => expect(latest.confirmation).toBe("Plan"));
    const oldAnswer = latest.resolveConfirmation;
    view.rerender(<Tree visible={false} />);
    await act(async () => {
      await first;
    });
    view.rerender(<Tree visible />);
    await act(async () => {
      await latest.reapply("codex");
    });
    let second!: Promise<void>;
    act(() => {
      second = latest.confirmReview();
    });
    await waitFor(() => expect(latest.confirmation).toBe("Plan"));
    act(() => oldAnswer(false, { choice: "keep", revision: "r" }));
    await act(async () => {
      await Promise.resolve();
    });
    expect(apply).not.toHaveBeenCalled();
    await act(async () => {
      latest.resolveConfirmation(false, { choice: "keep", revision: "r" });
      await second;
    });
    expect(apply).not.toHaveBeenCalled();
    expect(switcher).toHaveBeenCalledTimes(3);
  });

  it("does not reuse confirmation authority when an API reuses its response object", async () => {
    const switcher = vi
      .spyOn(vendorApi, "switchPreparedConnection")
      .mockResolvedValueOnce(confirmation as any)
      .mockResolvedValue({ status: "switched", warnings: [] } as any);
    const apply = vi
      .spyOn(vendorApi, "previewConnectionCatalog")
      .mockResolvedValue(confirmation as any);
    vi.spyOn(applicationOverviewApi, "state").mockResolvedValue(
      snapshot as any,
    );
    const { result } = setup();
    let pending!: Promise<void>;
    act(() => {
      pending = result.current.finish();
    });
    await waitFor(() => expect(result.current.confirmation).toBe("Plan"));
    const oldAnswer = result.current.resolveConfirmation;
    await act(async () => {
      oldAnswer(false, { choice: "keep", revision: "r" }, true);
    });
    await waitFor(() => expect(apply).toHaveBeenCalledTimes(1));
    act(() => oldAnswer(false, { choice: "keep", revision: "r" }));
    await act(async () => {
      await Promise.resolve();
    });
    expect(apply).toHaveBeenCalledTimes(1);
    await act(async () => {
      result.current.resolveConfirmation(false, {
        choice: "keep",
        revision: "r",
      });
      await pending;
    });
    expect(apply).toHaveBeenCalledTimes(1);
    expect(switcher).toHaveBeenCalledTimes(2);
  });

  it("retires an open confirmation when the source app changes", async () => {
    vi.spyOn(vendorApi, "switchPreparedConnection").mockResolvedValue(
      confirmation as any,
    );
    const client = new QueryClient();
    const wrapper = ({ children }: { children: React.ReactNode }) => (
      <QueryClientProvider client={client}>{children}</QueryClientProvider>
    );
    const { result, rerender } = renderHook(
      ({ app }: { app: "codex" | "claude" }) =>
        useVendorServiceConfiguration(account, app, vi.fn()),
      { initialProps: { app: "codex" as "codex" | "claude" }, wrapper },
    );
    let pending!: Promise<void>;
    act(() => {
      pending = result.current.finish();
    });
    await waitFor(() => expect(result.current.confirmation).toBe("Plan"));
    rerender({ app: "claude" });
    await waitFor(() => expect(result.current.busy).toBe(false));
    await act(async () => {
      await pending;
    });
    expect(result.current.confirmation).toBeNull();
    expect(result.current.results.codex?.state).toBe("unverified");
  });

  it("preserves an in-flight write as uncertain when the source changes", async () => {
    let release!: (value: any) => void;
    const switcher = vi
      .spyOn(vendorApi, "switchPreparedConnection")
      .mockImplementation(
        () =>
          new Promise((resolve) => {
            release = resolve;
          }),
      );
    const client = new QueryClient();
    const wrapper = ({ children }: { children: React.ReactNode }) => (
      <QueryClientProvider client={client}>{children}</QueryClientProvider>
    );
    const { result, rerender } = renderHook(
      ({ app }: { app: "codex" | "claude" }) =>
        useVendorServiceConfiguration(account, app, vi.fn()),
      { initialProps: { app: "codex" as "codex" | "claude" }, wrapper },
    );
    let pending!: Promise<void>;
    act(() => {
      pending = result.current.finish();
    });
    await waitFor(() =>
      expect(result.current.results.codex?.state).toBe("applying"),
    );
    rerender({ app: "claude" });
    expect(result.current.results.codex?.state).toBe("unverified");
    await act(async () => {
      release({ status: "switched", warnings: [] });
      await pending;
    });
    expect(switcher).toHaveBeenCalledTimes(1);
    expect(result.current.results.codex?.state).toBe("unverified");
  });

  it("allows the current owner to proceed after StrictMode effect replay", async () => {
    const switcher = vi
      .spyOn(vendorApi, "switchPreparedConnection")
      .mockRejectedValue(new Error("unknown"));
    const client = new QueryClient();
    const wrapper = ({ children }: { children: React.ReactNode }) => (
      <React.StrictMode>
        <QueryClientProvider client={client}>{children}</QueryClientProvider>
      </React.StrictMode>
    );
    const { result } = renderHook(
      () => useVendorServiceConfiguration(account, "codex", vi.fn()),
      { wrapper },
    );
    await act(async () => {
      await result.current.finish();
    });
    expect(switcher).toHaveBeenCalledTimes(1);
    expect(result.current.results.codex?.state).toBe("unverified");
  });

  it("does not revive a callback after the source app changes away and back", async () => {
    const switcher = vi
      .spyOn(vendorApi, "switchPreparedConnection")
      .mockRejectedValue(new Error("unknown"));
    const client = new QueryClient();
    const wrapper = ({ children }: { children: React.ReactNode }) => (
      <QueryClientProvider client={client}>{children}</QueryClientProvider>
    );
    const { result, rerender } = renderHook(
      ({ app }: { app: "codex" | "claude" }) =>
        useVendorServiceConfiguration(account, app, vi.fn()),
      { initialProps: { app: "codex" as "codex" | "claude" }, wrapper },
    );
    const oldFinish = result.current.finish;
    rerender({ app: "claude" });
    rerender({ app: "codex" });
    await act(async () => {
      await oldFinish();
    });
    expect(switcher).not.toHaveBeenCalled();
  });

  it("rejects a finish callback predating the current selection", async () => {
    const switcher = vi
      .spyOn(vendorApi, "switchPreparedConnection")
      .mockRejectedValue(new Error("unknown"));
    const { result } = setup();
    const oldFinish = result.current.finish;
    act(() => result.current.setSelection({ codex: "" }));
    await act(async () => {
      await oldFinish();
    });
    expect(switcher).not.toHaveBeenCalled();
    await act(async () => {
      await result.current.finish();
    });
    expect(switcher).not.toHaveBeenCalled();
  });

  it("never applies an old share-data consent after opt-out", async () => {
    runtime.completed = false;
    const complete = vi
      .spyOn(serviceOnboardingApi, "complete")
      .mockResolvedValue({
        shouldPrompt: false,
        completed: true,
        plazaVisible: false,
      });
    const { result } = setup();
    act(() => result.current.setSelection({ codex: "" }));
    const oldFinish = result.current.finish;
    act(() => result.current.setShareData(false));
    await act(async () => {
      await oldFinish();
    });
    expect(complete).not.toHaveBeenCalled();
    await act(async () => {
      await result.current.finish();
    });
    expect(complete).toHaveBeenCalledExactlyOnceWith(false);
  });

  it("retires a finish callback when choices change but not when identical data refetches", async () => {
    const switcher = vi
      .spyOn(vendorApi, "switchPreparedConnection")
      .mockRejectedValue(new Error("unknown"));
    const { result, rerender } = setup();
    const oldFinish = result.current.finish;
    runtime.choices = runtime.choices.map((choice) => ({ ...choice }));
    rerender();
    // Semantically unchanged query data must not invalidate the callback.
    await act(async () => {
      await oldFinish();
    });
    expect(switcher).toHaveBeenCalledTimes(1);
  });

  it("rejects a finish callback from an older choice set", async () => {
    const switcher = vi
      .spyOn(vendorApi, "switchPreparedConnection")
      .mockRejectedValue(new Error("unknown"));
    const { result, rerender } = setup();
    const oldFinish = result.current.finish;
    runtime.choices = [
      {
        app: "codex",
        id: "new-plan",
        providerId: "new-provider",
        name: "New Plan",
      },
    ];
    rerender();
    await act(async () => {
      await oldFinish();
    });
    expect(switcher).not.toHaveBeenCalled();
    await act(async () => {
      await result.current.finish();
    });
    expect(switcher.mock.calls[0][0].plan).toBe("new-plan");
  });

  it("keeps a failed read-only catalog preview retryable", async () => {
    vi.spyOn(vendorApi, "switchPreparedConnection").mockResolvedValue(
      confirmation as any,
    );
    vi.spyOn(vendorApi, "previewConnectionCatalog").mockRejectedValue(
      new Error("preview unavailable"),
    );
    const { result } = setup();
    let run: Promise<void>;
    act(() => {
      run = result.current.finish();
    });
    await waitFor(() => expect(result.current.confirmation).toBe("Plan"));
    await act(async () => {
      result.current.resolveConfirmation(
        undefined,
        { choice: "keep", revision: "r" },
        true,
      );
    });
    expect(result.current.confirmation).toBe("Plan");
    expect(result.current.completionError).toBe("preview unavailable");
    await act(async () => {
      result.current.resolveConfirmation(null);
      await run!;
    });
    expect(result.current.results.codex?.state).toBe("cancelled");
  });

  it("verifies a successful switch and completes normally", async () => {
    vi.spyOn(vendorApi, "switchPreparedConnection").mockResolvedValue({
      status: "switched",
      warnings: [],
    } as any);
    vi.spyOn(applicationOverviewApi, "state").mockResolvedValue(
      snapshot as any,
    );
    const { result, onDone } = setup();
    await act(async () => {
      await result.current.finish();
    });
    expect(result.current.results.codex).toEqual({
      id: "plan",
      state: "success",
      revision: "rev1",
    });
    expect(onDone).toHaveBeenCalledOnce();
  });

  it("preserves the existing explicit reapply action after an uncertain result", async () => {
    const switcher = vi
      .spyOn(vendorApi, "switchPreparedConnection")
      .mockRejectedValue(new Error("unknown"));
    const { result } = setup();
    await act(async () => {
      await result.current.finish();
    });
    await act(async () => {
      await result.current.reapply("codex");
    });
    await act(async () => {
      await result.current.confirmReview();
    });
    expect(switcher).toHaveBeenCalledTimes(2);
  });
});
