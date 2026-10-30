import { act, renderHook, waitFor } from "@testing-library/react";
import { beforeEach, expect, it, vi } from "vitest";
import { useWorkBuddyAccountsViewModel } from "@/components/workbuddy/useWorkBuddyAccountsViewModel";
import { useTraeAccountsViewModel } from "@/components/trae/useTraeAccountsViewModel";
import { useMiniMaxCodeViewModel } from "@/components/providers/forms/useMiniMaxCodeViewModel";
const apis = vi.hoisted(() => ({
  wb: {
    list: vi.fn(),
    listInferenceAccounts: vi.fn(),
    beginLogin: vi.fn(),
    cancelLogin: vi.fn(),
    refresh: vi.fn(),
    claim: vi.fn(),
    claimAll: vi.fn(),
    setAutomatic: vi.fn(),
    setInferenceConsent: vi.fn(),
    beginInferenceLogin: vi.fn(),
    cancelInferenceLogin: vi.fn(),
    discoverModels: vi.fn(),
  },
  trae: {
    list: vi.fn(),
    begin: vi.fn(),
    cancel: vi.fn(),
    confirm: vi.fn(),
    setAutomatic: vi.fn(),
  },
  mm: {
    list: vi.fn(),
    beginLogin: vi.fn(),
    cancelLogin: vi.fn(),
    claim: vi.fn(),
  },
}));
vi.mock("@/lib/api/workbuddy", () => ({ workbuddyApi: apis.wb }));
vi.mock("@/lib/api/traeAccounts", () => ({
  traeAccountsApi: apis.trae,
  traeAuthHost: (realm: string) => realm,
}));
vi.mock("@/lib/api/minimaxCode", () => ({ minimaxCodeApi: apis.mm }));
vi.mock("@/hooks/useAutomaticClaimStatus", () => ({
  useAutomaticClaimStatus: () => ({}),
}));
vi.mock("react-i18next", () => ({
  useTranslation: () => ({ t: (s: string) => s }),
}));
vi.mock("@tauri-apps/plugin-dialog", () => ({ open: vi.fn() }));
function deferred<T>() {
  let resolve!: (v: T) => void;
  const promise = new Promise<T>((r) => {
    resolve = r;
  });
  return { promise, resolve };
}
beforeEach(() => {
  vi.resetAllMocks();
  apis.wb.list.mockResolvedValue([]);
  apis.wb.listInferenceAccounts.mockResolvedValue([]);
  apis.wb.cancelLogin.mockResolvedValue(undefined);
  apis.trae.list.mockResolvedValue([]);
  apis.trae.cancel.mockResolvedValue(undefined);
  apis.mm.list.mockResolvedValue([]);
  apis.mm.cancelLogin.mockResolvedValue(undefined);
});
it("WorkBuddy cancels a begin receipt arriving after explicit cancellation and suppresses duplicate begin", async () => {
  const pending = deferred<any>();
  apis.wb.beginLogin.mockReturnValue(pending.promise);
  const { result } = renderHook(() => useWorkBuddyAccountsViewModel());
  await waitFor(() => expect(result.current.loading).toBe(false));
  let done!: Promise<void>;
  act(() => {
    done = result.current.beginLogin();
    void result.current.beginLogin();
  });
  act(() => result.current.closeLogin());
  await act(async () => {
    pending.resolve({ flowId: "old", verificationUri: "https://example.com" });
    await done;
  });
  expect(result.current.login).toBeNull();
  expect(result.current.busy).toBeNull();
  expect(apis.wb.beginLogin).toHaveBeenCalledTimes(1);
  expect(apis.wb.cancelLogin).toHaveBeenCalledWith("old");
});
it("WorkBuddy keeps a newer refresh receipt when an older list arrives", async () => {
  const row = {
    id: "a",
    generation: 1,
    autoClaimRevision: 2,
    canRefresh: true,
  };
  apis.wb.list.mockResolvedValue([row]);
  const { result } = renderHook(() => useWorkBuddyAccountsViewModel());
  await waitFor(() => expect(result.current.loading).toBe(false));
  const pending = deferred<any[]>();
  apis.wb.list.mockReturnValue(pending.promise);
  let list!: Promise<void>;
  act(() => {
    list = result.current.loadAccounts();
  });
  apis.wb.refresh.mockResolvedValue({ ...row, autoClaimRevision: 3 });
  await act(async () => {
    await result.current.refreshAccount(row as any);
  });
  await act(async () => {
    pending.resolve([row]);
    await list;
  });
  expect(result.current.accounts[0].autoClaimRevision).toBe(3);
});
it("WorkBuddy retains unknown policy after a lost setter reply and stale readback", async () => {
  const row = {
    id: "a",
    generation: 1,
    autoClaimRevision: 2,
    autoClaimPolicyVersion: 1,
    autoClaimEnabled: false,
    canSetAutomatic: true,
  };
  apis.wb.list.mockResolvedValue([row]);
  apis.wb.setAutomatic.mockRejectedValue(new Error("lost"));
  const { result } = renderHook(() => useWorkBuddyAccountsViewModel());
  await waitFor(() => expect(result.current.loading).toBe(false));
  await act(async () => {
    await result.current.setAutomatic(row as any);
  });
  expect(result.current.automaticUncertain.a).toBeDefined();
  expect(apis.wb.setAutomatic).toHaveBeenCalledTimes(1);
});
it("Trae cancels a late begin receipt after closing and does not submit consent implicitly", async () => {
  const pending = deferred<any>();
  apis.trae.begin.mockReturnValue(pending.promise);
  const { result } = renderHook(() => useTraeAccountsViewModel("global"));
  await waitFor(() => expect(result.current.loading).toBe(false));
  act(() => result.current.beginDialog());
  act(() => result.current.chooseRealm("sg"));
  let done!: Promise<void>;
  act(() => {
    done = result.current.selectFiles({
      storagePath: "/test/storage.json",
      productPath: "/test/product.json",
    } as any);
  });
  act(() => result.current.closeDialog());
  await act(async () => {
    pending.resolve({ flowId: "late", realm: "sg", authHost: "sg" });
    await done;
  });
  expect(result.current.flow).toBeNull();
  expect(result.current.dialogOpen).toBe(false);
  expect(apis.trae.cancel).toHaveBeenCalledWith("late");
  expect(apis.trae.confirm).not.toHaveBeenCalled();
});
it("Trae requires consent and submits confirm once even when the receipt is lost", async () => {
  apis.trae.begin.mockResolvedValue({
    flowId: "f",
    realm: "sg",
    authHost: "sg",
  });
  const pending = deferred<any>();
  apis.trae.confirm.mockReturnValue(pending.promise);
  const { result } = renderHook(() => useTraeAccountsViewModel("global"));
  await waitFor(() => expect(result.current.loading).toBe(false));
  act(() => result.current.beginDialog());
  act(() => result.current.chooseRealm("sg"));
  await act(async () => {
    await result.current.selectFiles({
      storagePath: "/test/storage.json",
      productPath: "/test/product.json",
    } as any);
    await result.current.confirm();
  });
  expect(apis.trae.confirm).not.toHaveBeenCalled();
  act(() => result.current.changeConsent(true));
  let done!: Promise<void>;
  act(() => {
    done = result.current.confirm();
    void result.current.confirm();
  });
  await act(async () => {
    pending.resolve(undefined);
    await done;
  });
  expect(result.current.failed).toBe("receiptUnknown");
  expect(result.current.attempted).toBe(true);
  expect(apis.trae.confirm).toHaveBeenCalledTimes(1);
});
it("MiniMax cancels a late begin receipt and permits no duplicate begin", async () => {
  const pending = deferred<any>();
  apis.mm.beginLogin.mockReturnValue(pending.promise);
  const { result } = renderHook(() => useMiniMaxCodeViewModel("cn"));
  await waitFor(() => expect(result.current.loaded).toBe(true));
  let done!: Promise<void>;
  act(() => {
    done = result.current.beginLogin();
    void result.current.beginLogin();
  });
  await act(async () => {
    await result.current.cancelLogin();
  });
  await act(async () => {
    pending.resolve({ flowId: "late" });
    await done;
  });
  expect(result.current.login).toBeNull();
  expect(result.current.loginBusy).toBe(false);
  expect(apis.mm.beginLogin).toHaveBeenCalledTimes(1);
  expect(apis.mm.cancelLogin).toHaveBeenCalledWith("late");
});
it("MiniMax lost claim reads durable unknown state once without replay", async () => {
  const row = { id: "a", region: "cn", authorized: true, canClaim: true };
  apis.mm.list.mockResolvedValue([row]);
  apis.mm.claim.mockRejectedValue(new Error("networkUnavailable"));
  const { result } = renderHook(() => useMiniMaxCodeViewModel("cn"));
  await waitFor(() => expect(result.current.loaded).toBe(true));
  apis.mm.list.mockResolvedValue([
    { ...row, status: "unknown", canClaim: false },
  ]);
  await act(async () => {
    await result.current.runAccount(row as any, "claim");
  });
  expect(result.current.visibleAccounts[0].status).toBe("unknown");
  expect(apis.mm.claim).toHaveBeenCalledTimes(1);
});
it("WorkBuddy command owner refuses a claim the backend marks unavailable", async () => {
  const row = { id: "a", generation: 1, autoClaimRevision: 1, canClaim: false };
  apis.wb.list.mockResolvedValue([row]);
  apis.wb.claim.mockResolvedValue(row);
  const { result } = renderHook(() => useWorkBuddyAccountsViewModel());
  await waitFor(() => expect(result.current.loading).toBe(false));
  await act(async () => {
    await result.current.claimAccount(row as any);
  });
  expect(apis.wb.claim).not.toHaveBeenCalled();
});
it("MiniMax command owner refuses an unavailable claim without hiding unknown", async () => {
  const row = {
    id: "a",
    region: "cn",
    authorized: true,
    status: "unknown",
    canClaim: false,
  };
  apis.mm.list.mockResolvedValue([row]);
  const { result } = renderHook(() => useMiniMaxCodeViewModel("cn"));
  await waitFor(() => expect(result.current.loaded).toBe(true));
  await act(async () => {
    await result.current.runAccount(row as any, "claim");
  });
  expect(apis.mm.claim).not.toHaveBeenCalled();
  expect(result.current.visibleAccounts[0].status).toBe("unknown");
});
it("WorkBuddy inference cancels late authorization after close and never publishes a link", async () => {
  apis.wb.setInferenceConsent.mockResolvedValue(false);
  apis.wb.cancelInferenceLogin.mockResolvedValue(undefined);
  const pending = deferred<any>();
  apis.wb.beginInferenceLogin.mockReturnValue(pending.promise);
  const { result } = renderHook(() => useWorkBuddyAccountsViewModel());
  await waitFor(() => expect(result.current.loading).toBe(false));
  let done!: Promise<void>;
  await act(async () => {
    done = result.current.beginInference("a");
    await Promise.resolve();
  });
  act(() => result.current.closeInference());
  await act(async () => {
    pending.resolve({ flowId: "late", accountId: "a" });
    await done;
  });
  expect(result.current.inferenceLogin).toBeNull();
  expect(result.current.inferenceTarget).toBeNull();
  expect(apis.wb.cancelInferenceLogin).toHaveBeenCalledWith("late");
});
it("Trae rejects an older policy list after a newer policy command receipt", async () => {
  const row = {
    id: "a",
    realm: "cn",
    status: "stored",
    generation: 1,
    autoCheckinRevision: 1,
    autoCheckinEnabled: false,
    canSetAutomatic: true,
  };
  apis.trae.list.mockResolvedValue([row]);
  const { result } = renderHook(() => useTraeAccountsViewModel("cn"));
  await waitFor(() => expect(result.current.loading).toBe(false));
  const pending = deferred<any[]>();
  apis.trae.list.mockReturnValue(pending.promise);
  let list!: Promise<void>;
  act(() => {
    list = result.current.loadAccounts();
  });
  apis.trae.setAutomatic.mockResolvedValue({
    ...row,
    autoCheckinRevision: 2,
    autoCheckinEnabled: true,
  });
  await act(async () => {
    await result.current.setAutomatic(row as any);
  });
  await act(async () => {
    pending.resolve([row]);
    await list;
  });
  expect(result.current.accounts[0].autoCheckinEnabled).toBe(true);
  expect(result.current.accounts[0].autoCheckinRevision).toBe(2);
});
it("Trae lost setter with failed readback remains unknown and is not resent", async () => {
  const row = {
    id: "a",
    realm: "cn",
    status: "stored",
    generation: 1,
    autoCheckinRevision: 1,
    autoCheckinEnabled: false,
    canSetAutomatic: true,
  };
  apis.trae.list.mockResolvedValue([row]);
  const { result } = renderHook(() => useTraeAccountsViewModel("cn"));
  await waitFor(() => expect(result.current.loading).toBe(false));
  apis.trae.setAutomatic.mockRejectedValue(new Error("lost"));
  apis.trae.list.mockRejectedValue(new Error("offline"));
  await act(async () => {
    await result.current.setAutomatic(row as any);
  });
  expect(result.current.policyIsUnknown(result.current.accounts[0])).toBe(true);
  await act(async () => {
    await result.current.setAutomatic(row as any);
  });
  expect(apis.trae.setAutomatic).toHaveBeenCalledTimes(1);
});
it("MiniMax late list cannot replace durable unknown readback after a claim", async () => {
  const row = { id: "a", region: "cn", authorized: true, canClaim: true };
  apis.mm.list.mockResolvedValue([row]);
  const { result } = renderHook(() => useMiniMaxCodeViewModel("cn"));
  await waitFor(() => expect(result.current.loaded).toBe(true));
  const pendingClaim = deferred<any>();
  apis.mm.claim.mockReturnValue(pendingClaim.promise);
  let claim!: Promise<void>;
  act(() => {
    claim = result.current.runAccount(row as any, "claim");
  });
  const pendingList = deferred<any[]>();
  apis.mm.list.mockReturnValueOnce(pendingList.promise);
  let list!: Promise<void>;
  act(() => {
    list = result.current.loadAccounts();
  });
  apis.mm.list.mockResolvedValue([
    { ...row, status: "unknown", canClaim: false },
  ]);
  await act(async () => {
    pendingClaim.resolve(undefined);
    await claim;
  });
  expect(result.current.visibleAccounts[0].status).toBe("unknown");
  await act(async () => {
    pendingList.resolve([row]);
    await list;
  });
  expect(result.current.visibleAccounts[0].status).toBe("unknown");
  expect(apis.mm.claim).toHaveBeenCalledTimes(1);
});

