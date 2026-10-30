import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import {
  act,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, expect, it, vi } from "vitest";
import { WorkBuddyNativePreview } from "@/components/workbuddy/WorkBuddyNativePreview";
import {
  useWorkBuddyNativeCaptureViewModel,
  useWorkBuddyNativePreviewViewModel,
} from "@/components/workbuddy/useWorkBuddyNativePreviewViewModel";
import { PreservedView } from "@/components/ui/PreservedView";
const api = vi.hoisted(() => ({
  previewNative: vi.fn(),
  captureNative: vi.fn(),
}));
vi.mock("@/lib/api/workbuddy", () => ({ workbuddyApi: api }));
vi.mock("react-i18next", () => ({
  useTranslation: () => ({ t: (key: string) => key }),
}));
const open = () =>
  screen.getByRole("button", { name: "workbuddy.nativeCapture.open" });
const label = () => screen.getByLabelText("workbuddy.nativeCapture.label");
const confirm = () =>
  screen.getByRole("button", { name: "workbuddy.nativeCapture.confirm" });
const client = () =>
  new QueryClient({
    defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
  });
const tree = (q: QueryClient) => (
  <QueryClientProvider client={q}>
    <WorkBuddyNativePreview />
  </QueryClientProvider>
);
beforeEach(() => vi.resetAllMocks());
it("does not read or save on mount, opening, or cancel", async () => {
  render(tree(client()));
  expect(api.previewNative).not.toHaveBeenCalled();
  expect(api.captureNative).not.toHaveBeenCalled();
  await userEvent.click(open());
  await userEvent.type(label(), "Synthetic");
  await userEvent.click(
    screen.getByRole("button", { name: "workbuddy.nativeCapture.cancel" }),
  );
  expect(api.captureNative).not.toHaveBeenCalled();
});
it("requires explicit confirmation and admits only one capture", async () => {
  let resolve!: (value: unknown) => void;
  api.captureNative.mockImplementation(
    () =>
      new Promise((r) => {
        resolve = r;
      }),
  );
  render(tree(client()));
  await userEvent.click(open());
  await userEvent.type(label(), "Synthetic");
  const button = confirm();
  fireEvent.click(button);
  fireEvent.click(button);
  await waitFor(() => expect(api.captureNative).toHaveBeenCalledTimes(1));
  expect(api.captureNative).toHaveBeenCalledWith({
    label: "Synthetic",
    expected: null,
  });
  await act(async () =>
    resolve({
      state: "saved",
      account: {
        label: "Synthetic",
        realm: "www.workbuddy.cn",
        selection: {
          id: "synthetic",
          generation: 1,
          incarnation: "0e89ed8a-02b4-4164-a081-f7bc419761e2",
        },
      },
      activationAvailable: false,
    }),
  );
  expect(
    await screen.findByText("workbuddy.nativeCapture.saved"),
  ).toBeVisible();
  expect(confirm()).toBeDisabled();
});
it("does not replay an uncertain or in-flight capture after remount", async () => {
  let reject!: (reason: unknown) => void;
  api.captureNative.mockImplementation(
    () =>
      new Promise((_, r) => {
        reject = r;
      }),
  );
  const q = client(),
    first = render(tree(q));
  await userEvent.click(open());
  await userEvent.type(label(), "Synthetic");
  fireEvent.click(confirm());
  await waitFor(() => expect(api.captureNative).toHaveBeenCalledTimes(1));
  first.unmount();
  render(tree(q));
  await userEvent.click(open());
  expect(confirm()).toBeDisabled();
  await act(async () => reject(new Error("unknown secret")));
  expect(
    await screen.findByText("workbuddy.nativeCapture.unknown"),
  ).toBeVisible();
  expect(confirm()).toBeDisabled();
  expect(api.captureNative).toHaveBeenCalledTimes(1);
  expect(document.body).not.toHaveTextContent("unknown secret");
});

