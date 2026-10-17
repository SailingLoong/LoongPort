import {
  act,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import type { AppId } from "@/lib/api";
import type { ServiceSource } from "@/lib/api/serviceSources";
import { BindVendorSource } from "../BindVendorSource";

const api = vi.hoisted(() => ({ list: vi.fn(), bindVendor: vi.fn() }));
vi.mock("@/lib/api/serviceSources", () => ({ serviceSourcesApi: api }));
vi.mock("react-i18next", () => ({
  useTranslation: () => ({ t: (key: string) => key }),
}));
const source: ServiceSource = {
  sourceId: "vendor-plan-a",
  account: { kind: "vendor", id: 21 },
  name: "Plan A",
  serviceName: "Synthetic vendor",
  accountLabel: "Work",
  configurationName: "Plan A",
  bindings: [
    {
      appId: "codex",
      providerId: "vendor-plan-a",
      model: "model-a",
      usageState: "inactive",
      canSelect: true,
    },
  ],
};
const attached = () => ({
  ...source,
  bindings: [
    ...source.bindings,
    { ...source.bindings[0], appId: "claude" as const },
  ],
});
let client: QueryClient;
beforeEach(() => {
  vi.resetAllMocks();
  client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
});
afterEach(() => client.clear());
function show(appId: AppId = "claude", disabled = false) {
  const content = (app: AppId) => (
    <QueryClientProvider client={client}>
      <BindVendorSource appId={app} disabled={disabled} />
    </QueryClientProvider>
  );
  const view = render(content(appId));
  return { ...view, changeApp: (app: AppId) => view.rerender(content(app)) };
}
async function choose() {
  await userEvent.click(
    screen.getByRole("button", { name: "applications.bindSource.open" }),
  );
  await userEvent.click(
    await screen.findByRole("combobox", {
      name: "applications.bindSource.source",
    }),
  );
  await userEvent.click(await screen.findByRole("option", { name: /Plan A/ }));
}
const add = () =>
  screen.getByRole("button", { name: "applications.bindSource.add" });

it("loads on demand and offers only unbound vendor references", async () => {
  api.list.mockResolvedValue([
    source,
    { ...source, sourceId: "relay", account: { kind: "relay", id: 21 } },
    { ...attached(), sourceId: "already", bindings: attached().bindings },
  ]);
  show();
  expect(api.list).not.toHaveBeenCalled();
  await userEvent.click(
    screen.getByRole("button", { name: "applications.bindSource.open" }),
  );
  await userEvent.click(
    await screen.findByRole("combobox", {
      name: "applications.bindSource.source",
    }),
  );
  expect(screen.getAllByRole("option")).toHaveLength(1);
  expect(api.bindVendor).not.toHaveBeenCalled();
});
it("prevents duplicate clicks and confirms a non-active binding by readback", async () => {
  let finish!: (created: boolean) => void;
  api.bindVendor.mockReturnValue(
    new Promise<boolean>((resolve) => {
      finish = resolve;
    }),
  );
  api.list.mockResolvedValueOnce([source]).mockResolvedValueOnce([attached()]);
  const invalidate = vi.spyOn(client, "invalidateQueries");
  show();
  await choose();
  fireEvent.click(add());
  fireEvent.click(add());
  expect(api.bindVendor).toHaveBeenCalledExactlyOnceWith(
    "vendor-plan-a",
    "claude",
  );
  expect(add()).toBeDisabled();
  await act(async () => finish(true));
  expect(
    await screen.findByText("applications.bindSource.confirmed"),
  ).toBeInTheDocument();
  expect(screen.getByText("applications.bindSource.hint")).toBeInTheDocument();
  expect(invalidate).toHaveBeenCalledWith({
    queryKey: ["applicationOverview", "claude"],
  });
  expect(
    invalidate.mock.calls.some(([arg]) => arg?.queryKey?.includes("codex")),
  ).toBe(false);
});
it("keeps a rejected transport with an absent readback unknown until read-only confirmation", async () => {
  api.bindVendor.mockRejectedValue(new Error("reply lost"));
  api.list
    .mockResolvedValueOnce([source])
    .mockResolvedValueOnce([source])
    .mockResolvedValueOnce([attached()]);
  show();
  await choose();
  await userEvent.click(add());
  expect(await screen.findByRole("alert")).toHaveTextContent(
    "applications.bindSource.unknown",
  );
  expect(add()).toBeDisabled();
  await userEvent.click(
    screen.getByRole("button", { name: "applications.bindSource.check" }),
  );
  expect(
    await screen.findByText("applications.bindSource.confirmed"),
  ).toBeInTheDocument();
  expect(api.bindVendor).toHaveBeenCalledTimes(1);
});
it("recognizes a completed binding after a lost reply without resending", async () => {
  api.bindVendor.mockRejectedValue(new Error("reply lost"));
  api.list.mockResolvedValueOnce([source]).mockResolvedValueOnce([attached()]);
  show();
  await choose();
  await userEvent.click(add());
  expect(
    await screen.findByText("applications.bindSource.confirmed"),
  ).toBeInTheDocument();
  expect(api.bindVendor).toHaveBeenCalledTimes(1);
});
it("keeps unknown outcome blocked until a read-only retry confirms it", async () => {
  api.bindVendor.mockResolvedValue(true);
  api.list
    .mockResolvedValueOnce([source])
    .mockRejectedValueOnce(new Error("read failed"))
    .mockResolvedValueOnce([attached()]);
  show();
  await choose();
  await userEvent.click(add());
  expect(await screen.findByRole("alert")).toHaveTextContent(
    "applications.bindSource.unknown",
  );
  expect(add()).toBeDisabled();
  await userEvent.click(
    screen.getByRole("button", { name: "applications.bindSource.check" }),
  );
  expect(
    await screen.findByText("applications.bindSource.confirmed"),
  ).toBeInTheDocument();
  expect(api.bindVendor).toHaveBeenCalledTimes(1);
});
it("does not offer unsupported Apps or write while the workspace is blocked", () => {
  const view = show("gemini");
  expect(screen.queryByRole("button")).not.toBeInTheDocument();
  view.unmount();
  show("claude", true);
  expect(
    screen.getByRole("button", { name: "applications.bindSource.open" }),
  ).toBeDisabled();
  expect(api.list).not.toHaveBeenCalled();
});
it("does not display a previous App's late success in the next App", async () => {
  let finish!: (created: boolean) => void;
  api.bindVendor.mockReturnValue(
    new Promise<boolean>((resolve) => {
      finish = resolve;
    }),
  );
  api.list.mockResolvedValueOnce([source]).mockResolvedValue([attached()]);
  const view = show();
  await choose();
  fireEvent.click(add());
  view.changeApp("codex");
  await act(async () => finish(true));
  await waitFor(() => expect(api.list).toHaveBeenCalledTimes(2));
  expect(
    screen.queryByText("applications.bindSource.confirmed"),
  ).not.toBeInTheDocument();
  expect(api.bindVendor).toHaveBeenCalledExactlyOnceWith(
    "vendor-plan-a",
    "claude",
  );
});

it("keeps a successful RPC with a missing binding unknown without resending", async () => {
  api.bindVendor.mockResolvedValue(true);
  api.list.mockResolvedValue([source]);
  show();
  await choose();
  await userEvent.click(add());
  expect(await screen.findByRole("alert")).toHaveTextContent(
    "applications.bindSource.unknown",
  );
  await userEvent.click(
    screen.getByRole("button", { name: "applications.bindSource.check" }),
  );
  expect(add()).toBeDisabled();
  expect(api.bindVendor).toHaveBeenCalledTimes(1);
});
it("does not confirm a same-named source belonging to a different account", async () => {
  api.bindVendor.mockResolvedValue(true);
  api.list
    .mockResolvedValueOnce([source])
    .mockResolvedValueOnce([
      { ...attached(), account: { kind: "vendor", id: 99 } },
    ]);
  show();
  await choose();
  await userEvent.click(add());
  expect(await screen.findByRole("alert")).toHaveTextContent(
    "applications.bindSource.unknown",
  );
  expect(
    screen.queryByText("applications.bindSource.confirmed"),
  ).not.toBeInTheDocument();
});
it("allows loading another source after a confirmed binding", async () => {
  api.bindVendor.mockResolvedValue(true);
  api.list
    .mockResolvedValueOnce([source])
    .mockResolvedValueOnce([attached()])
    .mockResolvedValueOnce([
      { ...source, sourceId: "second-source", name: "Plan B" },
    ]);
  show();
  await choose();
  await userEvent.click(add());
  expect(
    await screen.findByText("applications.bindSource.confirmed"),
  ).toBeInTheDocument();
  await userEvent.click(
    screen.getByRole("button", { name: "applications.bindSource.open" }),
  );
  await waitFor(() => expect(screen.getByRole("combobox")).not.toBeDisabled());
  await userEvent.click(screen.getByRole("combobox"));
  await userEvent.click(await screen.findByRole("option", { name: /Plan B/ }));
  api.list.mockResolvedValueOnce([
    { ...source, sourceId: "second-source", bindings: attached().bindings },
  ]);
  await userEvent.click(add());
  expect(
    await screen.findByText("applications.bindSource.confirmed"),
  ).toBeInTheDocument();
  expect(api.bindVendor).toHaveBeenLastCalledWith("second-source", "claude");
});

it("retains a pending request across App navigation and component remount", async () => {
  let finish!: (created: boolean) => void;
  api.bindVendor.mockReturnValue(
    new Promise<boolean>((resolve) => {
      finish = resolve;
    }),
  );
  api.list.mockResolvedValueOnce([source]).mockResolvedValueOnce([attached()]);
  const view = show();
  await choose();
  fireEvent.click(add());
  view.changeApp("codex");
  view.changeApp("claude");
  expect(add()).toBeDisabled();
  expect(api.bindVendor).toHaveBeenCalledTimes(1);
  await act(async () => finish(true));
  expect(
    await screen.findByText("applications.bindSource.confirmed"),
  ).toBeInTheDocument();
});
it("retains an unknown outcome across component remount", async () => {
  api.bindVendor.mockResolvedValue(true);
  api.list
    .mockResolvedValueOnce([source])
    .mockRejectedValueOnce(new Error("read failed"))
    .mockResolvedValueOnce([attached()]);
  const view = show();
  await choose();
  await userEvent.click(add());
  expect(await screen.findByRole("alert")).toHaveTextContent(
    "applications.bindSource.unknown",
  );
  view.unmount();
  show();
  expect(add()).toBeDisabled();
  expect(
    screen.getByRole("button", { name: "applications.bindSource.open" }),
  ).toBeDisabled();
  await userEvent.click(
    screen.getByRole("button", { name: "applications.bindSource.check" }),
  );
  expect(
    await screen.findByText("applications.bindSource.confirmed"),
  ).toBeInTheDocument();
  expect(api.bindVendor).toHaveBeenCalledTimes(1);
});