it("WorkBuddy batch command refuses submission when no backend row can claim", async () => {
  apis.wb.list.mockResolvedValue([{ id: "a", canClaim: false }]);
  apis.wb.claimAll.mockResolvedValue([]);
  const { result } = renderHook(() => useWorkBuddyAccountsViewModel());
  await waitFor(() => expect(result.current.loading).toBe(false));
  await act(async () => {
    await result.current.claimAll();
  });
  expect(apis.wb.claimAll).not.toHaveBeenCalled();
});

it("Trae same-revision readback cannot resolve a lost policy write or permit replay", async () => {
  const row = {
    id: "a",
    realm: "cn",
    status: "stored",
    generation: 1,
    autoCheckinRevision: 2,
    autoCheckinEnabled: false,
    canSetAutomatic: true,
  };
  apis.trae.list.mockResolvedValue([row]);
  apis.trae.setAutomatic.mockRejectedValue(new Error("lost"));
  const { result } = renderHook(() => useTraeAccountsViewModel("cn"));
  await waitFor(() => expect(result.current.loading).toBe(false));
  await act(async () => {
    await result.current.setAutomatic(row as any);
  });
  expect(result.current.policyIsUnknown(result.current.accounts[0])).toBe(true);
  expect(result.current.automaticDisabled(result.current.accounts[0])).toBe(
    true,
  );
  await act(async () => {
    await result.current.setAutomatic(row as any);
  });
  expect(apis.trae.setAutomatic).toHaveBeenCalledTimes(1);
});

