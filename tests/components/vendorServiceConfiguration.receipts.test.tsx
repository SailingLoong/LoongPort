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

const mock = vi.hoisted(() => ({
  prepare: vi.fn(),
  query: vi.fn(),
  switchPrepared: vi.fn(),
  legacy: vi.fn(),
  previewCatalog: vi.fn(),
  previewReview: vi.fn(),
  review: vi.fn(),
  previewNew: vi.fn(),
  confirmNew: vi.fn(),
  state: vi.fn(),
  complete: vi.fn(),
  completed: false,
}));
vi.mock("react-i18next", () => ({
  useTranslation: () => ({ t: (s: string) => s }),
}));
vi.mock("sonner", () => ({ toast: { warning: vi.fn() } }));
vi.mock("@/lib/api/vendor", () => ({
  vendorApi: {
    prepareConnection: mock.prepare,
    queryConnection: mock.query,
    switchPreparedConnection: mock.switchPrepared,
    switch: mock.legacy,
    previewConnectionCatalog: mock.previewCatalog,
    previewConnectionReview: mock.previewReview,
    reviewConnection: mock.review,
    previewNewConnectionIntent: mock.previewNew,
    confirmNewConnectionIntent: mock.confirmNew,
  },
}));
vi.mock("@/lib/api/applicationOverview", () => ({
  applicationOverviewApi: { state: mock.state },
}));
vi.mock("@/lib/api/serviceOnboarding", () => ({
  serviceOnboardingApi: { complete: mock.complete },
}));
vi.mock("@/components/relay/onboarding/useServiceOnboarding", () => ({
  useVendorConfigurationChoices: () => ({
    data: [
      { app: "claude", id: "a", providerId: "pa", name: "A" },
      { app: "codex", id: "b", providerId: "pb", name: "B" },
    ],
  }),
  useServiceOnboardingStatus: () => ({ data: { completed: mock.completed } }),
  serviceOnboardingKey: ["onboarding"],
}));
const account = { rowId: 7, name: "Vendor" };
const OLD_OPERATION_ID = "f8d02319-67c1-426b-a931-c836de11320e";
const OTHER_OPERATION_ID = "95fc0852-e7f3-4d3d-bd83-5e333fa49ec4";
function receipt(input: any, status = "prepared", revision = "rev1") {
  return {
    operationId: input.operationId,
    rowId: input.rowId,
    plan: input.plan,
    app: input.app,
    providerId: input.app === "claude" ? "pa" : "pb",
    receiptRevision: 1,
    status,
    operationState: status,
    completedConfigurationRevision: status === "completed" ? revision : null,
  };
}
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
      () => useVendorServiceConfiguration(account, "claude", onDone),
      { wrapper },
    ),
    onDone,
  };
}
beforeEach(() => {
  vi.clearAllMocks();
  mock.completed = false;
  mock.query.mockImplementation(async (_app: string, id: string | null) =>
    id === null ? { status: "none" } : { status: "unknown" },
  );
  mock.prepare.mockImplementation(async (input: any) => receipt(input));
  mock.switchPrepared.mockResolvedValue({ status: "unknown" });
  mock.legacy.mockRejectedValue(new Error("old path must not run"));
  mock.state.mockImplementation(async (app: string) => ({
    configurationRevision: "rev1",
    isAdditive: false,
    configurations: [
      {
        selection: {
          kind: "vendor",
          rowId: 7,
          planId: app === "claude" ? "a" : "b",
        },
        presentation: { isCurrent: true },
      },
    ],
  }));
  mock.complete.mockResolvedValue(undefined);
});
afterEach(() => cleanup());

