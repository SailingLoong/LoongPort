import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { expect, it, vi } from "vitest";
import { WorkBuddyNativePreview } from "@/components/workbuddy/WorkBuddyNativePreview";
import { PreservedView } from "@/components/ui/PreservedView";
let savedCancel: (() => void) | undefined;
let savedConfirm: (() => void) | undefined;
const api = vi.hoisted(() => ({ captureNative: vi.fn() }));
vi.mock("@/components/ui/button", () => ({
  Button: ({ children, onClick, ...props }: any) => {
    if (
      children === "workbuddy.nativeCapture.cancel" ||
      children === "workbuddy.nativeCapture.close"
    )
      savedCancel = onClick;
    if (children === "workbuddy.nativeCapture.confirm") savedConfirm = onClick;
    return (
      <button onClick={onClick} disabled={props.disabled}>
        {children}
      </button>
    );
  },
}));
vi.mock("@/lib/api/workbuddy", () => ({ workbuddyApi: api }));
vi.mock("react-i18next", () => ({
  useTranslation: () => ({ t: (key: string) => key }),
}));
it("does not let an old cancel close a newly opened capture draft", async () => {
  render(
    <QueryClientProvider client={new QueryClient()}>
      <WorkBuddyNativePreview />
    </QueryClientProvider>,
  );
  await userEvent.click(
    screen.getByRole("button", { name: "workbuddy.nativeCapture.open" }),
  );
  const oldCancel = savedCancel!;
  await userEvent.click(
    screen.getByRole("button", { name: "workbuddy.nativeCapture.cancel" }),
  );
  await userEvent.click(
    screen.getByRole("button", { name: "workbuddy.nativeCapture.open" }),
  );
  await userEvent.type(
    screen.getByLabelText("workbuddy.nativeCapture.label"),
    "New draft",
  );
  act(() => oldCancel());
  expect(screen.getByLabelText("workbuddy.nativeCapture.label")).toHaveValue(
    "New draft",
  );
});

it("does not let a prior visible owner's cancel close the restored capture draft", async () => {
  const q = new QueryClient();
  const tree = (active: boolean) => (
    <QueryClientProvider client={q}>
      <PreservedView active={active}>
        <WorkBuddyNativePreview />
      </PreservedView>
    </QueryClientProvider>
  );
  const view = render(tree(true));
  await userEvent.click(
    screen.getByRole("button", { name: "workbuddy.nativeCapture.open" }),
  );
  await userEvent.type(
    screen.getByLabelText("workbuddy.nativeCapture.label"),
    "Retained draft",
  );
  const oldCancel = savedCancel!;
  view.rerender(tree(false));
  view.rerender(tree(true));
  act(() => oldCancel());
  expect(screen.getByLabelText("workbuddy.nativeCapture.label")).toHaveValue(
    "Retained draft",
  );
  api.captureNative.mockClear();
  api.captureNative.mockImplementation(() => new Promise(() => {}));
  await userEvent.click(
    screen.getByRole("button", { name: "workbuddy.nativeCapture.confirm" }),
  );
  await waitFor(() => expect(api.captureNative).toHaveBeenCalledTimes(1));
  await userEvent.click(
    screen.getByRole("button", { name: "workbuddy.nativeCapture.close" }),
  );
  expect(
    screen.queryByLabelText("workbuddy.nativeCapture.label"),
  ).not.toBeInTheDocument();
});

it("cancel retires the retained confirm synchronously within the same batch", async () => {
  api.captureNative.mockClear();
  render(
    <QueryClientProvider client={new QueryClient()}>
      <WorkBuddyNativePreview />
    </QueryClientProvider>,
  );
  await userEvent.click(
    screen.getByRole("button", { name: "workbuddy.nativeCapture.open" }),
  );
  await userEvent.type(
    screen.getByLabelText("workbuddy.nativeCapture.label"),
    "Synthetic",
  );
  const cancel = savedCancel!,
    confirm = savedConfirm!;
  await act(async () => {
    cancel();
    confirm();
  });
  expect(api.captureNative).not.toHaveBeenCalled();
});