it.each([
  { autoCheckinRevision: 2, autoCheckinEnabled: true },
  { autoCheckinRevision: 1, autoCheckinEnabled: true },
  { autoCheckinRevision: undefined, autoCheckinEnabled: true },
  { autoCheckinRevision: 3, autoCheckinEnabled: false },
])("Trae rejects unconfirmed setter receipt %j", async (receipt) => {
  const row = {
    id: "a",
    realm: "cn",
    status: "stored",
    generation: 1,
    autoCheckinRevision: 2,
    autoCheckinEnabled: false,
    canSetAutomatic: true,
  };
  apis.trae.list.mockResolvedValue([row]);
  apis.trae.setAutomatic.mockResolvedValue({ ...row, ...receipt });
  const { result } = renderHook(() => useTraeAccountsViewModel("cn"));
  await waitFor(() => expect(result.current.loading).toBe(false));
  apis.trae.list.mockRejectedValue(new Error("offline"));
  await act(async () => {
    await result.current.setAutomatic(row as any);
  });
  expect(result.current.accounts[0]).toEqual(row);
  expect(result.current.policyIsUnknown(result.current.accounts[0])).toBe(true);
});

it.each([
  { autoClaimPolicyVersion: 2, autoClaimEnabled: true },
  { autoClaimPolicyVersion: 1, autoClaimEnabled: false },
])("WorkBuddy rejects unconfirmed setter receipt %j", async (receipt) => {
  const row = {
    id: "a",
    generation: 1,
    autoClaimRevision: 2,
    autoClaimPolicyVersion: 1,
    autoClaimEnabled: false,
    canSetAutomatic: true,
  };
  apis.wb.list.mockResolvedValue([row]);
  apis.wb.setAutomatic.mockResolvedValue({
    ...row,
    autoClaimRevision: 3,
    ...receipt,
  });
  const { result } = renderHook(() => useWorkBuddyAccountsViewModel());
  await waitFor(() => expect(result.current.loading).toBe(false));
  apis.wb.list.mockRejectedValue(new Error("offline"));
  await act(async () => {
    await result.current.setAutomatic(row as any);
  });
  expect(result.current.accounts[0]).toEqual(row);
  expect(result.current.automaticUncertain.a).toBeDefined();
});

it("WorkBuddy refuses immediate same-turn replay after a lost policy reply", async () => {
  const row = {
    id: "a",
    generation: 1,
    autoClaimRevision: 2,
    autoClaimPolicyVersion: 1,
    autoClaimEnabled: false,
    canSetAutomatic: true,
  };
  apis.wb.list.mockResolvedValue([row]);
  apis.wb.setAutomatic.mockRejectedValue(new Error("lost"));
  const { result } = renderHook(() => useWorkBuddyAccountsViewModel());
  await waitFor(() => expect(result.current.loading).toBe(false));
  await act(async () => {
    const command = result.current.setAutomatic;
    await command(row as any);
    await command(row as any);
  });
  expect(result.current.automaticUncertain.a).toBeDefined();
  expect(apis.wb.setAutomatic).toHaveBeenCalledTimes(1);
});