it("does not revive an old confirmation callback after Activity hide and reveal", async () => {
  const q = client();
  let model!: ReturnType<typeof useWorkBuddyNativeCaptureViewModel>;
  function Harness() {
    model = useWorkBuddyNativeCaptureViewModel();
    return null;
  }
  const view = (active: boolean) => (
    <QueryClientProvider client={q}>
      <PreservedView active={active}>
        <Harness />
      </PreservedView>
    </QueryClientProvider>
  );
  const rendered = render(view(true));
  act(() => model.setLabel("Synthetic"));
  const oldConfirm = model.confirm;
  rendered.rerender(view(false));
  rendered.rerender(view(true));
  await act(async () => oldConfirm());
  expect(api.captureNative).not.toHaveBeenCalled();
});
it("admits at most one dispatch when two confirmations race on the same QueryClient", async () => {
  const q = client();
  let a!: ReturnType<typeof useWorkBuddyNativeCaptureViewModel>, b!: typeof a;
  function A() {
    a = useWorkBuddyNativeCaptureViewModel();
    return null;
  }
  function B() {
    b = useWorkBuddyNativeCaptureViewModel();
    return null;
  }
  api.captureNative.mockImplementation(() => new Promise(() => {}));
  render(
    <QueryClientProvider client={q}>
      <A />
      <B />
    </QueryClientProvider>,
  );
  act(() => {
    a.setLabel("First");
    b.setLabel("Second");
  });
  act(() => {
    a.confirm();
    b.confirm();
  });
  await waitFor(() => expect(api.captureNative).toHaveBeenCalledTimes(1));
});
it("refreshes shared inventories and the manager only after a confirmed saved receipt", async () => {
  const q = client(),
    invalidate = vi.spyOn(q, "invalidateQueries"),
    onCaptured = vi.fn();
  api.captureNative.mockResolvedValue({
    state: "saved",
    account: {
      label: "Synthetic",
      realm: "www.workbuddy.cn",
      selection: {
        id: "synthetic",
        generation: 1,
        incarnation: "0e89ed8a-02b4-4164-a081-f7bc419761e2",
      },
    },
    activationAvailable: false,
  });
  render(
    <QueryClientProvider client={q}>
      <WorkBuddyNativePreview onCaptured={onCaptured} />
    </QueryClientProvider>,
  );
  await userEvent.click(open());
  await userEvent.type(label(), "Synthetic");
  await userEvent.click(confirm());
  await waitFor(() => expect(onCaptured).toHaveBeenCalledTimes(1));
  expect(invalidate).toHaveBeenCalledWith({
    queryKey: ["accountPlatformPresence"],
  });
  expect(invalidate).toHaveBeenCalledWith({
    queryKey: ["workbuddyNativeCaptures"],
  });
});
it("does not notify a restored owner when an earlier visible session finishes", async () => {
  const q = client(),
    onCaptured = vi.fn();
  let resolve!: (value: unknown) => void;
  api.captureNative.mockImplementation(
    () =>
      new Promise((r) => {
        resolve = r;
      }),
  );
  const view = (active: boolean) => (
    <QueryClientProvider client={q}>
      <PreservedView active={active}>
        <WorkBuddyNativePreview onCaptured={onCaptured} />
      </PreservedView>
    </QueryClientProvider>
  );
  const rendered = render(view(true));
  await userEvent.click(open());
  await userEvent.type(label(), "Synthetic");
  await userEvent.click(confirm());
  await waitFor(() => expect(api.captureNative).toHaveBeenCalledTimes(1));
  rendered.rerender(view(false));
  rendered.rerender(view(true));
  await act(async () =>
    resolve({
      state: "saved",
      account: {
        label: "Synthetic",
        realm: "www.workbuddy.cn",
        selection: {
          id: "synthetic",
          generation: 1,
          incarnation: "0e89ed8a-02b4-4164-a081-f7bc419761e2",
        },
      },
      activationAvailable: false,
    }),
  );
  expect(onCaptured).not.toHaveBeenCalled();
});
it.each(["requestConflict", "unsupportedContext"])(
  "does not treat %s as saved or automatically retry",
  async (failure) => {
    const q = client(),
      invalidate = vi.spyOn(q, "invalidateQueries"),
      onCaptured = vi.fn();
    api.captureNative.mockRejectedValue(new Error(failure));
    render(
      <QueryClientProvider client={q}>
        <WorkBuddyNativePreview onCaptured={onCaptured} />
      </QueryClientProvider>,
    );
    await userEvent.click(open());
    await userEvent.type(label(), "Synthetic");
    await userEvent.click(confirm());
    expect(
      await screen.findByText("workbuddy.nativeCapture.unknown"),
    ).toBeVisible();
    expect(
      screen.queryByText("workbuddy.nativeCapture.saved"),
    ).not.toBeInTheDocument();
    expect(onCaptured).not.toHaveBeenCalled();
    expect(invalidate).not.toHaveBeenCalled();
    expect(api.captureNative).toHaveBeenCalledTimes(1);
    await userEvent.click(
      screen.getByRole("button", { name: "workbuddy.nativeCapture.close" }),
    );
    await userEvent.click(open());
    expect(confirm()).toBeDisabled();
    expect(api.captureNative).toHaveBeenCalledTimes(1);
  },
);
it("does not revive an old inspection callback after Activity hide and reveal", async () => {
  let model!: ReturnType<typeof useWorkBuddyNativePreviewViewModel>;
  function Harness() {
    model = useWorkBuddyNativePreviewViewModel();
    return null;
  }
  const view = (active: boolean) => (
    <PreservedView active={active}>
      <Harness />
    </PreservedView>
  );
  const rendered = render(view(true));
  const retained = model.inspect;
  rendered.rerender(view(false));
  rendered.rerender(view(true));
  await act(async () => {
    await retained();
  });
  expect(api.previewNative).not.toHaveBeenCalled();
});
it("clears stale inspection busy state after the old read settles without showing its result", async () => {
  let resolve!: (value: unknown) => void;
  api.previewNative.mockImplementation(
    () =>
      new Promise((r) => {
        resolve = r;
      }),
  );
  let model!: ReturnType<typeof useWorkBuddyNativePreviewViewModel>;
  function Harness() {
    model = useWorkBuddyNativePreviewViewModel();
    return null;
  }
  const tree = (active: boolean) => (
    <PreservedView active={active}>
      <Harness />
    </PreservedView>
  );
  const view = render(tree(true));
  let pending!: Promise<void>;
  act(() => {
    pending = model.inspect();
  });
  view.rerender(tree(false));
  view.rerender(tree(true));
  expect(model.busy).toBe(true);
  await act(async () => {
    resolve({
      sourceVerification: "knownLocationOnly",
      activationAvailable: false,
      logoutMarkerPresent: false,
      authFile: "missing",
      realm: null,
    });
    await pending;
  });
  expect(model.busy).toBe(false);
  expect(model.result).toBeNull();
});
it("never treats a preview result as authorization to capture", async () => {
  api.previewNative.mockResolvedValue({
    sourceVerification: "knownLocationOnly",
    activationAvailable: false,
    logoutMarkerPresent: false,
    authFile: "recognizedPersonalFormat",
    realm: "www.workbuddy.cn",
  });
  render(tree(client()));
  await userEvent.click(
    screen.getByRole("button", { name: "workbuddy.nativePreview.inspect" }),
  );
  expect(
    await screen.findByText("workbuddy.nativePreview.recognized"),
  ).toBeVisible();
  expect(api.captureNative).not.toHaveBeenCalled();
});
it("does not revive a confirmation after the label changes away and back", async () => {
  const q = client();
  let model!: ReturnType<typeof useWorkBuddyNativeCaptureViewModel>;
  function Harness() {
    model = useWorkBuddyNativeCaptureViewModel();
    return null;
  }
  render(
    <QueryClientProvider client={q}>
      <Harness />
    </QueryClientProvider>,
  );
  act(() => model.setLabel("First"));
  const retained = model.confirm;
  act(() => model.setLabel("Second"));
  act(() => model.setLabel("First"));
  await act(async () => retained());
  expect(api.captureNative).not.toHaveBeenCalled();
});
it("does not let a retired label callback alter the restored draft", () => {
  const q = client();
  let model!: ReturnType<typeof useWorkBuddyNativeCaptureViewModel>;
  function Harness() {
    model = useWorkBuddyNativeCaptureViewModel();
    return null;
  }
  const tree = (active: boolean) => (
    <QueryClientProvider client={q}>
      <PreservedView active={active}>
        <Harness />
      </PreservedView>
    </QueryClientProvider>
  );
  const view = render(tree(true));
  const retained = model.setLabel;
  view.rerender(tree(false));
  view.rerender(tree(true));
  act(() => model.setLabel("New"));
  act(() => retained("Old"));
  expect(model.label).toBe("New");
});
it("shows a fixed conflict warning without claiming an existing account was replaced", async () => {
  const error = Object.assign(new Error("private-token"), {
    code: "requestConflict",
  });
  api.captureNative.mockRejectedValue(error);
  render(tree(client()));
  await userEvent.click(open());
  await userEvent.type(label(), "Synthetic");
  await userEvent.click(confirm());
  expect(
    await screen.findByText("workbuddy.nativeCapture.conflict"),
  ).toBeVisible();
  expect(screen.getByText("workbuddy.nativeCapture.unknown")).toBeVisible();
  expect(
    screen.queryByText("workbuddy.nativeCapture.saved"),
  ).not.toBeInTheDocument();
  expect(document.body).not.toHaveTextContent("private-token");
});
it("does not show an old preview observation after hiding and restoring the owner", async () => {
  api.previewNative.mockResolvedValue({
    sourceVerification: "knownLocationOnly",
    activationAvailable: false,
    logoutMarkerPresent: false,
    authFile: "recognizedPersonalFormat",
    realm: "www.workbuddy.cn",
  });
  const q = client();
  const tree = (active: boolean) => (
    <QueryClientProvider client={q}>
      <PreservedView active={active}>
        <WorkBuddyNativePreview />
      </PreservedView>
    </QueryClientProvider>
  );
  const view = render(tree(true));
  await userEvent.click(
    screen.getByRole("button", { name: "workbuddy.nativePreview.inspect" }),
  );
  expect(
    await screen.findByText("workbuddy.nativePreview.recognized"),
  ).toBeVisible();
  view.rerender(tree(false));
  view.rerender(tree(true));
  expect(
    screen.queryByText("workbuddy.nativePreview.recognized"),
  ).not.toBeInTheDocument();
  expect(api.captureNative).not.toHaveBeenCalled();
});

it("freezes the submitted label while capture is unresolved", async () => {
  api.captureNative.mockImplementation(() => new Promise(() => {}));
  render(tree(client()));
  await userEvent.click(open());
  await userEvent.type(label(), "First");
  await userEvent.click(confirm());
  await waitFor(() => expect(api.captureNative).toHaveBeenCalledTimes(1));
  expect(label()).toBeDisabled();
  expect(label()).toHaveValue("First");
  expect(
    screen.getByRole("button", { name: "workbuddy.nativeCapture.close" }),
  ).toBeVisible();
});
