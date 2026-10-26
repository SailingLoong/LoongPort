import { createRef } from "react";
import { act, render, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { beforeEach, expect, it, vi } from "vitest";
import {
  ApplicationWorkspace,
  type ApplicationWorkspaceHandle,
} from "../ApplicationWorkspace";

const state = vi.hoisted(() => ({
  data: {} as any,
  routing: {} as any,
  writeBlocked: false,
  setOrder: vi.fn(),
  select: vi.fn(),
  table: null as any,
}));
vi.mock("../ApplicationTierTable", () => ({
  ApplicationTierTable: (props: unknown) => {
    state.table = props;
    return <table aria-label="native tier table" />;
  },
}));
vi.mock("../useApplicationOverview", () => ({
  useApplicationOverview: () => ({
    data: state.data,
    isPending: false,
    error: null,
    busy: false,
    refetch: vi.fn(),
    select: state.select,
    confirmation: null,
  }),
}));
vi.mock("../useApplicationRouting", () => ({
  useApplicationRouting: () => ({
    data: state.routing,
    isPending: false,
    error: null,
    writeBlocked: state.writeBlocked,
    busy: false,
    refetch: vi.fn(),
    setOrder: state.setOrder,
  }),
}));
vi.mock("@/lib/api/modelVerification", () => ({
  modelVerificationApi: { listSummaries: vi.fn().mockResolvedValue([]) },
}));
vi.mock("react-i18next", () => ({
  useTranslation: () => ({ t: (key: string) => key }),
}));
beforeEach(() => {
  vi.clearAllMocks();
  state.writeBlocked = false;
  state.setOrder.mockResolvedValue(undefined);
  state.data = {
    configurations: ["a", "b", "c"].map((id) => ({
      providerId: id,
      name: id,
      model: null,
      presentation: { isCurrent: id === "a" },
    })),
    isAdditive: true,
  };
  state.routing = {
    autoFailoverEnabled: false,
    tiers: ["a", "b", "c"].map((providerId, position) => ({
      providerId,
      position,
      rateMultiplier: position + 1,
      models: [],
    })),
  };
});
function mount() {
  const ref = createRef<ApplicationWorkspaceHandle>();
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  const view = () => (
    <QueryClientProvider client={client}>
      <ApplicationWorkspace
        ref={ref}
        appId="pi"
        providers={{}}
        onSwitchProvider={vi.fn()}
        onOpenAccount={vi.fn()}
        onAdd={vi.fn()}
        children={null}
      />
    </QueryClientProvider>
  );
  const rendered = render(view());
  return { ref, rerender: () => rendered.rerender(view()) };
}
it("persists non-proxy reorder immediately without creating a route draft", async () => {
  const { ref } = mount();
  expect(state.table.reorderEnabled).toBe(true);
  await act(async () => state.table.onReorder(["c", "a", "b"]));
  expect(state.setOrder).toHaveBeenCalledExactlyOnceWith(["c", "a", "b"]);
  expect(state.table.orderedIds).toEqual(["c", "a", "b"]);
  expect(ref.current?.hasPendingChanges).toBe(false);
  expect(ref.current?.submitting).toBe(false);
  expect(state.select).not.toHaveBeenCalled();
});
it("keeps navigation busy while the immediate order write is pending and rejects another reorder", async () => {
  let finish!: () => void;
  state.setOrder.mockReturnValueOnce(
    new Promise<void>((resolve) => {
      finish = resolve;
    }),
  );
  const { ref } = mount();
  await act(async () => state.table.onReorder(["b", "a", "c"]));
  expect(ref.current?.submitting).toBe(true);
  expect(state.table.orderBusy).toBe(true);
  await act(async () => state.table.onReorder(["c", "b", "a"]));
  expect(state.setOrder).toHaveBeenCalledTimes(1);
  await act(async () => finish());
  expect(ref.current?.submitting).toBe(false);
});
it("restores the last order after rejection and never automatically replays it", async () => {
  state.setOrder.mockRejectedValueOnce(new Error("synthetic rejected order"));
  const { rerender, ref } = mount();
  await act(async () => state.table.onReorder(["b", "a", "c"]));
  expect(state.table.orderedIds).toEqual(["a", "b", "c"]);
  expect(ref.current?.hasPendingChanges).toBe(false);
  state.routing = { ...state.routing };
  rerender();
  expect(state.setOrder).toHaveBeenCalledTimes(1);
  expect(state.select).not.toHaveBeenCalled();
});
it("uses authoritative order on the next routing read instead of keeping an optimistic mirror", async () => {
  const { rerender } = mount();
  await act(async () => state.table.onReorder(["b", "a", "c"]));
  state.routing = {
    ...state.routing,
    tiers: [...state.routing.tiers].reverse(),
  };
  rerender();
  await waitFor(() => expect(state.table.orderedIds).toEqual(["c", "b", "a"]));
  expect(state.setOrder).toHaveBeenCalledTimes(1);
});
it("keeps metric sort local and allows no immediate order write while the owner is blocked", async () => {
  state.writeBlocked = true;
  const { ref } = mount();
  act(() => state.table.onSort("rateMultiplier"));
  expect(state.table.sort).toEqual({
    key: "rateMultiplier",
    descending: false,
  });
  await act(async () => state.table.onReorder(["b", "a", "c"]));
  expect(state.setOrder).not.toHaveBeenCalled();
  expect(state.table.busy).toBe(true);
  expect(state.table.orderBusy).toBe(false);
  expect(ref.current?.hasPendingChanges).toBe(false);
});
