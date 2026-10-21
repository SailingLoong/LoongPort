import { act, render, screen, fireEvent } from "@testing-library/react";
import { beforeEach, expect, it, vi } from "vitest";
import { PreservedView } from "@/components/ui/PreservedView";
import { AddHubPage } from "@/components/relay/AddHubPage";
const pending = vi.hoisted(() => [] as Array<() => void>);
vi.mock("@/components/providers/AddProviderForm", () => ({
  AddProviderForm: ({ onSaveStart, onDone }: any) => (
    <div>
      <button
        onClick={() => {
          const epoch = onSaveStart();
          pending.push(() => onDone("saved", epoch));
        }}
      >
        Start save
      </button>
      <button onClick={() => onDone("cancel")}>Cancel manual</button>
      <button onClick={() => pending.shift()?.()}>Finish save</button>
    </div>
  ),
}));
vi.mock("@/components/relay/directory/RelayDirectoryPage", () => ({
  RelayDirectoryPage: () => null,
}));
vi.mock("@/components/relay/OfficialApiPage", () => ({
  OfficialApiPage: () => null,
}));
beforeEach(() => {
  pending.length = 0;
});
function setup() {
  const back = vi.fn();
  render(
    <AddHubPage
      sourceAppId="codex"
      initialTab="manual"
      onBack={back}
      onAddProvider={vi.fn()}
    />,
  );
  return back;
}
it("consumes one destination only once when two same-destination saves finish", () => {
  const back = setup();
  fireEvent.click(screen.getByText("Start save"));
  fireEvent.click(screen.getByText("Start save"));
  fireEvent.click(screen.getByText("Finish save"));
  fireEvent.click(screen.getByText("Finish save"));
  expect(back).toHaveBeenCalledTimes(1);
});
it.each(["Cancel manual", "common.back"])(
  "invalidates an outstanding save after %s",
  (button) => {
    const back = setup();
    fireEvent.click(screen.getByText("Start save"));
    fireEvent.click(screen.getByRole("button", { name: button }));
    fireEvent.click(screen.getByText("Finish save"));
    expect(back).toHaveBeenCalledTimes(1);
  },
);

it("does not navigate after the whole AddHub is hidden by global navigation", () => {
  const back = vi.fn();
  const view = (active: boolean) => (
    <PreservedView active={active}>
      <AddHubPage
        sourceAppId="codex"
        initialTab="manual"
        onBack={back}
        onAddProvider={vi.fn()}
      />
    </PreservedView>
  );
  const { rerender } = render(view(true));
  fireEvent.click(screen.getByText("Start save"));
  rerender(view(false));
  act(() => pending.shift()?.());
  expect(back).not.toHaveBeenCalled();
});
it("invalidates an old save across hide and restore of the same destination", () => {
  const back = vi.fn();
  const view = (active: boolean) => (
    <PreservedView active={active}>
      <AddHubPage
        sourceAppId="codex"
        initialTab="manual"
        onBack={back}
        onAddProvider={vi.fn()}
      />
    </PreservedView>
  );
  const { rerender } = render(view(true));
  fireEvent.click(screen.getByText("Start save"));
  rerender(view(false));
  rerender(view(true));
  act(() => pending.shift()?.());
  expect(back).not.toHaveBeenCalled();
});
