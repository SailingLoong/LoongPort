import { render } from "@testing-library/react";
import { beforeEach, expect, it, vi } from "vitest";
import type { ReactNode } from "react";
import { AccountOrderSurface } from "../AccountOrderSurface";

// Component boundary only. This deliberately does not claim a real pointer drag.
const boundary = vi.hoisted(() => ({
  onDragEnd: undefined as
    | undefined
    | ((event: {
        active: { id: string };
        over: { id: string } | null;
      }) => void),
}));
vi.mock("@dnd-kit/core", async (load) => ({
  ...(await load<typeof import("@dnd-kit/core")>()),
  DndContext: ({
    children,
    onDragEnd,
  }: {
    children: ReactNode;
    onDragEnd: typeof boundary.onDragEnd;
  }) => {
    boundary.onDragEnd = onDragEnd;
    return children;
  },
}));
beforeEach(() => {
  boundary.onDragEnd = undefined;
});
const items = [
  { id: "workbuddy", label: "WorkBuddy" },
  { id: "minimax_code", label: "MiniMax" },
] as const;
it("forwards a completed drag's exact known App pair to the shared order intent", () => {
  const onReorder = vi.fn();
  render(
    <AccountOrderSurface items={items} onReorder={onReorder}>
      <span>Groups</span>
    </AccountOrderSurface>,
  );
  boundary.onDragEnd!({
    active: { id: "workbuddy" },
    over: { id: "minimax_code" },
  });
  expect(onReorder.mock.calls).toEqual([["workbuddy", "minimax_code"]]);
});
it.each([
  { active: { id: "workbuddy" }, over: null },
  { active: { id: "workbuddy" }, over: { id: "workbuddy" } },
  { active: { id: "unrecognized" }, over: { id: "minimax_code" } },
  { active: { id: "workbuddy" }, over: { id: "unrecognized" } },
])("ignores cancelled, unchanged or unrecognized drag targets: %j", (event) => {
  const onReorder = vi.fn();
  render(
    <AccountOrderSurface items={items} onReorder={onReorder}>
      <span>Groups</span>
    </AccountOrderSurface>,
  );
  boundary.onDragEnd!(event);
  expect(onReorder).not.toHaveBeenCalled();
});
it("cannot reorder while account navigation is blocked", () => {
  const onReorder = vi.fn();
  render(
    <AccountOrderSurface items={items} onReorder={onReorder} disabled>
      <span>Groups</span>
    </AccountOrderSurface>,
  );
  boundary.onDragEnd!({
    active: { id: "workbuddy" },
    over: { id: "minimax_code" },
  });
  expect(onReorder).not.toHaveBeenCalled();
});