describe("vendor durable operation consumer", () => {
  it("prepares, switches and queries one operation; consent remains independent", async () => {
    let saved: any;
    mock.prepare.mockImplementation(async (i: any) => {
      saved = i;
      return receipt(i);
    });
    mock.query.mockImplementation(async (_app: string, id: string | null) =>
      id === null ? { status: "none" } : receipt(saved, "completed"),
    );
    const { result, onDone } = setup();
    act(() => result.current.setShareData(false));
    await act(async () => {
      await result.current.finish();
    });
    expect(mock.prepare).toHaveBeenCalledOnce();
    expect(mock.switchPrepared).toHaveBeenCalledOnce();
    expect(mock.switchPrepared).toHaveBeenCalledWith({
      rowId: 7,
      plan: "a",
      app: "claude",
      quitChatgpt: null,
      operationId: saved.operationId,
      expectedRevision: 1,
    });
    expect(mock.prepare).toHaveBeenCalledOnce();
    expect(mock.query).toHaveBeenCalledWith("claude", saved.operationId);
    expect(result.current.results.claude).toMatchObject({
      id: "a",
      state: "success",
      revision: "rev1",
    });
    expect(mock.complete).toHaveBeenCalledExactlyOnceWith(false);
    expect(onDone).toHaveBeenCalledOnce();
    expect(mock.legacy).not.toHaveBeenCalled();
  });
  it("presents the native quit confirmation and cancellation does not reapply", async () => {
    mock.switchPrepared.mockResolvedValue({
      status: "confirmationRequired",
      targetName: "Plan",
      quitRequired: true,
    });
    const { result, onDone } = setup();
    let pending!: Promise<void>;
    act(() => {
      pending = result.current.finish();
    });
    await waitFor(() => expect(result.current.confirmation).toBe("Plan"));
    expect(result.current.quitRequired).toBe(true);
    expect(mock.switchPrepared).toHaveBeenCalledOnce();
    await act(async () => {
      result.current.resolveConfirmation(null);
      await pending;
    });
    expect(result.current.confirmation).toBeNull();
    expect(mock.switchPrepared).toHaveBeenCalledOnce();
    expect(mock.complete).not.toHaveBeenCalled();
    expect(onDone).not.toHaveBeenCalled();
  });

  it("stops later selected app dispatch when the user cancels a confirmation", async () => {
    mock.switchPrepared
      .mockResolvedValueOnce({
        status: "confirmationRequired",
        targetName: "Plan",
        quitRequired: true,
      })
      .mockResolvedValue({ status: "unknown" });
    const { result, onDone } = setup();
    act(() => result.current.setSelection({ codex: "b" }));
    let pending!: Promise<void>;
    act(() => {
      pending = result.current.finish();
    });
    await waitFor(() => expect(result.current.confirmation).toBe("Plan"));
    await act(async () => {
      result.current.resolveConfirmation(null);
      await pending;
    });
    expect(mock.prepare).toHaveBeenCalledOnce();
    expect(mock.switchPrepared).toHaveBeenCalledOnce();
    expect(result.current.results.claude?.state).toBe("cancelled");
    expect(mock.complete).not.toHaveBeenCalled();
    expect(onDone).not.toHaveBeenCalled();
  });

  it("continues an explicitly confirmed quit with the original prepared operation", async () => {
    let saved: any;
    mock.prepare.mockImplementation(async (input: any) => {
      saved = input;
      return receipt(input);
    });
    mock.switchPrepared
      .mockResolvedValueOnce({
        status: "confirmationRequired",
        targetName: "Plan",
        quitRequired: true,
      })
      .mockResolvedValue({ status: "switched", warnings: [] });
    mock.query.mockImplementation(async (_app: string, id: string | null) =>
      id === null ? { status: "none" } : receipt(saved, "completed"),
    );
    const { result, onDone } = setup();
    let pending!: Promise<void>;
    act(() => {
      pending = result.current.finish();
    });
    await waitFor(() => expect(result.current.confirmation).toBe("Plan"));
    await act(async () => {
      result.current.resolveConfirmation(true);
      await pending;
    });
    expect(mock.prepare).toHaveBeenCalledOnce();
    expect(mock.switchPrepared).toHaveBeenCalledTimes(2);
    expect(mock.switchPrepared.mock.calls[1][0]).toEqual({
      ...mock.switchPrepared.mock.calls[0][0],
      quitChatgpt: true,
    });
    expect(result.current.results.claude?.state).toBe("success");
    expect(onDone).toHaveBeenCalledOnce();
  });

  it("binds a confirmed catalog decision to a new operation after reviewing the original prepare", async () => {
    const decision = { choice: "keep", revision: "a".repeat(64) } as const;
    let saved: any;
    mock.prepare.mockImplementation(async (input: any) => {
      saved = input;
      return receipt(input);
    });
    mock.previewReview.mockImplementation(async (input: any) => ({
      ...input,
      status: "reviewRequired",
      receiptRevision: 1,
      observedRevision: "observed-before-confirm",
    }));
    mock.review.mockImplementation(async () => ({
      ...receipt(saved),
      status: "failedBeforeWrite",
      operationState: "failedBeforeWrite",
      receiptRevision: 2,
    }));
    mock.switchPrepared
      .mockResolvedValueOnce({
        status: "confirmationRequired",
        targetName: "Plan",
        quitRequired: false,
        catalog: { ...decision },
      })
      .mockResolvedValue({ status: "switched", warnings: [] });
    mock.query.mockImplementation(async (_app: string, id: string | null) =>
      id === null ? { status: "none" } : receipt(saved, "completed"),
    );
    const { result, onDone } = setup();
    act(() => result.current.setSelection({ claude: "", codex: "b" }));
    let run!: Promise<void>;
    act(() => {
      run = result.current.finish();
    });
    await waitFor(() => expect(result.current.confirmation).toBe("Plan"));
    expect(mock.previewReview).toHaveBeenCalledOnce();
    expect(mock.review).not.toHaveBeenCalled();
    await act(async () => {
      result.current.resolveConfirmation(false, decision);
      await run;
    });
    const first = mock.prepare.mock.calls[0][0];
    const second = mock.prepare.mock.calls[1][0];
    expect(second.operationId).not.toBe(first.operationId);
    expect(second.catalogDecision).toEqual(decision);
    expect(mock.review).toHaveBeenCalledWith({
      app: "codex",
      operationId: first.operationId,
      expectedRevision: 1,
      observedRevision: "observed-before-confirm",
    });
    expect(mock.switchPrepared.mock.calls[1][0]).toMatchObject({
      operationId: second.operationId,
      quitChatgpt: false,
    });
    expect(onDone).toHaveBeenCalledOnce();
  });

  it("never prepares a replacement when catalog review loses its response", async () => {
    const decision = { choice: "keep", revision: "a".repeat(64) } as const;
    mock.previewReview.mockImplementation(async (input: any) => ({
      ...input,
      status: "reviewRequired",
      receiptRevision: 1,
      observedRevision: "before",
    }));
    mock.review.mockRejectedValue(new Error("review reply lost"));
    mock.switchPrepared.mockResolvedValue({
      status: "confirmationRequired",
      targetName: "Plan",
      quitRequired: false,
      catalog: decision,
    });
    const { result, onDone } = setup();
    act(() => result.current.setSelection({ claude: "", codex: "b" }));
    let run!: Promise<void>;
    act(() => {
      run = result.current.finish();
    });
    await waitFor(() => expect(result.current.confirmation).toBe("Plan"));
    await act(async () => {
      result.current.resolveConfirmation(false, decision);
      await run;
    });
    expect(mock.prepare).toHaveBeenCalledOnce();
    expect(mock.switchPrepared).toHaveBeenCalledOnce();
    expect(result.current.results.codex?.state).toBe("unverified");
    expect(onDone).not.toHaveBeenCalled();
    expect(mock.query).toHaveBeenCalledWith(
      "codex",
      mock.prepare.mock.calls[0][0].operationId,
    );
  });

  it("keeps a failed catalog preview retryable without retiring or applying", async () => {
    const decision = { choice: "keep", revision: "a".repeat(64) } as const;
    mock.previewReview.mockImplementation(async (input: any) => ({
      ...input,
      status: "reviewRequired",
      receiptRevision: 1,
      observedRevision: "before",
    }));
    mock.previewCatalog.mockRejectedValueOnce(new Error("preview unavailable"));
    mock.switchPrepared.mockResolvedValue({
      status: "confirmationRequired",
      targetName: "Plan",
      quitRequired: false,
      catalog: decision,
    });
    const { result } = setup();
    act(() => result.current.setSelection({ claude: "", codex: "b" }));
    let run!: Promise<void>;
    act(() => {
      run = result.current.finish();
    });
    await waitFor(() => expect(result.current.confirmation).toBe("Plan"));
    await act(async () => {
      result.current.resolveConfirmation(
        undefined,
        { ...decision, choice: "manage" },
        true,
      );
    });
    await waitFor(() => expect(result.current.confirmation).toBe("Plan"));
    expect(result.current.completionError).toBe("preview unavailable");
    expect(mock.review).not.toHaveBeenCalled();
    expect(mock.prepare).toHaveBeenCalledOnce();
    expect(mock.switchPrepared).toHaveBeenCalledOnce();
    await act(async () => {
      result.current.resolveConfirmation(null);
      await run;
    });
    expect(result.current.results.codex?.state).toBe("cancelled");
  });

  it.each([
    { choice: "manage", revision: "a".repeat(64) },
    { choice: "keep", revision: "b".repeat(64) },
  ])(
    "rejects a catalog confirmation different from the displayed preview %#",
    async (unseen) => {
      const displayed = { choice: "keep", revision: "a".repeat(64) } as const;
      mock.previewReview.mockImplementation(async (input: any) => ({
        ...input,
        status: "reviewRequired",
        receiptRevision: 1,
        observedRevision: "before",
      }));
      mock.review.mockResolvedValue({ status: "unknown" });
      mock.switchPrepared.mockResolvedValue({
        status: "confirmationRequired",
        targetName: "Plan",
        quitRequired: false,
        catalog: displayed,
      });
      const { result } = setup();
      act(() => result.current.setSelection({ claude: "", codex: "b" }));
      let run!: Promise<void>;
      act(() => {
        run = result.current.finish();
      });
      await waitFor(() => expect(result.current.confirmation).toBe("Plan"));
      act(() => result.current.resolveConfirmation(false, unseen as any));
      await act(async () => {
        await Promise.resolve();
      });
      expect(mock.review).not.toHaveBeenCalled();
      expect(mock.prepare).toHaveBeenCalledOnce();
      expect(result.current.confirmation).toBe("Plan");
      await act(async () => {
        result.current.resolveConfirmation(null);
        await run;
      });
    },
  );

  it("requires a displayed new-intent review before preparing another uncertain operation", async () => {
    let saved: any;
    let retired = false;
    mock.prepare.mockImplementation(async (input: any) => {
      saved = input;
      return receipt(input);
    });
    mock.query.mockImplementation(async (_app: string, id: string | null) =>
      id === null
        ? { status: "none" }
        : retired
          ? receipt(saved, "completed")
          : { status: "unknown", operationId: id },
    );
    mock.previewNew.mockImplementation(async (input: any) => ({
      ...input,
      status: "newIntentReviewRequired",
      receiptRevision: 1,
      originalRowId: 7,
      originalPlan: "a",
      targetProviderId: "pa",
      reviewToken: "a".repeat(64),
    }));
    mock.confirmNew.mockImplementation(async () => {
      retired = true;
      return { status: "newIntentPermitted", app: "claude" };
    });
    const { result, onDone } = setup();
    await act(async () => {
      await result.current.finish();
    });
    const oldId = saved.operationId;
    await act(async () => {
      await result.current.reapply("claude");
    });
    expect(result.current.pendingReview).toMatchObject({
      app: "claude",
      id: "a",
    });
    expect(mock.prepare).toHaveBeenCalledOnce();
    expect(mock.confirmNew).not.toHaveBeenCalled();
    await act(async () => {
      await result.current.confirmReview();
    });
    expect(mock.confirmNew).toHaveBeenCalledWith({
      app: "claude",
      operationId: oldId,
      targetRowId: 7,
      targetPlan: "a",
      reviewToken: "a".repeat(64),
    });
    expect(mock.prepare).toHaveBeenCalledTimes(2);
    expect(saved.operationId).not.toBe(oldId);
    expect(onDone).toHaveBeenCalledOnce();
  });

  it("reviews a changed completed result without retiring an already terminal operation", async () => {
    let saved: any;
    let revision = "rev1";
    mock.prepare.mockImplementation(async (input: any) => {
      saved = input;
      return receipt(input);
    });
    mock.query.mockImplementation(async (_app: string, id: string | null) =>
      id === null ? { status: "none" } : receipt(saved, "completed", revision),
    );
    mock.state.mockImplementation(async () => ({
      configurationRevision: revision,
      isAdditive: false,
      configurations: [
        {
          selection: { kind: "vendor", rowId: 7, planId: "a" },
          presentation: { isCurrent: true },
        },
      ],
    }));
    mock.complete.mockRejectedValueOnce(new Error("consent retry"));
    mock.previewNew.mockRejectedValue(new Error("terminal cannot retire"));
    const { result, onDone } = setup();
    await act(async () => {
      await result.current.finish();
    });
    revision = "rev2";
    await act(async () => {
      await result.current.finish();
    });
    expect(result.current.results.claude?.state).toBe("changed");
    await act(async () => {
      await result.current.reapply("claude");
    });
    expect(result.current.pendingReview).toMatchObject({
      app: "claude",
      proof: null,
      observedRevision: "rev2",
    });
    expect(mock.previewNew).not.toHaveBeenCalled();
    await act(async () => {
      await result.current.confirmReview();
    });
    expect(mock.prepare).toHaveBeenCalledTimes(2);
    expect(mock.confirmNew).not.toHaveBeenCalled();
    expect(onDone).toHaveBeenCalledOnce();
  });

  it("recovers a trusted retired operation only through a fresh displayed review", async () => {
    let saved: any;
    mock.prepare.mockImplementation(async (input: any) => {
      saved = input;
      return receipt(input);
    });
    const { result } = setup();
    await act(async () => {
      await result.current.finish();
    });
    mock.query.mockImplementation(async () => ({
      ...receipt(saved),
      status: "unknown",
      operationState: "reviewed",
    }));
    await act(async () => {
      await result.current.reapply("claude");
    });
    expect(result.current.pendingReview).toMatchObject({
      app: "claude",
      proof: null,
      observedRevision: "rev1",
    });
    expect(mock.previewNew).not.toHaveBeenCalled();
    expect(mock.prepare).toHaveBeenCalledOnce();
    act(() => result.current.cancelReview());
    await act(async () => {
      await result.current.confirmReview();
    });
    expect(mock.prepare).toHaveBeenCalledOnce();
  });

  it("cancel during a new-intent confirmation prevents the subsequent prepare", async () => {
    mock.previewNew.mockImplementation(async (input: any) => ({
      ...input,
      status: "newIntentReviewRequired",
      receiptRevision: 1,
      originalRowId: 7,
      originalPlan: "a",
      targetProviderId: "pa",
      reviewToken: "a".repeat(64),
    }));
    let resolve!: (value: any) => void;
    mock.confirmNew.mockImplementation(
      () =>
        new Promise((done) => {
          resolve = done;
        }),
    );
    const { result } = setup();
    await act(async () => {
      await result.current.finish();
    });
    await act(async () => {
      await result.current.reapply("claude");
    });
    let confirmation!: Promise<void>;
    act(() => {
      confirmation = result.current.confirmReview();
    });
    act(() => result.current.cancelReview());
    await act(async () => {
      resolve({ status: "newIntentPermitted", app: "claude" });
      await confirmation;
    });
    expect(result.current.pendingReview).toBeNull();
    expect(mock.prepare).toHaveBeenCalledOnce();
  });

  it("after a lost switch response queries the original id and never resends while unknown", async () => {
    let saved: any;
    mock.prepare.mockImplementation(async (i: any) => {
      saved = i;
      return receipt(i);
    });
    mock.switchPrepared.mockRejectedValue(new Error("lost response"));
    const { result } = setup();
    await act(async () => {
      await result.current.finish();
    });
    expect(result.current.results.claude?.state).toBe("unverified");
    expect(result.current.results.claude?.error).toBe("lost response");
    expect(mock.prepare).toHaveBeenCalledOnce();
    expect(mock.query).toHaveBeenCalledWith("claude", saved.operationId);
    await act(async () => {
      await result.current.finish();
    });
    expect(
      mock.query.mock.calls.filter(([, id]) => id === saved.operationId).length,
    ).toBeGreaterThanOrEqual(2);
    expect(mock.prepare).toHaveBeenCalledOnce();
    expect(mock.switchPrepared).toHaveBeenCalledOnce();
    expect(mock.complete).not.toHaveBeenCalled();
  });
  it("accepts a query-confirmed existing completed operation without a second write", async () => {
    let saved: any;
    mock.prepare.mockImplementation(async (input: any) => {
      saved = input;
      return receipt(input, "completed");
    });
    mock.query.mockImplementation(async (_app: string, id: string | null) =>
      id === null ? { status: "none" } : receipt(saved, "completed"),
    );
    const { result, onDone } = setup();
    await act(async () => {
      await result.current.finish();
    });
    expect(mock.query).toHaveBeenCalledWith("claude", saved.operationId);
    expect(mock.switchPrepared).not.toHaveBeenCalled();
    expect(result.current.results.claude?.state).toBe("success");
    expect(onDone).toHaveBeenCalledOnce();
  });

  it("lost prepare response queries that same id but does not dispatch a switch", async () => {
    mock.prepare.mockRejectedValue(new Error("lost prepare"));
    const { result } = setup();
    await act(async () => {
      await result.current.finish();
    });
    expect(mock.prepare).toHaveBeenCalledOnce();
    const id = mock.prepare.mock.calls[0][0].operationId;
    expect(mock.query).toHaveBeenCalledWith("claude", id);
    expect(mock.switchPrepared).not.toHaveBeenCalled();
    expect(result.current.results.claude?.state).toBe("unverified");
    expect(mock.complete).not.toHaveBeenCalled();
  });
  it.each([{ app: "codex" }])(
    "never accepts an unrelated restored receipt %#",
    async (change) => {
      mock.query.mockResolvedValue({
        ...receipt(
          { operationId: OLD_OPERATION_ID, rowId: 7, plan: "a", app: "claude" },
          "completed",
        ),
        ...change,
      });
      const { result } = setup();
      await act(async () => {
        await result.current.finish();
      });
      expect(result.current.results.claude?.state).toBe("unverified");
      expect(mock.prepare).not.toHaveBeenCalled();
      expect(mock.switchPrepared).not.toHaveBeenCalled();
      expect(mock.legacy).not.toHaveBeenCalled();
      expect(mock.complete).not.toHaveBeenCalled();
    },
  );
  it("does not block a selected app because another selected app is unresolved", async () => {
    let saved: any;
    mock.prepare.mockImplementation(async (input: any) => {
      saved = input;
      return receipt(input);
    });
    mock.query.mockImplementation(async (app: string, id: string | null) => {
      if (app === "codex") return { status: "unknown" };
      return id === null ? { status: "none" } : receipt(saved, "completed");
    });
    const { result, onDone } = setup();
    act(() => result.current.setSelection({ codex: "b" }));
    await act(async () => {
      await result.current.finish();
    });
    expect(mock.switchPrepared.mock.calls.map(([input]) => input.app)).toEqual([
      "claude",
    ]);
    expect(result.current.results.claude?.state).toBe("success");
    expect(result.current.results.codex?.state).toBe("unverified");
    expect(mock.complete).not.toHaveBeenCalled();
    expect(onDone).not.toHaveBeenCalled();
  });

  it.each([
    { rowId: 8 },
    { plan: "other" },
    { providerId: "other" },
    { operationId: OTHER_OPERATION_ID },
  ])(
    "does not accept a wrong target for a known operation %#",
    async (change) => {
      let saved: any;
      mock.prepare.mockImplementation(async (input: any) => {
        saved = input;
        return receipt(input);
      });
      mock.query.mockImplementation(async (_app: string, id: string | null) =>
        id === null
          ? { status: "none" }
          : { ...receipt(saved, "completed"), ...change },
      );
      const { result, onDone } = setup();
      await act(async () => {
        await result.current.finish();
      });
      expect(result.current.results.claude?.state).toBe("unverified");
      expect(mock.prepare).toHaveBeenCalledOnce();
      expect(mock.switchPrepared).toHaveBeenCalledOnce();
      expect(mock.complete).not.toHaveBeenCalled();
      expect(onDone).not.toHaveBeenCalled();
    },
  );

  it.each([{ rowId: 8 }, { plan: "other" }, { providerId: "old-provider" }])(
    "permits an explicit new target after a different terminal operation %#",
    async (change) => {
      let saved: any;
      mock.prepare.mockImplementation(async (input: any) => {
        saved = input;
        return receipt(input);
      });
      mock.query.mockImplementation(async (app: string, id: string | null) => {
        if (app !== "claude") return { status: "none" };
        if (id === null)
          return {
            ...receipt(
              { operationId: OLD_OPERATION_ID, rowId: 7, plan: "a", app },
              "completed",
            ),
            ...change,
          };
        return receipt(saved, "completed");
      });
      const { result, onDone } = setup();
      await act(async () => {
        await result.current.finish();
      });
      expect(mock.prepare).toHaveBeenCalledOnce();
      expect(mock.switchPrepared).toHaveBeenCalledOnce();
      expect(result.current.results.claude?.state).toBe("success");
      expect(onDone).toHaveBeenCalledOnce();
    },
  );

  it("recovers an unresolved unselected app without blocking an independent app write", async () => {
    let saved: any;
    mock.prepare.mockImplementation(async (input: any) => {
      saved = input;
      return receipt(input);
    });
    mock.query.mockImplementation(async (app: string, id: string | null) => {
      if (app === "codex" && id === null)
        return {
          ...receipt({
            operationId: OLD_OPERATION_ID,
            rowId: 7,
            plan: "b",
            app: "codex",
          }),
          status: "unknown",
          operationState: "applying",
        };
      return id === null ? { status: "none" } : receipt(saved, "completed");
    });
    const { result, onDone } = setup();
    await act(async () => {
      await result.current.finish();
    });
    expect(mock.query).toHaveBeenCalledWith("codex", null);
    expect(mock.switchPrepared.mock.calls.map(([input]) => input.app)).toEqual([
      "claude",
    ]);
    expect(result.current.results.claude?.state).toBe("success");
    expect(result.current.results.codex?.state).toBe("unverified");
    expect(mock.complete).not.toHaveBeenCalled();
    expect(onDone).not.toHaveBeenCalled();
  });

  it("treats a lost reply as completed only after querying and verifying the original receipt", async () => {
    let saved: any;
    mock.prepare.mockImplementation(async (input: any) => {
      saved = input;
      return receipt(input);
    });
    mock.switchPrepared.mockRejectedValue(new Error("lost"));
    mock.query.mockImplementation(async (_app: string, id: string | null) =>
      id === null ? { status: "none" } : receipt(saved, "completed"),
    );
    const { result, onDone } = setup();
    await act(async () => {
      await result.current.finish();
    });
    expect(mock.query).toHaveBeenCalledWith("claude", saved.operationId);
    expect(mock.switchPrepared).toHaveBeenCalledOnce();
    expect(result.current.results.claude?.state).toBe("success");
    expect(onDone).toHaveBeenCalledOnce();
  });

  it("rechecks the same uncertain ID until a matching completion is available", async () => {
    let saved: any;
    mock.prepare.mockImplementation(async (input: any) => {
      saved = input;
      return receipt(input);
    });
    mock.switchPrepared.mockRejectedValue(new Error("lost"));
    mock.query.mockImplementation(async (_app: string, id: string | null) =>
      id === null ? { status: "none" } : { status: "unknown" },
    );
    const { result, onDone } = setup();
    await act(async () => {
      await result.current.finish();
    });
    expect(result.current.results.claude?.state).toBe("unverified");
    mock.query.mockImplementation(async (_app: string, id: string | null) =>
      id === null ? { status: "none" } : receipt(saved, "completed"),
    );
    await act(async () => {
      await result.current.finish();
    });
    expect(mock.prepare).toHaveBeenCalledOnce();
    expect(mock.switchPrepared).toHaveBeenCalledOnce();
    expect(mock.query).toHaveBeenCalledWith("claude", saved.operationId);
    expect(result.current.results.claude?.state).toBe("success");
    expect(onDone).toHaveBeenCalledOnce();
  });

  it("does not confuse an unrelated account's operation with this setup", async () => {
    let saved: any;
    mock.prepare.mockImplementation(async (input: any) => {
      saved = input;
      return receipt(input);
    });
    mock.query.mockImplementation(async (app: string, id: string | null) => {
      if (app === "codex" && id === null)
        return {
          ...receipt({
            operationId: OTHER_OPERATION_ID,
            rowId: 8,
            plan: "b",
            app: "codex",
          }),
          status: "unknown",
          operationState: "applying",
        };
      return id === null ? { status: "none" } : receipt(saved, "completed");
    });
    const { result, onDone } = setup();
    await act(async () => {
      await result.current.finish();
    });
    expect(result.current.results.claude?.state).toBe("success");
    expect(result.current.results.codex).toBeUndefined();
    expect(onDone).toHaveBeenCalledOnce();
  });

  it.each(["read-error", "redacted"])(
    "does not attribute an unselected app %s to this account or block an independent completion",
    async (kind) => {
      let saved: any;
      mock.prepare.mockImplementation(async (input: any) => {
        saved = input;
        return receipt(input);
      });
      mock.query.mockImplementation(async (app: string, id: string | null) => {
        if (app === "codex" && id === null) {
          if (kind === "read-error") throw new Error("read failed");
          return { status: "unknown" };
        }
        return id === null ? { status: "none" } : receipt(saved, "completed");
      });
      const { result, onDone } = setup();
      await act(async () => {
        await result.current.finish();
      });
      expect(result.current.results.claude?.state).toBe("success");
      expect(result.current.results.codex).toBeUndefined();
      expect(onDone).toHaveBeenCalledOnce();
      expect(mock.complete).toHaveBeenCalledOnce();
    },
  );

  it("does not dispatch a reapply for an unresolved native operation without review", async () => {
    mock.query.mockImplementation(async (app: string) =>
      app === "claude" ? { status: "unknown" } : { status: "none" },
    );
    const { result } = setup();
    await act(async () => {
      await result.current.finish();
    });
    expect(result.current.completionError).toBeNull();
    await act(async () => {
      await result.current.reapply("claude");
    });
    expect(result.current.completionError).toBe(
      "loongport.onboarding.cannotVerify",
    );
    await act(async () => {
      await result.current.finish();
    });
    expect(mock.prepare).not.toHaveBeenCalled();
    expect(mock.switchPrepared).not.toHaveBeenCalled();
    expect(mock.legacy).not.toHaveBeenCalled();
  });

  it("does not create two native operations on same-tick finish", async () => {
    let release!: (value: any) => void;
    mock.query
      .mockImplementationOnce(
        () =>
          new Promise((resolve) => {
            release = resolve;
          }),
      )
      .mockResolvedValue({ status: "unknown" });
    const { result } = setup();
    let first!: Promise<void>;
    act(() => {
      first = result.current.finish();
      void result.current.finish();
    });
    expect(mock.query).toHaveBeenCalledOnce();
    await act(async () => {
      release({ status: "unknown" });
      await first;
    });
    expect(mock.prepare).not.toHaveBeenCalled();
    expect(mock.switchPrepared).not.toHaveBeenCalled();
  });

  it("retains a recovered unknown operation identity for the next explicit check", async () => {
    mock.query.mockImplementation(async (app: string, _id: string | null) => {
      if (app === "claude")
        return {
          ...receipt({
            operationId: OLD_OPERATION_ID,
            rowId: 7,
            plan: "a",
            app,
          }),
          status: "unknown",
          operationState: "applying",
        };
      return { status: "none" };
    });
    const { result } = setup();
    await act(async () => {
      await result.current.finish();
    });
    await act(async () => {
      await result.current.finish();
    });
    expect(mock.query).toHaveBeenCalledWith("claude", OLD_OPERATION_ID);
    expect(mock.prepare).not.toHaveBeenCalled();
    expect(mock.switchPrepared).not.toHaveBeenCalled();
  });

  it("recovers the original operation after unmount without a new dispatch", async () => {
    let saved: any;
    let release!: (value: any) => void;
    mock.prepare.mockImplementation(async (input: any) => {
      saved = input;
      return receipt(input);
    });
    mock.switchPrepared.mockImplementation(
      () =>
        new Promise((resolve) => {
          release = resolve;
        }),
    );
    mock.query.mockImplementation(async (app: string, _id: string | null) => {
      if (app === "claude" && saved)
        return {
          ...receipt(saved),
          status: "unknown",
          operationState: "applying",
        };
      return { status: "none" };
    });
    const first = setup();
    let pending!: Promise<void>;
    act(() => {
      pending = first.result.current.finish();
    });
    await waitFor(() => expect(mock.switchPrepared).toHaveBeenCalledOnce());
    first.unmount();
    await act(async () => {
      release({ status: "unknown" });
      await pending;
    });
    const next = setup();
    await act(async () => {
      await next.result.current.finish();
    });
    expect(next.result.current.results.claude?.state).toBe("unverified");
    await act(async () => {
      await next.result.current.finish();
    });
    expect(mock.query).toHaveBeenCalledWith("claude", saved.operationId);
    expect(mock.prepare).toHaveBeenCalledOnce();
    expect(mock.switchPrepared).toHaveBeenCalledOnce();
    expect(next.onDone).not.toHaveBeenCalled();
  });

  it("retries only consent after its failure without creating another operation", async () => {
    let saved: any;
    mock.prepare.mockImplementation(async (input: any) => {
      saved = input;
      return receipt(input);
    });
    mock.query.mockImplementation(async (_app: string, id: string | null) =>
      id === null ? { status: "none" } : receipt(saved, "completed"),
    );
    mock.complete
      .mockRejectedValueOnce(new Error("consent failed"))
      .mockResolvedValue(undefined);
    const { result, onDone } = setup();
    act(() => result.current.setShareData(false));
    await act(async () => {
      await result.current.finish();
    });
    expect(result.current.completionError).toBe("consent failed");
    expect(result.current.results.claude?.state).toBe("success");
    await act(async () => {
      await result.current.finish();
    });
    expect(mock.prepare).toHaveBeenCalledOnce();
    expect(mock.switchPrepared).toHaveBeenCalledOnce();
    expect(mock.complete.mock.calls).toEqual([[false], [false]]);
    expect(onDone).toHaveBeenCalledOnce();
  });

  it("does not carry a successful account result into a different account", async () => {
    let saved: any;
    mock.prepare.mockImplementation(async (input: any) => {
      saved = input;
      return receipt(input);
    });
    mock.query.mockImplementation(async (_app: string, id: string | null) =>
      id === null ? { status: "none" } : receipt(saved, "completed"),
    );
    mock.complete.mockRejectedValueOnce(new Error("consent unavailable"));
    const client = new QueryClient();
    const onDone = vi.fn();
    const wrapper = ({ children }: { children: React.ReactNode }) => (
      <QueryClientProvider client={client}>{children}</QueryClientProvider>
    );
    const { result, rerender } = renderHook(
      ({ rowId }) =>
        useVendorServiceConfiguration(
          { rowId, name: "Vendor" },
          "claude",
          onDone,
        ),
      { initialProps: { rowId: 7 }, wrapper },
    );
    await act(async () => {
      await result.current.finish();
    });
    expect(result.current.results.claude?.state).toBe("success");
    rerender({ rowId: 8 });
    expect(result.current.results.claude?.state).not.toBe("success");
    mock.state.mockResolvedValue({
      configurationRevision: "rev1",
      isAdditive: false,
      configurations: [
        {
          selection: { kind: "vendor", rowId: 8, planId: "a" },
          presentation: { isCurrent: true },
        },
      ],
    });
    await act(async () => {
      await result.current.finish();
    });
    expect(onDone).not.toHaveBeenCalled();
    expect(mock.complete).toHaveBeenCalledOnce();
    expect(mock.prepare).toHaveBeenCalledOnce();
  });

  it("retires an old finish callback after the user changes consent", async () => {
    const { result } = setup();
    const oldFinish = result.current.finish;
    act(() => result.current.setShareData(false));
    await act(async () => {
      await oldFinish();
    });
    expect(mock.query).not.toHaveBeenCalled();
    expect(mock.prepare).not.toHaveBeenCalled();
    expect(mock.complete).not.toHaveBeenCalled();
  });

  it("rejects a retained finish callback after a source app changes away and back", async () => {
    const client = new QueryClient();
    const wrapper = ({ children }: { children: React.ReactNode }) => (
      <QueryClientProvider client={client}>{children}</QueryClientProvider>
    );
    const { result, rerender } = renderHook(
      ({ app }: { app: "claude" | "codex" }) =>
        useVendorServiceConfiguration(account, app, vi.fn()),
      { initialProps: { app: "claude" as "claude" | "codex" }, wrapper },
    );
    const oldFinish = result.current.finish;
    rerender({ app: "codex" });
    rerender({ app: "claude" });
    await act(async () => {
      await oldFinish();
    });
    expect(mock.query).not.toHaveBeenCalled();
    expect(mock.prepare).not.toHaveBeenCalled();
  });

  it("retires a retained finish callback across Activity hide and reveal", async () => {
    const client = new QueryClient();
    let latest!: ReturnType<typeof useVendorServiceConfiguration>;
    function Owner() {
      latest = useVendorServiceConfiguration(account, "claude", vi.fn());
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
    const oldFinish = latest.finish;
    view.rerender(<Tree visible={false} />);
    view.rerender(<Tree visible />);
    await act(async () => {
      await oldFinish();
    });
    expect(mock.query).not.toHaveBeenCalled();
    mock.query.mockResolvedValue({ status: "unknown" });
    await act(async () => {
      await latest.finish();
    });
    expect(mock.query).toHaveBeenCalled();
    expect(mock.prepare).not.toHaveBeenCalled();
  });

  it("retains the original ID across Activity suspension until visible recheck", async () => {
    let saved: any;
    let release!: (value: any) => void;
    mock.prepare.mockImplementation(async (input: any) => {
      saved = input;
      return receipt(input);
    });
    mock.switchPrepared.mockImplementation(
      () =>
        new Promise((resolve) => {
          release = resolve;
        }),
    );
    mock.query.mockImplementation(async (app: string, _id: string | null) => {
      if (app === "claude" && saved)
        return {
          ...receipt(saved),
          status: "unknown",
          operationState: "applying",
        };
      return { status: "none" };
    });
    const client = new QueryClient();
    let latest!: ReturnType<typeof useVendorServiceConfiguration>;
    function Owner() {
      latest = useVendorServiceConfiguration(account, "claude", vi.fn());
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
    await waitFor(() => expect(mock.switchPrepared).toHaveBeenCalledOnce());
    view.rerender(<Tree visible={false} />);
    await act(async () => {
      release({ status: "unknown" });
      await first;
    });
    view.rerender(<Tree visible />);
    await act(async () => {
      await latest.finish();
    });
    expect(latest.results.claude?.state).toBe("unverified");
    expect(mock.query).toHaveBeenCalledWith("claude", saved.operationId);
    expect(mock.prepare).toHaveBeenCalledOnce();
    expect(mock.switchPrepared).toHaveBeenCalledOnce();
  });

  it("retains a recovered completed ID even when its configuration revision is absent", async () => {
    let nullReads = 0;
    mock.query.mockImplementation(async (app: string, id: string | null) => {
      if (app !== "claude") return { status: "none" };
      if (id === null) {
        nullReads++;
        return {
          ...receipt(
            {
              operationId:
                nullReads === 1 ? OLD_OPERATION_ID : OTHER_OPERATION_ID,
              rowId: 7,
              plan: "a",
              app,
            },
            "completed",
          ),
          completedConfigurationRevision: nullReads === 1 ? null : "rev1",
        };
      }
      return {
        ...receipt(
          { operationId: OLD_OPERATION_ID, rowId: 7, plan: "a", app },
          "completed",
        ),
        completedConfigurationRevision: null,
      };
    });
    const { result, onDone } = setup();
    await act(async () => {
      await result.current.finish();
    });
    await act(async () => {
      await result.current.finish();
    });
    expect(mock.query).toHaveBeenCalledWith("claude", OLD_OPERATION_ID);
    expect(result.current.results.claude?.state).toBe("unverified");
    expect(onDone).not.toHaveBeenCalled();
    expect(mock.switchPrepared).not.toHaveBeenCalled();
  });

  it("retains a redacted unknown ID only as a query hint", async () => {
    mock.query.mockImplementation(async (app: string, id: string | null) => {
      if (app !== "claude") return { status: "none" };
      return id === null
        ? { status: "unknown", app, operationId: OLD_OPERATION_ID }
        : { status: "unknown" };
    });
    const { result, onDone } = setup();
    await act(async () => {
      await result.current.finish();
    });
    await act(async () => {
      await result.current.finish();
    });
    expect(mock.query).toHaveBeenCalledWith("claude", OLD_OPERATION_ID);
    expect(mock.prepare).not.toHaveBeenCalled();
    expect(mock.switchPrepared).not.toHaveBeenCalled();
    expect(onDone).not.toHaveBeenCalled();
  });

  it("allows a new intent after a redacted query hint proves to be a different completed operation", async () => {
    let saved: any;
    mock.prepare.mockImplementation(async (input: any) => {
      saved = input;
      return receipt(input);
    });
    mock.query.mockImplementation(async (app: string, id: string | null) => {
      if (app !== "claude") return { status: "none" };
      if (id === null)
        return { status: "unknown", app, operationId: OLD_OPERATION_ID };
      if (id === OLD_OPERATION_ID)
        return receipt(
          { operationId: OLD_OPERATION_ID, rowId: 8, plan: "other", app },
          "completed",
        );
      return receipt(saved, "completed");
    });
    const { result, onDone } = setup();
    await act(async () => {
      await result.current.finish();
    });
    expect(result.current.results.claude?.state).toBe("unverified");
    expect(mock.prepare).not.toHaveBeenCalled();
    await act(async () => {
      await result.current.finish();
    });
    expect(mock.query).toHaveBeenCalledWith("claude", OLD_OPERATION_ID);
    expect(mock.prepare).toHaveBeenCalledOnce();
    expect(mock.switchPrepared).toHaveBeenCalledOnce();
    expect(result.current.results.claude?.state).toBe("success");
    expect(onDone).toHaveBeenCalledOnce();
  });

  it("retains a foreign full unknown as a lookup hint rather than adopting a later receipt", async () => {
    let saved: any;
    let initialReads = 0;
    mock.prepare.mockImplementation(async (input: any) => {
      saved = input;
      return receipt(input);
    });
    mock.query.mockImplementation(async (app: string, id: string | null) => {
      if (app !== "claude") return { status: "none" };
      if (id === null) {
        initialReads++;
        return initialReads === 1
          ? {
              ...receipt({
                operationId: OLD_OPERATION_ID,
                rowId: 8,
                plan: "other",
                app,
              }),
              status: "unknown",
              operationState: "applying",
            }
          : receipt(
              { operationId: OTHER_OPERATION_ID, rowId: 7, plan: "a", app },
              "completed",
            );
      }
      if (id === OLD_OPERATION_ID)
        return receipt(
          { operationId: OLD_OPERATION_ID, rowId: 8, plan: "other", app },
          "completed",
        );
      return receipt(saved, "completed");
    });
    const { result, onDone } = setup();
    await act(async () => {
      await result.current.finish();
    });
    await act(async () => {
      await result.current.finish();
    });
    expect(mock.query).toHaveBeenCalledWith("claude", OLD_OPERATION_ID);
    expect(mock.prepare).toHaveBeenCalledOnce();
    expect(mock.switchPrepared).toHaveBeenCalledOnce();
    expect(onDone).toHaveBeenCalledOnce();
  });

  it("rejects contradictory target identities for the same recovered operation", async () => {
    mock.query.mockImplementation(async (app: string, id: string | null) => {
      if (app !== "claude") return { status: "none" };
      return id === null
        ? {
            ...receipt({
              operationId: OLD_OPERATION_ID,
              rowId: 8,
              plan: "other",
              app,
            }),
            status: "unknown",
            operationState: "applying",
          }
        : receipt(
            { operationId: OLD_OPERATION_ID, rowId: 7, plan: "a", app },
            "completed",
          );
    });
    const { result, onDone } = setup();
    await act(async () => {
      await result.current.finish();
    });
    await act(async () => {
      await result.current.finish();
    });
    expect(mock.query).toHaveBeenCalledWith("claude", OLD_OPERATION_ID);
    expect(result.current.results.claude?.state).toBe("unverified");
    expect(mock.prepare).not.toHaveBeenCalled();
    expect(mock.switchPrepared).not.toHaveBeenCalled();
    expect(onDone).not.toHaveBeenCalled();
  });

  it("never downgrades a bound operation to a redacted lookup hint", async () => {
    let saved: any;
    let reads = 0;
    mock.prepare.mockImplementation(async (input: any) => {
      saved = input;
      return receipt(input);
    });
    mock.switchPrepared.mockRejectedValue(new Error("lost"));
    mock.query.mockImplementation(async (app: string, id: string | null) => {
      if (app !== "claude" || id === null) return { status: "none" };
      reads++;
      if (reads === 1) return { status: "unknown", app, operationId: id };
      return { ...receipt(saved, "completed"), rowId: 8, plan: "other" };
    });
    const { result, onDone } = setup();
    await act(async () => {
      await result.current.finish();
    });
    await act(async () => {
      await result.current.finish();
    });
    expect(mock.prepare).toHaveBeenCalledOnce();
    expect(mock.switchPrepared).toHaveBeenCalledOnce();
    expect(result.current.results.claude?.state).toBe("unverified");
    expect(onDone).not.toHaveBeenCalled();
  });

  it("does not invent a completed configuration revision", async () => {
    mock.query.mockResolvedValue({
      ...receipt(
        { operationId: OLD_OPERATION_ID, rowId: 7, plan: "a", app: "claude" },
        "completed",
      ),
      completedConfigurationRevision: null,
    });
    const { result, onDone } = setup();
    await act(async () => {
      await result.current.finish();
    });
    expect(result.current.results.claude?.state).toBe("unverified");
    expect(mock.state).not.toHaveBeenCalled();
    expect(mock.switchPrepared).not.toHaveBeenCalled();
    expect(mock.legacy).not.toHaveBeenCalled();
    expect(onDone).not.toHaveBeenCalled();
  });
  it("rechecks a successful app without rewriting it when another app is unknown", async () => {
    const byId = new Map<string, any>();
    mock.prepare.mockImplementation(async (i: any) => {
      byId.set(i.operationId, i);
      return receipt(i);
    });
    mock.query.mockImplementation(async (app: string, id: string | null) =>
      id === null
        ? { status: "none" }
        : app === "claude"
          ? receipt(byId.get(id), "completed")
          : { status: "unknown" },
    );
    const { result } = setup();
    act(() => result.current.setSelection({ codex: "b" }));
    await act(async () => {
      await result.current.finish();
    });
    expect(result.current.results.claude?.state).toBe("success");
    expect(result.current.results.codex?.state).toBe("unverified");
    await act(async () => {
      await result.current.finish();
    });
    expect(mock.switchPrepared.mock.calls.map(([i]) => i.app)).toEqual([
      "claude",
      "codex",
    ]);
    expect(mock.complete).not.toHaveBeenCalled();
    expect(mock.state.mock.calls.map(([app]) => app)).toEqual([
      "claude",
      "claude",
    ]);
  });
});
