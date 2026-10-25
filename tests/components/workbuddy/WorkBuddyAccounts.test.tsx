import { beforeEach, expect, it, vi } from "vitest";
import { StrictMode } from "react";
import { act, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { WorkBuddyAccounts } from "@/components/workbuddy/WorkBuddyAccounts";
import type { WorkBuddyAccount } from "@/lib/api/workbuddy";
const api = vi.hoisted(() => ({
  automaticStatus: vi.fn(),
  list: vi.fn(),
  refresh: vi.fn(),
  refreshAll: vi.fn(),
  claim: vi.fn(),
  claimAll: vi.fn(),
  setAutomatic: vi.fn(),
  beginLogin: vi.fn(),
  cancelLogin: vi.fn(),
  finishLogin: vi.fn(),
  listInferenceAccounts: vi.fn(),
  beginInferenceLogin: vi.fn(),
  setInferenceConsent: vi.fn(),
  pollInferenceLogin: vi.fn(),
  cancelInferenceLogin: vi.fn(),
  discoverModels: vi.fn(),
  openExternal: vi.fn().mockResolvedValue(undefined),
}));
vi.mock("@/lib/api/automaticClaim", () => ({
  automaticClaimApi: { status: api.automaticStatus },
}));
vi.mock("@/lib/api/workbuddy", () => ({ workbuddyApi: api }));
vi.mock("@/lib/api/settings", () => ({
  settingsApi: { openExternal: api.openExternal },
}));
vi.mock("react-i18next", () => ({
  useTranslation: () => ({ t: (key: string) => key }),
}));
const row: WorkBuddyAccount = {
  generation: 0,
  autoClaimEnabled: false,
  autoClaimRevision: 0,
  autoClaimPolicyVersion: 1,
  canSetAutomatic: true,
  autoClaimBlocked: false,
  id: "cn-account-a",
  label: "Synthetic account",
  canRefresh: true,
  canClaim: true,
  claimState: "available",
  credited: null,
  credits: {
    totalRemaining: null,
    nearestExpiry: null,
    updatedAt: null,
    packages: [
      { id: "r1", name: "Synthetic package", remaining: 0, expireAt: null },
    ],
  },
};
beforeEach(() => {
  vi.clearAllMocks();
  api.automaticStatus.mockResolvedValue({
    provider: "workbuddy",
    phase: "waiting",
    lastCheckedAt: null,
    nextCheckAt: null,
    result: null,
    reason: null,
  });
  api.list.mockResolvedValue([row]);
  api.listInferenceAccounts.mockResolvedValue([
    { id: row.id, state: "needsAuthorization" },
  ]);
  api.setInferenceConsent.mockResolvedValue(false);
  api.beginInferenceLogin.mockResolvedValue({
    flowId: "inference-a",
    accountId: row.id,
    verificationUri: "https://copilot.tencent.com/login?state=synthetic",
  });
  api.pollInferenceLogin.mockResolvedValue({ state: "waiting" });
  api.cancelInferenceLogin.mockResolvedValue(undefined);
  api.cancelLogin.mockResolvedValue(undefined);
  api.discoverModels.mockResolvedValue([
    { id: "live-cli-model", name: "Live model" },
  ]);
  api.refresh.mockResolvedValue(row);
  api.refreshAll.mockResolvedValue([row]);
  api.claim.mockResolvedValue({
    ...row,
    claimState: "unconfirmed",
    canClaim: false,
  });
  api.claimAll.mockResolvedValue([]);
  api.setAutomatic.mockResolvedValue({
    ...row,
    autoClaimEnabled: true,
    autoClaimRevision: 1,
  });
});
it("mount and refresh never claim, and unknown balance is not zero", async () => {
  render(<WorkBuddyAccounts />);
  await screen.findByText("Synthetic account");
  expect(screen.getByTestId("workbuddy-total")).toHaveTextContent("—");
  expect(api.refresh).not.toHaveBeenCalled();
  expect(api.claim).not.toHaveBeenCalled();
  await userEvent.click(screen.getByRole("button", { name: "common.refresh" }));
  await waitFor(() => expect(api.refresh).toHaveBeenCalledWith("cn-account-a"));
  await userEvent.click(
    screen.getByRole("button", { name: "workbuddy.refreshAll" }),
  );
  await waitFor(() => expect(api.refreshAll).toHaveBeenCalledOnce());
  expect(api.claim).not.toHaveBeenCalled();
  await userEvent.click(
    screen.getByRole("button", { name: "workbuddy.packages" }),
  );
  expect(await screen.findByText("Synthetic package")).toBeVisible();
});
it("explicit claim is single flight and uncertain readback stays unconfirmed", async () => {
  let resolve!: (v: typeof row) => void;
  api.claim.mockImplementation(
    () =>
      new Promise((r) => {
        resolve = r;
      }),
  );
  render(<WorkBuddyAccounts />);
  await screen.findByText("Synthetic account");
  const button = screen.getByRole("button", { name: "workbuddy.claim" });
  await userEvent.dblClick(button);
  expect(api.claim).toHaveBeenCalledTimes(1);
  resolve({ ...row, claimState: "unconfirmed", canClaim: false });
  expect(await screen.findByText("workbuddy.state.unconfirmed")).toBeVisible();
});
it("backend capabilities control the claim action", async () => {
  api.list.mockResolvedValue([
    { ...row, claimState: "needsVerification", canClaim: false },
  ]);
  render(<WorkBuddyAccounts />);
  await screen.findByText("Synthetic account");
  expect(
    screen.getByRole("button", { name: "workbuddy.claim" }),
  ).toBeDisabled();
  expect(screen.getByText("workbuddy.state.needsVerification")).toBeVisible();
});
it("untrusted transport errors do not reach UI and failed refresh retains snapshot", async () => {
  api.refresh.mockRejectedValue(new Error("SYNTHETIC_SECRET_CANARY"));
  render(<WorkBuddyAccounts />);
  await screen.findByText("Synthetic account");
  await userEvent.click(screen.getByRole("button", { name: "common.refresh" }));
  expect(await screen.findByRole("alert")).toHaveTextContent(
    "workbuddy.failed",
  );
  expect(screen.queryByText(/SYNTHETIC_SECRET_CANARY/)).not.toBeInTheDocument();
  expect(screen.getByText("Synthetic account")).toBeVisible();
});
it("adding an account starts only on click and saves after explicit authorization completion", async () => {
  api.beginLogin.mockResolvedValue({
    flowId: "f1",
    verificationUri: "https://www.codebuddy.cn/login?state=synthetic",
  });
  api.finishLogin.mockResolvedValue({ state: "saved" });
  render(<WorkBuddyAccounts />);
  await screen.findByText("Synthetic account");
  expect(api.beginLogin).not.toHaveBeenCalled();
  await userEvent.click(screen.getByRole("button", { name: "workbuddy.add" }));
  const link = await screen.findByRole("link", { name: "workbuddy.authorize" });
  expect(link).toHaveAttribute(
    "href",
    "https://www.codebuddy.cn/login?state=synthetic",
  );
  expect(api.openExternal).not.toHaveBeenCalled();
  await userEvent.click(link);
  expect(api.openExternal).toHaveBeenCalledWith(
    "https://www.codebuddy.cn/login?state=synthetic",
  );
  expect(api.finishLogin).not.toHaveBeenCalled();
  await userEvent.click(
    screen.getByRole("button", { name: "workbuddy.finishAuthorization" }),
  );
  await waitFor(() => expect(api.finishLogin).toHaveBeenCalledWith("f1"));
  expect(api.claim).not.toHaveBeenCalled();
});

it.each(["success", "failure"] as const)(
  "retains the saved authorization list when the initial read settles late with %s",
  async (outcome) => {
    let resolveInitial!: (rows: WorkBuddyAccount[]) => void;
    let rejectInitial!: (error: Error) => void;
    api.list
      .mockImplementationOnce(
        () =>
          new Promise<WorkBuddyAccount[]>((resolve, reject) => {
            resolveInitial = resolve;
            rejectInitial = reject;
          }),
      )
      .mockResolvedValueOnce([row]);
    api.beginLogin.mockResolvedValue({
      flowId: "f1",
      verificationUri: "https://www.codebuddy.cn/login?state=synthetic",
    });
    api.finishLogin.mockResolvedValue({ state: "saved" });
    render(<WorkBuddyAccounts />);
    await userEvent.click(
      screen.getByRole("button", { name: "workbuddy.add" }),
    );
    await userEvent.click(
      await screen.findByRole("button", {
        name: "workbuddy.finishAuthorization",
      }),
    );
    expect(await screen.findByText("Synthetic account")).toBeVisible();
    await act(async () => {
      if (outcome === "success") resolveInitial([]);
      else rejectInitial(new Error("SYNTHETIC_OLD_LIST_FAILURE"));
    });
    expect(screen.getByText("Synthetic account")).toBeVisible();
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
    expect(screen.queryByText("common.loading")).not.toBeInTheDocument();
    expect(api.list).toHaveBeenCalledTimes(3);
    expect(api.finishLogin).toHaveBeenCalledOnce();
    expect(api.claim).not.toHaveBeenCalled();
    expect(api.claimAll).not.toHaveBeenCalled();
  },
);

it("keeps the newer authorization read pending when the initial read finishes", async () => {
  let resolveInitial!: (rows: WorkBuddyAccount[]) => void;
  let resolveSaved!: (rows: WorkBuddyAccount[]) => void;
  api.list
    .mockImplementationOnce(
      () =>
        new Promise<WorkBuddyAccount[]>(
          (resolve) => (resolveInitial = resolve),
        ),
    )
    .mockImplementationOnce(() => new Promise<WorkBuddyAccount[]>(() => {}))
    .mockImplementationOnce(
      () =>
        new Promise<WorkBuddyAccount[]>((resolve) => (resolveSaved = resolve)),
    );
  api.beginLogin.mockResolvedValue({
    flowId: "f1",
    verificationUri: "https://www.codebuddy.cn/login?state=synthetic",
  });
  api.finishLogin.mockResolvedValue({ state: "saved" });
  render(<WorkBuddyAccounts />);
  await userEvent.click(screen.getByRole("button", { name: "workbuddy.add" }));
  await userEvent.click(
    await screen.findByRole("button", {
      name: "workbuddy.finishAuthorization",
    }),
  );
  await waitFor(() => expect(api.list).toHaveBeenCalledTimes(3));
  await act(async () => resolveInitial([]));
  expect(screen.getByText("common.loading")).toBeVisible();
  expect(screen.queryByText("workbuddy.empty")).not.toBeInTheDocument();
  await act(async () => resolveSaved([row]));
  expect(screen.getByText("Synthetic account")).toBeVisible();
  expect(screen.queryByText("common.loading")).not.toBeInTheDocument();
});

it("ignores the retired mount read after StrictMode restarts the effect", async () => {
  let resolveRetired!: (rows: WorkBuddyAccount[]) => void;
  api.list
    .mockImplementationOnce(
      () =>
        new Promise<WorkBuddyAccount[]>(
          (resolve) => (resolveRetired = resolve),
        ),
    )
    .mockResolvedValueOnce([row]);
  render(
    <StrictMode>
      <WorkBuddyAccounts />
    </StrictMode>,
  );
  expect(await screen.findByText("Synthetic account")).toBeVisible();
  await act(async () => resolveRetired([]));
  expect(screen.getByText("Synthetic account")).toBeVisible();
  expect(api.list).toHaveBeenCalledTimes(3);
  expect(api.beginLogin).not.toHaveBeenCalled();
});

it("clears an initial read failure when authorization starts a replacement read", async () => {
  let rejectInitial!: (error: Error) => void;
  let resolveFinish!: (value: { state: "saved" }) => void;
  const initial = new Promise<WorkBuddyAccount[]>(
    (_, reject) => (rejectInitial = reject),
  );
  api.list
    .mockReturnValueOnce(initial)
    .mockReturnValueOnce(initial)
    .mockResolvedValueOnce([row]);
  api.beginLogin.mockResolvedValue({
    flowId: "f1",
    verificationUri: "https://www.codebuddy.cn/login?state=synthetic",
  });
  api.finishLogin.mockImplementationOnce(
    () => new Promise((resolve) => (resolveFinish = resolve)),
  );
  render(<WorkBuddyAccounts />);
  await userEvent.click(screen.getByRole("button", { name: "workbuddy.add" }));
  await userEvent.click(
    await screen.findByRole("button", {
      name: "workbuddy.finishAuthorization",
    }),
  );
  await act(async () => rejectInitial(new Error("SYNTHETIC_INITIAL_FAILURE")));
  expect(screen.getByRole("alert")).toHaveTextContent("workbuddy.failed");
  await act(async () => resolveFinish({ state: "saved" }));
  expect(await screen.findByText("Synthetic account")).toBeVisible();
  expect(screen.queryByRole("alert")).not.toBeInTheDocument();
});

it("does not let a pending initial read hide a newer authorization failure", async () => {
  let resolveInitial!: (rows: WorkBuddyAccount[]) => void;
  api.list.mockImplementationOnce(
    () =>
      new Promise<WorkBuddyAccount[]>((resolve) => (resolveInitial = resolve)),
  );
  api.beginLogin.mockRejectedValueOnce(new Error("SYNTHETIC_AUTH_FAILURE"));
  render(<WorkBuddyAccounts />);
  await userEvent.click(screen.getByRole("button", { name: "workbuddy.add" }));
  expect(await screen.findByRole("alert")).toHaveTextContent(
    "workbuddy.failed",
  );
  await act(async () => resolveInitial([row]));
  expect(screen.getByText("Synthetic account")).toBeVisible();
  expect(screen.getByRole("alert")).toHaveTextContent("workbuddy.failed");
});

it("retains claim results if the saved authorization list cannot be read", async () => {
  api.claimAll.mockResolvedValueOnce([{ account: row, error: "busy" }]);
  api.list
    .mockResolvedValueOnce([row])
    .mockResolvedValueOnce([row])
    .mockRejectedValueOnce(new Error("SYNTHETIC_LIST_FAILURE"));
  api.beginLogin.mockResolvedValue({
    flowId: "f1",
    verificationUri: "https://www.codebuddy.cn/login?state=synthetic",
  });
  api.finishLogin.mockResolvedValue({ state: "saved" });
  render(<WorkBuddyAccounts />);
  await screen.findByText("Synthetic account");
  await userEvent.click(
    screen.getByRole("button", { name: "workbuddy.claimAll" }),
  );
  const results = await screen.findByRole("region", {
    name: "workbuddy.claimResults",
  });
  await userEvent.click(screen.getByRole("button", { name: "workbuddy.add" }));
  await userEvent.click(
    await screen.findByRole("button", {
      name: "workbuddy.finishAuthorization",
    }),
  );
  expect(await screen.findByRole("alert")).toHaveTextContent(
    "workbuddy.failed",
  );
  expect(results).toBeVisible();
  expect(within(results).getByText("workbuddy.claimFailed")).toBeVisible();
  expect(screen.getAllByText("Synthetic account")).toHaveLength(2);
});

it("batch claim is explicit, single flight and applies only backend results", async () => {
  let resolve!: (value: { account: typeof row; error: null }[]) => void;
  api.claimAll.mockImplementation(
    () =>
      new Promise((done) => {
        resolve = done;
      }),
  );
  render(<WorkBuddyAccounts />);
  await screen.findByText("Synthetic account");
  expect(api.claimAll).not.toHaveBeenCalled();
  const button = screen.getByRole("button", { name: "workbuddy.claimAll" });
  await userEvent.dblClick(button);
  expect(api.claimAll).toHaveBeenCalledOnce();
  expect(api.claim).not.toHaveBeenCalled();
  expect(
    screen.getByRole("button", { name: "workbuddy.claim" }),
  ).toBeDisabled();
  expect(
    screen.getByRole("button", { name: "workbuddy.refreshAll" }),
  ).toBeDisabled();
  resolve([
    {
      account: {
        ...row,
        claimState: "claimed",
        canClaim: false,
        credited: 20,
        credits: { ...row.credits, totalRemaining: 30 },
      },
      error: null,
    },
  ]);
  const results = await screen.findByRole("region", {
    name: "workbuddy.claimResults",
  });
  expect(within(results).getByText("workbuddy.state.claimed")).toBeVisible();
  expect(screen.getByTestId("workbuddy-total")).toHaveTextContent("30");
  expect(button).toBeDisabled();
});

it("batch partial failures retain per-account results and allow an explicit account retry", async () => {
  const other = { ...row, id: "cn-account-b", label: "Synthetic account B" };
  const claimed = {
    ...row,
    claimState: "claimed",
    canClaim: false,
    credited: 20,
  };
  api.list.mockResolvedValue([row, other]);
  api.claimAll.mockResolvedValue([
    { account: claimed, error: null },
    { account: other, error: "busy" },
  ]);
  api.claim.mockResolvedValue({
    ...other,
    claimState: "claimed",
    canClaim: false,
    credited: 20,
  });
  render(<WorkBuddyAccounts />);
  await screen.findByText("Synthetic account B");
  await userEvent.click(
    screen.getByRole("button", { name: "workbuddy.claimAll" }),
  );
  const results = await screen.findByRole("region", {
    name: "workbuddy.claimResults",
  });
  expect(within(results).getByText("workbuddy.claimFailed")).toBeVisible();
  expect(within(results).getByText("workbuddy.state.claimed")).toBeVisible();
  const cards = screen.getAllByRole("article");
  expect(
    within(cards[0]).getByRole("button", { name: "workbuddy.claim" }),
  ).toBeDisabled();
  await userEvent.click(
    within(cards[1]).getByRole("button", { name: "workbuddy.claim" }),
  );
  await waitFor(() => expect(api.claim).toHaveBeenCalledWith("cn-account-b"));
  await waitFor(() =>
    expect(
      within(results).queryByText("workbuddy.claimFailed"),
    ).not.toBeInTheDocument(),
  );
  expect(within(results).getAllByText("workbuddy.state.claimed")).toHaveLength(
    2,
  );
  expect(api.claimAll).toHaveBeenCalledOnce();
});

it("batch eligibility uses backend capabilities", async () => {
  api.list.mockResolvedValue([
    { ...row, claimState: "needsVerification", canClaim: false },
  ]);
  render(<WorkBuddyAccounts />);
  await screen.findByText("Synthetic account");
  expect(
    screen.getByRole("button", { name: "workbuddy.claimAll" }),
  ).toBeDisabled();
  expect(api.claimAll).not.toHaveBeenCalled();
});

it("batch accepts an empty fresh backend selection without making account claims", async () => {
  render(<WorkBuddyAccounts />);
  await screen.findByText("Synthetic account");
  await userEvent.click(
    screen.getByRole("button", { name: "workbuddy.claimAll" }),
  );
  expect(await screen.findByText("workbuddy.claimNone")).toBeVisible();
  expect(api.claim).not.toHaveBeenCalled();
});

it("batch transport failure preserves snapshots and redacts untrusted errors", async () => {
  api.claimAll.mockRejectedValue(new Error("SYNTHETIC_BATCH_SECRET_CANARY"));
  render(<WorkBuddyAccounts />);
  await screen.findByText("Synthetic account");
  await userEvent.click(
    screen.getByRole("button", { name: "workbuddy.claimAll" }),
  );
  expect(await screen.findByRole("alert")).toHaveTextContent(
    "workbuddy.failed",
  );
  expect(
    screen.queryByText(/SYNTHETIC_BATCH_SECRET_CANARY/),
  ).not.toBeInTheDocument();
  expect(screen.getByText("Synthetic account")).toBeVisible();
  expect(api.claimAll).toHaveBeenCalledOnce();
  expect(api.claim).not.toHaveBeenCalled();
});

it("inference requires explicit same-row authorization; legacy check-in never makes it ready", async () => {
  render(<WorkBuddyAccounts />);
  await screen.findByText("Synthetic account");
  expect(api.beginInferenceLogin).not.toHaveBeenCalled();
  expect(api.discoverModels).not.toHaveBeenCalled();
  expect(
    screen.getByRole("button", { name: "workbuddy.inference.models" }),
  ).toBeDisabled();
  await userEvent.click(
    screen.getByRole("button", { name: "workbuddy.inference.authorize" }),
  );
  expect(api.beginInferenceLogin).toHaveBeenCalledWith(row.id);
  const link = await screen.findByRole("link", {
    name: "workbuddy.inference.open",
  });
  expect(api.openExternal).not.toHaveBeenCalled();
  await userEvent.click(link);
  expect(api.openExternal).toHaveBeenCalledWith(
    "https://copilot.tencent.com/login?state=synthetic",
  );
  api.pollInferenceLogin.mockResolvedValueOnce({ state: "saved" });
  api.listInferenceAccounts.mockResolvedValueOnce([
    { id: row.id, state: "ready" },
  ]);
  await userEvent.click(
    screen.getByRole("button", { name: "workbuddy.inference.finish" }),
  );
  await waitFor(() =>
    expect(
      screen.getByRole("button", { name: "workbuddy.inference.models" }),
    ).toBeEnabled(),
  );
  expect(api.discoverModels).not.toHaveBeenCalled();
  await userEvent.click(
    screen.getByRole("button", { name: "workbuddy.inference.models" }),
  );
  expect(api.discoverModels).toHaveBeenCalledWith(row.id);
  expect(await screen.findByText("Live model · live-cli-model")).toBeVisible();
  expect(api.claim).not.toHaveBeenCalled();
});
it.each(["close", "unmount"])(
  "cancels late begin after %s without publishing a stale link",
  async (action) => {
    let complete!: (flow: unknown) => void;
    api.beginInferenceLogin.mockImplementationOnce(
      () =>
        new Promise((resolve) => {
          complete = resolve;
        }),
    );
    const view = render(<WorkBuddyAccounts />);
    await screen.findByText("Synthetic account");
    await userEvent.click(
      screen.getByRole("button", { name: "workbuddy.inference.authorize" }),
    );
    if (action === "close")
      await userEvent.click(
        screen.getByRole("button", { name: "workbuddy.inference.cancel" }),
      );
    else view.unmount();
    await act(async () =>
      complete({
        flowId: "late-flow",
        accountId: row.id,
        verificationUri: "https://copilot.tencent.com/login?state=late",
      }),
    );
    await waitFor(() =>
      expect(api.cancelInferenceLogin).toHaveBeenCalledWith("late-flow"),
    );
    expect(
      screen.queryByRole("link", { name: "workbuddy.inference.open" }),
    ).not.toBeInTheDocument();
    expect(api.pollInferenceLogin).not.toHaveBeenCalled();
  },
);
it("closing pending poll and opening another row rejects old completion without cancelling new flow", async () => {
  api.list.mockResolvedValue([
    row,
    { ...row, id: "cn-account-b", label: "Synthetic B" },
  ]);
  let complete!: (value: unknown) => void;
  api.pollInferenceLogin.mockImplementationOnce(
    () =>
      new Promise((resolve) => {
        complete = resolve;
      }),
  );
  render(<WorkBuddyAccounts />);
  await screen.findByText("Synthetic B");
  await userEvent.click(
    screen.getAllByRole("button", { name: "workbuddy.inference.authorize" })[0],
  );
  await screen.findByRole("link", { name: "workbuddy.inference.open" });
  await userEvent.click(
    screen.getByRole("button", { name: "workbuddy.inference.finish" }),
  );
  await userEvent.click(
    screen.getByRole("button", { name: "workbuddy.inference.cancel" }),
  );
  expect(api.cancelInferenceLogin).toHaveBeenCalledWith("inference-a");
  api.beginInferenceLogin.mockResolvedValueOnce({
    flowId: "inference-b",
    accountId: "cn-account-b",
    verificationUri: "https://copilot.tencent.com/login?state=b",
  });
  await userEvent.click(
    screen.getAllByRole("button", { name: "workbuddy.inference.authorize" })[1],
  );
  await screen.findByRole("link", { name: "workbuddy.inference.open" });
  await act(async () => complete({ state: "saved" }));
  expect(
    screen.getByRole("link", { name: "workbuddy.inference.open" }),
  ).toHaveAttribute("href", "https://copilot.tencent.com/login?state=b");
  expect(api.cancelInferenceLogin).not.toHaveBeenCalledWith("inference-b");
  expect(api.listInferenceAccounts).toHaveBeenCalledTimes(1);
});
it("discovery late response after reauthorization does not repopulate stale models or raw errors", async () => {
  api.listInferenceAccounts.mockResolvedValue([{ id: row.id, state: "ready" }]);
  let complete!: (value: unknown) => void;
  api.discoverModels.mockImplementationOnce(
    () =>
      new Promise((resolve) => {
        complete = resolve;
      }),
  );
  render(<WorkBuddyAccounts />);
  await screen.findByText("Synthetic account");
  await userEvent.click(
    screen.getByRole("button", { name: "workbuddy.inference.models" }),
  );
  await userEvent.click(
    screen.getByRole("button", { name: "workbuddy.inference.authorize" }),
  );
  await screen.findByRole("link", { name: "workbuddy.inference.open" });
  await act(async () => complete([{ id: "stale", name: "Stale model" }]));
  expect(screen.queryByText(/Stale model/)).not.toBeInTheDocument();
  api.pollInferenceLogin.mockRejectedValueOnce(
    new Error("SYNTHETIC_SECRET_CANARY"),
  );
  await userEvent.click(
    screen.getByRole("button", { name: "workbuddy.inference.finish" }),
  );
  expect(await screen.findByRole("alert")).toHaveTextContent(
    "workbuddy.inference.failed",
  );
  expect(screen.queryByText(/SYNTHETIC_SECRET_CANARY/)).not.toBeInTheDocument();
});

it.each(["cancel", "open"])(
  "old %s rejection cannot mark the reopened flow as failed",
  async (operation) => {
    let reject!: (error: Error) => void;
    if (operation === "cancel")
      api.cancelInferenceLogin.mockImplementationOnce(
        () =>
          new Promise((_, fail) => {
            reject = fail;
          }),
      );
    else
      api.openExternal.mockImplementationOnce(
        () =>
          new Promise((_, fail) => {
            reject = fail;
          }),
      );
    render(<WorkBuddyAccounts />);
    await screen.findByText("Synthetic account");
    await userEvent.click(
      screen.getByRole("button", { name: "workbuddy.inference.authorize" }),
    );
    const link = await screen.findByRole("link", {
      name: "workbuddy.inference.open",
    });
    if (operation === "open") await userEvent.click(link);
    await userEvent.click(
      screen.getByRole("button", { name: "workbuddy.inference.cancel" }),
    );
    api.beginInferenceLogin.mockResolvedValueOnce({
      flowId: "reopened-flow",
      accountId: row.id,
      verificationUri: "https://copilot.tencent.com/login?state=reopened",
    });
    await userEvent.click(
      screen.getByRole("button", { name: "workbuddy.inference.authorize" }),
    );
    await screen.findByRole("link", { name: "workbuddy.inference.open" });
    await act(async () => reject(new Error("SYNTHETIC_SECRET_CANARY")));
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
    expect(
      screen.getByRole("link", { name: "workbuddy.inference.open" }),
    ).toHaveAttribute(
      "href",
      "https://copilot.tencent.com/login?state=reopened",
    );
  },
);

async function saveLegacyAuthorization() {
  api.beginLogin.mockResolvedValue({
    flowId: "legacy-renewal",
    verificationUri: "https://www.codebuddy.cn/login?state=synthetic-renewal",
  });
  api.finishLogin.mockResolvedValue({ state: "saved" });
  await userEvent.click(screen.getByRole("button", { name: "workbuddy.add" }));
  await screen.findByRole("link", { name: "workbuddy.authorize" });
  await userEvent.click(
    screen.getByRole("button", { name: "workbuddy.finishAuthorization" }),
  );
  await waitFor(() =>
    expect(
      screen.queryByRole("link", { name: "workbuddy.authorize" }),
    ).not.toBeInTheDocument(),
  );
  await waitFor(() =>
    expect(screen.getByRole("button", { name: "workbuddy.add" })).toBeEnabled(),
  );
}

it.each(["success", "failure"] as const)(
  "legacy same-account reauthorization retires late inference discovery %s",
  async (outcome) => {
    api.listInferenceAccounts.mockResolvedValueOnce([
      { id: row.id, state: "ready" },
    ]);
    let resolve!: (value: unknown) => void;
    let reject!: (error: Error) => void;
    api.discoverModels.mockImplementationOnce(
      () =>
        new Promise((ok, fail) => {
          resolve = ok;
          reject = fail;
        }),
    );
    render(<WorkBuddyAccounts />);
    await screen.findByText(row.label);
    await userEvent.click(
      screen.getByRole("button", { name: "workbuddy.inference.models" }),
    );
    await saveLegacyAuthorization();
    expect(
      screen.getByText("workbuddy.inference.state.needsAuthorization"),
    ).toBeVisible();
    await act(async () => {
      if (outcome === "success")
        resolve([{ id: "retired-model", name: "Retired model" }]);
      else reject(new Error("SYNTHETIC_RETIRED_SECRET"));
    });
    expect(
      screen.queryByText(/Retired model|SYNTHETIC_RETIRED_SECRET/),
    ).not.toBeInTheDocument();
    expect(
      screen.queryByText("workbuddy.inference.failed"),
    ).not.toBeInTheDocument();
    expect(
      screen.getByRole("button", { name: "workbuddy.inference.models" }),
    ).toBeDisabled();
  },
);

it("adding another account retires only transient inference UI and allows a fresh discovery", async () => {
  const other = { ...row, id: "cn-account-b", label: "Synthetic account B" };
  api.listInferenceAccounts.mockResolvedValue([
    { id: row.id, state: "ready" },
    { id: other.id, state: "needsAuthorization" },
  ]);
  let resolveOld!: (value: unknown) => void;
  api.discoverModels.mockImplementationOnce(
    () =>
      new Promise((resolve) => {
        resolveOld = resolve;
      }),
  );
  render(<WorkBuddyAccounts />);
  await screen.findByText(row.label);
  await userEvent.click(
    screen.getByRole("button", { name: "workbuddy.inference.models" }),
  );
  api.list.mockResolvedValue([row, other]);
  await saveLegacyAuthorization();
  const cards = screen.getAllByRole("article");
  expect(
    within(cards[0]).getByRole("button", {
      name: "workbuddy.inference.models",
    }),
  ).toBeEnabled();
  expect(
    within(cards[1]).getByRole("button", {
      name: "workbuddy.inference.models",
    }),
  ).toBeDisabled();
  await userEvent.click(
    within(cards[0]).getByRole("button", {
      name: "workbuddy.inference.models",
    }),
  );
  await screen.findByText("Live model · live-cli-model");
  await act(async () =>
    resolveOld([{ id: "retired-model", name: "Retired model" }]),
  );
  expect(screen.getByText("Live model · live-cli-model")).toBeVisible();
  expect(screen.queryByText(/Retired model/)).not.toBeInTheDocument();
  expect(api.discoverModels).toHaveBeenCalledTimes(2);
  expect(api.cancelInferenceLogin).not.toHaveBeenCalled();
});

it("legacy save closes a pending inference poll and ignores its late success", async () => {
  let finishPoll!: (value: unknown) => void;
  api.pollInferenceLogin.mockImplementationOnce(
    () =>
      new Promise((resolve) => {
        finishPoll = resolve;
      }),
  );
  render(<WorkBuddyAccounts />);
  await screen.findByText(row.label);
  await userEvent.click(
    screen.getByRole("button", { name: "workbuddy.inference.authorize" }),
  );
  await screen.findByRole("link", { name: "workbuddy.inference.open" });
  await userEvent.click(
    screen.getByRole("button", { name: "workbuddy.inference.finish" }),
  );
  await saveLegacyAuthorization();
  expect(api.cancelInferenceLogin).toHaveBeenCalledWith("inference-a");
  expect(
    screen.queryByRole("link", { name: "workbuddy.inference.open" }),
  ).not.toBeInTheDocument();
  const reads = api.listInferenceAccounts.mock.calls.length;
  await act(async () => finishPoll({ state: "saved" }));
  expect(api.listInferenceAccounts).toHaveBeenCalledTimes(reads);
  expect(
    screen.getByText("workbuddy.inference.state.needsAuthorization"),
  ).toBeVisible();
});

it.each(["cancel", "unmount"] as const)(
  "late inference begin after legacy save cannot replace the new flow before %s",
  async (cleanup) => {
    let oldBegin!: (value: unknown) => void;
    api.beginInferenceLogin.mockImplementationOnce(
      () =>
        new Promise((resolve) => {
          oldBegin = resolve;
        }),
    );
    const view = render(<WorkBuddyAccounts />);
    await screen.findByText(row.label);
    await userEvent.click(
      screen.getByRole("button", { name: "workbuddy.inference.authorize" }),
    );
    await saveLegacyAuthorization();
    api.beginInferenceLogin.mockResolvedValue({
      flowId: "new-inference",
      accountId: row.id,
      verificationUri: "https://copilot.tencent.com/login?state=new",
    });
    await userEvent.click(
      screen.getByRole("button", { name: "workbuddy.inference.authorize" }),
    );
    const link = await screen.findByRole("link", {
      name: "workbuddy.inference.open",
    });
    await act(async () =>
      oldBegin({
        flowId: "old-inference",
        accountId: row.id,
        verificationUri: "https://copilot.tencent.com/login?state=old",
      }),
    );
    expect(api.cancelInferenceLogin).toHaveBeenCalledWith("old-inference");
    expect(api.cancelInferenceLogin).not.toHaveBeenCalledWith("new-inference");
    expect(link).toHaveAttribute(
      "href",
      "https://copilot.tencent.com/login?state=new",
    );
    if (cleanup === "cancel")
      await userEvent.click(
        screen.getByRole("button", { name: "workbuddy.inference.cancel" }),
      );
    else view.unmount();
    await waitFor(() =>
      expect(api.cancelInferenceLogin).toHaveBeenCalledWith("new-inference"),
    );
    expect(api.finishLogin).toHaveBeenCalledOnce();
  },
);

it("legacy save fences an older inference status read while its account reload is pending", async () => {
  let oldStatus!: (value: unknown) => void;
  let reloadAccounts!: (value: unknown) => void;
  api.listInferenceAccounts.mockImplementationOnce(
    () =>
      new Promise((resolve) => {
        oldStatus = resolve;
      }),
  );
  api.list
    .mockResolvedValueOnce([row])
    .mockResolvedValueOnce([row])
    .mockImplementationOnce(
      () =>
        new Promise((resolve) => {
          reloadAccounts = resolve;
        }),
    );
  api.beginLogin.mockResolvedValue({
    flowId: "legacy",
    verificationUri: "https://www.codebuddy.cn/login?state=synthetic",
  });
  api.finishLogin.mockResolvedValue({ state: "saved" });
  render(<WorkBuddyAccounts />);
  await screen.findByText(row.label);
  await userEvent.click(screen.getByRole("button", { name: "workbuddy.add" }));
  await screen.findByRole("link", { name: "workbuddy.authorize" });
  await userEvent.click(
    screen.getByRole("button", { name: "workbuddy.finishAuthorization" }),
  );
  await waitFor(() => expect(api.list).toHaveBeenCalledTimes(3));
  await act(async () => oldStatus([{ id: row.id, state: "ready" }]));
  expect(
    screen.queryByText("workbuddy.inference.state.ready"),
  ).not.toBeInTheDocument();
  await act(async () => reloadAccounts([row]));
  await waitFor(() =>
    expect(api.listInferenceAccounts).toHaveBeenCalledTimes(2),
  );
  expect(
    screen.getByText("workbuddy.inference.state.needsAuthorization"),
  ).toBeVisible();
});
it("places native authorization, claim and model actions in separate unified detail sections", async () => {
  render(<WorkBuddyAccounts unified />);
  const auth = await screen.findByRole("region", { name: "授权状态" });
  const rights = screen.getByRole("region", { name: "权益与签到" });
  const inference = screen.getByRole("region", { name: "模型与推理" });
  expect(
    within(auth).getByRole("button", { name: "workbuddy.inference.authorize" }),
  ).toBeInTheDocument();
  expect(
    within(rights).getByRole("button", { name: "workbuddy.claim" }),
  ).toBeEnabled();
  expect(
    within(inference).getByRole("button", {
      name: "workbuddy.inference.models",
    }),
  ).toBeDisabled();
  expect(
    within(inference).getByText("automaticStatus.inferenceUnverified"),
  ).toBeInTheDocument();
  await userEvent.click(
    within(rights).getByRole("button", { name: "workbuddy.claim" }),
  );
  await waitFor(() => expect(api.claim).toHaveBeenCalledWith(row.id));
  expect(api.beginInferenceLogin).not.toHaveBeenCalled();
});

it("automatic claim remains off until an explicit account opt-in", async () => {
  render(<WorkBuddyAccounts />);
  await screen.findByText("Synthetic account");
  expect(api.setAutomatic).not.toHaveBeenCalled();
  expect(
    screen.getByRole("switch", { name: "workbuddy.auto.label" }),
  ).toHaveAttribute("aria-checked", "false");
  expect(screen.getByText("workbuddy.auto.notice")).toBeVisible();
});
it("setter success is not discarded by a list refresh started while it is pending", async () => {
  let resolveSetter!: (value: WorkBuddyAccount) => void;
  api.setAutomatic.mockImplementation(
    () =>
      new Promise((resolve) => {
        resolveSetter = resolve;
      }),
  );
  render(<WorkBuddyAccounts />);
  await screen.findByText("Synthetic account");
  await userEvent.click(
    screen.getByRole("switch", { name: "workbuddy.auto.label" }),
  );
  expect(api.setAutomatic).toHaveBeenCalledWith(row, true);
  await userEvent.click(
    screen.getByRole("button", { name: "workbuddy.refreshAll" }),
  );
  await waitFor(() => expect(api.refreshAll).toHaveBeenCalledOnce());
  await act(async () =>
    resolveSetter({ ...row, autoClaimEnabled: true, autoClaimRevision: 1 }),
  );
  expect(screen.getByRole("switch")).toHaveAttribute("aria-checked", "true");
});
it("a late old refresh cannot display off after a successful enable", async () => {
  let resolveSetter!: (value: WorkBuddyAccount) => void;
  let resolveRefresh!: (value: WorkBuddyAccount[]) => void;
  api.setAutomatic.mockImplementation(
    () =>
      new Promise((resolve) => {
        resolveSetter = resolve;
      }),
  );
  api.refreshAll.mockImplementation(
    () =>
      new Promise((resolve) => {
        resolveRefresh = resolve;
      }),
  );
  render(<WorkBuddyAccounts />);
  await screen.findByText("Synthetic account");
  await userEvent.click(screen.getByRole("switch"));
  await userEvent.click(
    screen.getByRole("button", { name: "workbuddy.refreshAll" }),
  );
  await act(async () =>
    resolveSetter({ ...row, autoClaimEnabled: true, autoClaimRevision: 1 }),
  );
  expect(screen.getByRole("switch")).toHaveAttribute("aria-checked", "true");
  await act(async () => resolveRefresh([row]));
  expect(screen.getByRole("switch")).toHaveAttribute("aria-checked", "true");
});
it("newer account generation wins over an older successful enable response", async () => {
  let resolveSetter!: (value: WorkBuddyAccount) => void;
  api.setAutomatic.mockImplementation(
    () =>
      new Promise((resolve) => {
        resolveSetter = resolve;
      }),
  );
  api.refreshAll.mockResolvedValue([
    { ...row, generation: 1, autoClaimRevision: 2, autoClaimEnabled: false },
  ]);
  render(<WorkBuddyAccounts />);
  await screen.findByText("Synthetic account");
  await userEvent.click(screen.getByRole("switch"));
  await userEvent.click(
    screen.getByRole("button", { name: "workbuddy.refreshAll" }),
  );
  await waitFor(() => expect(api.refreshAll).toHaveBeenCalledOnce());
  await act(async () =>
    resolveSetter({ ...row, autoClaimEnabled: true, autoClaimRevision: 1 }),
  );
  expect(screen.getByRole("switch")).toHaveAttribute("aria-checked", "false");
});
it("uncertain opt-in is not displayed as off and repeated clicks do not duplicate setters", async () => {
  let rejectSetter!: (error: Error) => void;
  api.setAutomatic.mockImplementation(
    () =>
      new Promise((_, reject) => {
        rejectSetter = reject;
      }),
  );
  render(<WorkBuddyAccounts />);
  await screen.findByText("Synthetic account");
  await userEvent.dblClick(screen.getByRole("switch"));
  expect(api.setAutomatic).toHaveBeenCalledOnce();
  expect(screen.getByRole("switch")).toHaveAttribute("aria-checked", "false");
  await act(async () => rejectSetter(new Error("uncertain")));
  expect(screen.queryByRole("switch")).not.toBeInTheDocument();
  expect(
    screen.getByRole("button", { name: "workbuddy.auto.unknown" }),
  ).toBeDisabled();
  expect(screen.getByRole("alert")).toHaveTextContent("workbuddy.auto.failed");
});
it("unknown claims display a pause and stale DTOs cannot opt in", async () => {
  api.list.mockResolvedValue([
    { ...row, autoClaimBlocked: true, canClaim: false },
  ]);
  const { unmount } = render(<WorkBuddyAccounts />);
  expect(await screen.findByText("workbuddy.auto.blocked")).toBeVisible();
  expect(
    screen.getByRole("button", { name: "workbuddy.claim" }),
  ).toBeDisabled();
  unmount();
  const legacy = {
    ...row,
    generation: undefined,
    autoClaimRevision: undefined,
  };
  api.list.mockResolvedValue([legacy]);
  render(<WorkBuddyAccounts />);
  await screen.findByText("Synthetic account");
  expect(screen.getByRole("switch")).toBeDisabled();
  expect(api.setAutomatic).not.toHaveBeenCalled();
});
it("an older enabled refresh cannot restore a confirmed opt-out", async () => {
  const enabled = { ...row, autoClaimEnabled: true, autoClaimRevision: 2 };
  let resolveSetter!: (value: WorkBuddyAccount) => void;
  let resolveRefresh!: (value: WorkBuddyAccount[]) => void;
  api.list.mockResolvedValue([enabled]);
  api.setAutomatic.mockImplementation(
    () =>
      new Promise((resolve) => {
        resolveSetter = resolve;
      }),
  );
  api.refreshAll.mockImplementation(
    () =>
      new Promise((resolve) => {
        resolveRefresh = resolve;
      }),
  );
  render(<WorkBuddyAccounts />);
  await screen.findByText("Synthetic account");
  await userEvent.click(screen.getByRole("switch"));
  await userEvent.click(
    screen.getByRole("button", { name: "workbuddy.refreshAll" }),
  );
  await act(async () => resolveSetter({ ...row, autoClaimRevision: 3 }));
  await act(async () => resolveRefresh([enabled]));
  expect(screen.getByRole("switch")).toHaveAttribute("aria-checked", "false");
  expect(api.setAutomatic).toHaveBeenCalledWith(enabled, false);
});

it("lost enable response reads back the committed enabled policy instead of showing off", async () => {
  api.list
    .mockResolvedValueOnce([row])
    .mockResolvedValueOnce([row])
    .mockResolvedValue([
      { ...row, autoClaimEnabled: true, autoClaimRevision: 1 },
    ]);
  api.setAutomatic.mockRejectedValue(new Error("response lost"));
  render(<WorkBuddyAccounts />);
  await screen.findByText("Synthetic account");
  await userEvent.click(screen.getByRole("switch"));
  await waitFor(() =>
    expect(screen.getByRole("switch")).toHaveAttribute("aria-checked", "true"),
  );
  expect(api.list).toHaveBeenCalledTimes(3);
});
it("lost disable response and failed readback display unknown, never a false on or off", async () => {
  api.list
    .mockResolvedValueOnce([
      { ...row, autoClaimEnabled: true, autoClaimRevision: 1 },
    ])
    .mockResolvedValueOnce([
      { ...row, autoClaimEnabled: true, autoClaimRevision: 1 },
    ])
    .mockRejectedValue(new Error("readback unavailable"));
  api.setAutomatic.mockRejectedValue(new Error("response lost"));
  render(<WorkBuddyAccounts />);
  await screen.findByText("Synthetic account");
  await userEvent.click(screen.getByRole("switch"));
  await screen.findByRole("button", { name: "workbuddy.auto.unknown" });
  expect(screen.queryByRole("switch")).not.toBeInTheDocument();
});

it.each(["cancel", "unmount"] as const)(
  "retires a late account login after %s without showing it",
  async (action) => {
    let resolve!: (flow: { flowId: string; verificationUri: string }) => void;
    api.beginLogin.mockImplementationOnce(
      () =>
        new Promise((done) => {
          resolve = done;
        }),
    );
    const view = render(<WorkBuddyAccounts />);
    await screen.findByText("Synthetic account");
    await userEvent.click(
      screen.getByRole("button", { name: "workbuddy.add" }),
    );
    if (action === "cancel")
      await userEvent.click(
        screen.getByRole("button", { name: "common.cancel" }),
      );
    else view.unmount();
    await act(async () =>
      resolve({
        flowId: "late-login",
        verificationUri: "https://www.codebuddy.cn/login?state=late",
      }),
    );
    await waitFor(() =>
      expect(api.cancelLogin).toHaveBeenCalledWith("late-login"),
    );
    expect(
      screen.queryByRole("link", { name: "workbuddy.authorize" }),
    ).not.toBeInTheDocument();
    expect(api.finishLogin).not.toHaveBeenCalled();
  },
);

it("cancels the exact previous login when adding another account", async () => {
  api.beginLogin
    .mockResolvedValueOnce({
      flowId: "old-login",
      verificationUri: "https://www.codebuddy.cn/login?state=old",
    })
    .mockResolvedValueOnce({
      flowId: "new-login",
      verificationUri: "https://www.codebuddy.cn/login?state=new",
    });
  const view = render(<WorkBuddyAccounts />);
  await screen.findByText("Synthetic account");
  await userEvent.click(screen.getByRole("button", { name: "workbuddy.add" }));
  await screen.findByRole("link", { name: "workbuddy.authorize" });
  await userEvent.click(screen.getByRole("button", { name: "workbuddy.add" }));
  await waitFor(() =>
    expect(
      screen.getByRole("link", { name: "workbuddy.authorize" }),
    ).toHaveAttribute("href", "https://www.codebuddy.cn/login?state=new"),
  );
  expect(api.cancelLogin).toHaveBeenCalledWith("old-login");
  view.unmount();
  expect(api.cancelLogin).toHaveBeenCalledWith("new-login");
});

it.each(["saved", "waiting", "failure"] as const)(
  "ignores a retired account finish %s after cancel and a new login",
  async (outcome) => {
    let resolve!: (value: { state: string }) => void;
    let reject!: (error: Error) => void;
    api.beginLogin
      .mockResolvedValueOnce({
        flowId: "old-login",
        verificationUri: "https://www.codebuddy.cn/login?state=old",
      })
      .mockResolvedValueOnce({
        flowId: "new-login",
        verificationUri: "https://www.codebuddy.cn/login?state=new",
      });
    api.finishLogin.mockImplementationOnce(
      () =>
        new Promise((done, fail) => {
          resolve = done;
          reject = fail;
        }),
    );
    render(<WorkBuddyAccounts />);
    await screen.findByText("Synthetic account");
    await userEvent.click(
      screen.getByRole("button", { name: "workbuddy.add" }),
    );
    await userEvent.click(
      await screen.findByRole("button", {
        name: "workbuddy.finishAuthorization",
      }),
    );
    await userEvent.click(
      screen.getByRole("button", { name: "common.cancel" }),
    );
    expect(api.cancelLogin).toHaveBeenCalledWith("old-login");
    await userEvent.click(
      screen.getByRole("button", { name: "workbuddy.add" }),
    );
    await waitFor(() =>
      expect(
        screen.getByRole("link", { name: "workbuddy.authorize" }),
      ).toHaveAttribute("href", "https://www.codebuddy.cn/login?state=new"),
    );
    await act(async () => {
      if (outcome === "failure") reject(new Error("SYNTHETIC_RETIRED_FAILURE"));
      else resolve({ state: outcome });
    });
    expect(
      screen.getByRole("link", { name: "workbuddy.authorize" }),
    ).toHaveAttribute("href", "https://www.codebuddy.cn/login?state=new");
    expect(
      screen.queryByText("workbuddy.authorizationWaiting"),
    ).not.toBeInTheDocument();
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
    expect(api.list).toHaveBeenCalledTimes(2);
    expect(api.claim).not.toHaveBeenCalled();
  },
);

it("reads background completion without claiming, refreshing, or authorizing inference", async () => {
  const { emitTauriEvent } = await import("../../msw/tauriMocks");
  const view = render(<WorkBuddyAccounts />);
  await screen.findByText("Synthetic account");
  api.list.mockResolvedValue([{ ...row, claimState: "claimed", credited: 10 }]);
  act(() =>
    emitTauriEvent("automatic-claim-status-updated", {
      provider: "minimaxCode",
      phase: "waiting",
    }),
  );
  expect(api.list).toHaveBeenCalledTimes(2);
  act(() =>
    emitTauriEvent("automatic-claim-status-updated", {
      provider: "workbuddy",
      phase: "waiting",
    }),
  );
  expect(await screen.findByText("workbuddy.state.claimed")).toBeVisible();
  expect(api.refresh).not.toHaveBeenCalled();
  expect(api.claim).not.toHaveBeenCalled();
  expect(api.claimAll).not.toHaveBeenCalled();
  expect(api.beginInferenceLogin).not.toHaveBeenCalled();
  expect(api.discoverModels).not.toHaveBeenCalled();
  view.unmount();
  const reads = api.list.mock.calls.length;
  act(() =>
    emitTauriEvent("automatic-claim-status-updated", {
      provider: "workbuddy",
      phase: "waiting",
    }),
  );
  expect(api.list).toHaveBeenCalledTimes(reads);
});

it("background catalog reads preserve the manual batch receipt", async () => {
  const { emitTauriEvent } = await import("../../msw/tauriMocks");
  api.claimAll.mockResolvedValue([
    { account: { ...row, claimState: "claimed" }, error: null },
  ]);
  render(<WorkBuddyAccounts />);
  await screen.findByText("Synthetic account");
  await userEvent.click(
    screen.getByRole("button", { name: "workbuddy.claimAll" }),
  );
  await screen.findByRole("region", { name: "workbuddy.claimResults" });
  act(() =>
    emitTauriEvent("automatic-claim-status-updated", {
      provider: "workbuddy",
      phase: "waiting",
    }),
  );
  await waitFor(() => expect(api.list).toHaveBeenCalledTimes(3));
  expect(
    screen.getByRole("region", { name: "workbuddy.claimResults" }),
  ).toBeVisible();
});

it.each([
  ["single", "before"],
  ["single", "during"],
  ["batch", "before"],
  ["batch", "during"],
] as const)(
  "does not let a passive snapshot started %s %s manual completion regress its receipt",
  async (kind, timing) => {
    const { emitTauriEvent } = await import("../../msw/tauriMocks");
    let finishList!: (rows: WorkBuddyAccount[]) => void;
    let finishClaim!: (result: unknown) => void;
    const claimed = {
      ...row,
      claimState: "claimed" as const,
      canClaim: false,
      credited: 10,
    };
    (kind === "batch" ? api.claimAll : api.claim).mockImplementationOnce(
      () =>
        new Promise((resolve) => {
          finishClaim = resolve;
        }),
    );
    render(<WorkBuddyAccounts />);
    await screen.findByText(row.label);
    api.list.mockImplementationOnce(
      () =>
        new Promise((resolve) => {
          finishList = resolve;
        }),
    );
    const emit = () =>
      act(() =>
        emitTauriEvent("automatic-claim-status-updated", {
          provider: "workbuddy",
          phase: "waiting",
        }),
      );
    if (timing === "before") emit();
    await userEvent.click(
      screen.getByRole("button", {
        name: kind === "batch" ? "workbuddy.claimAll" : "workbuddy.claim",
      }),
    );
    if (timing === "during") emit();
    await waitFor(() => expect(api.list).toHaveBeenCalledTimes(3));
    await act(async () =>
      finishClaim(
        kind === "batch" ? [{ account: claimed, error: null }] : claimed,
      ),
    );
    await waitFor(() =>
      expect(
        screen.getAllByText("workbuddy.state.claimed").length,
      ).toBeGreaterThan(0),
    );
    await act(async () => finishList([row]));
    expect(
      screen.queryByText("workbuddy.state.available"),
    ).not.toBeInTheDocument();
    expect(
      screen.getAllByText("workbuddy.state.claimed").length,
    ).toBeGreaterThan(0);
    if (kind === "batch")
      expect(
        screen.getByRole("region", { name: "workbuddy.claimResults" }),
      ).toHaveTextContent("workbuddy.state.claimed");
  },
);

it("reuses a complete login for inference without a second browser or claim", async () => {
  api.setInferenceConsent.mockResolvedValue(true);
  render(<WorkBuddyAccounts />);
  await screen.findByText("Synthetic account");
  await userEvent.click(
    screen.getByRole("button", { name: "workbuddy.inference.authorize" }),
  );
  await waitFor(() =>
    expect(api.setInferenceConsent).toHaveBeenCalledWith(row.id, true),
  );
  expect(api.beginInferenceLogin).not.toHaveBeenCalled();
  expect(api.openExternal).not.toHaveBeenCalled();
  expect(api.claim).not.toHaveBeenCalled();
  expect(api.setAutomatic).not.toHaveBeenCalled();
});

it("stops inference without cancelling shared login or automatic claims", async () => {
  api.listInferenceAccounts.mockResolvedValue([{ id: row.id, state: "ready" }]);
  api.setInferenceConsent.mockResolvedValue(true);
  render(<WorkBuddyAccounts />);
  await userEvent.click(
    await screen.findByRole("button", { name: "workbuddy.inference.disable" }),
  );
  await waitFor(() =>
    expect(api.setInferenceConsent).toHaveBeenCalledWith(row.id, false),
  );
  expect(api.beginInferenceLogin).not.toHaveBeenCalled();
  expect(api.setAutomatic).not.toHaveBeenCalled();
  expect(api.claim).not.toHaveBeenCalled();
});
