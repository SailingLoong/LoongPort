import { StrictMode } from "react";
import {
  act,
  fireEvent,
  render,
  screen,
  waitFor,
  within,
} from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, expect, it, vi } from "vitest";
import { TraeAccounts } from "@/components/trae/TraeAccounts";
import {
  TRAE_AUTH_HOST,
  type TraeAccountView,
  type TraeFlowView,
} from "@/lib/api/traeAccounts";

const mocks = vi.hoisted(() => ({
  invoke: vi.fn(),
  open: vi.fn(),
  list: vi.fn(),
  discover: vi.fn(),
  begin: vi.fn(),
  confirm: vi.fn(),
  cancel: vi.fn(),
  retire: vi.fn(),
}));
vi.mock("@tauri-apps/api/core", () => ({ invoke: mocks.invoke }));
vi.mock("@tauri-apps/plugin-dialog", () => ({ open: mocks.open }));
vi.mock("react-i18next", async (importOriginal) => ({
  ...(await importOriginal<typeof import("react-i18next")>()),
  useTranslation: () => ({ t: (key: string) => key }),
}));

const storagePath = "/synthetic/official-trae/storage.json";
const productPath = "/synthetic/official-trae/product.json";
const flow: TraeFlowView = {
  flowId: "synthetic-flow-1",
  realm: "sg",
  authHost: TRAE_AUTH_HOST,
};
const account: TraeAccountView = {
  id: "synthetic-account-1",
  realm: "sg",
  status: "stored",
};
function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (error: unknown) => void;
  const promise = new Promise<T>((resolvePromise, rejectPromise) => {
    resolve = resolvePromise;
    reject = rejectPromise;
  });
  return { promise, resolve, reject };
}
async function openDialog(realm: "SG" | "US" = "SG") {
  await userEvent.click(screen.getByRole("button", { name: "trae.add" }));
  await userEvent.click(screen.getByRole("radio", { name: realm }));
}
async function selectFiles() {
  await openDialog();
  await userEvent.click(
    screen.getByRole("button", { name: "trae.selectFiles" }),
  );
  await screen.findByRole("checkbox", { name: "trae.consent" });
}
async function authorize() {
  await selectFiles();
  await userEvent.click(screen.getByRole("checkbox", { name: "trae.consent" }));
  await userEvent.click(screen.getByRole("button", { name: "trae.confirm" }));
}
beforeEach(() => {
  Object.values(mocks).forEach((mock) => mock.mockReset());
  mocks.open.mockImplementation(async ({ title }: { title: string }) =>
    title === "trae.selectStorage" ? storagePath : productPath,
  );
  mocks.list.mockResolvedValue([]);
  mocks.discover.mockResolvedValue([]);
  mocks.begin.mockResolvedValue(flow);
  mocks.confirm.mockResolvedValue(account);
  mocks.cancel.mockResolvedValue(undefined);
  mocks.retire.mockResolvedValue(undefined);
  mocks.invoke.mockImplementation(
    (command: string, args?: Record<string, unknown>) => {
      switch (command) {
        case "discover_trae_installations":
          return mocks.discover(args?.realm);
        case "list_trae_accounts":
          return mocks.list();
        case "begin_trae_authorization":
          return mocks.begin(args);
        case "confirm_trae_authorization":
          return mocks.confirm(args?.flowId);
        case "cancel_trae_authorization":
          return mocks.cancel(args?.flowId);
        case "retire_trae_account":
          return mocks.retire(args?.id);
        default:
          throw new Error("Unexpected synthetic command");
      }
    },
  );
});

it("only lists safe local state automatically, with text-only/CN and entitlement caveats", async () => {
  mocks.list.mockResolvedValue([account]);
  render(<TraeAccounts />);
  expect(await screen.findByText(account.id)).toBeVisible();
  expect(screen.getByText("trae.scope")).toBeVisible();
  expect(screen.getByText("trae.localState")).toBeVisible();
  expect(screen.getByText("trae.status.stored")).toBeVisible();
  expect(mocks.invoke.mock.calls).toEqual([["list_trae_accounts"]]);
  expect(mocks.open).not.toHaveBeenCalled();
});

it("requires separate explicit read consent after selection and guards duplicate confirmation", async () => {
  const pending = deferred<TraeAccountView>();
  mocks.confirm.mockReturnValue(pending.promise);
  const view = render(<TraeAccounts />);
  await userEvent.click(screen.getByRole("button", { name: "trae.add" }));
  expect(mocks.open).not.toHaveBeenCalled();
  expect(
    screen.getByRole("button", { name: "trae.selectFiles" }),
  ).toBeDisabled();
  await userEvent.click(screen.getByRole("radio", { name: "SG" }));
  await userEvent.click(
    screen.getByRole("button", { name: "trae.selectFiles" }),
  );
  const checkbox = await screen.findByRole("checkbox", {
    name: "trae.consent",
  });
  expect(checkbox).not.toBeChecked();
  expect(mocks.begin).toHaveBeenCalledExactlyOnceWith({
    storagePath,
    productPath,
    realm: "sg",
  });
  expect(mocks.confirm).not.toHaveBeenCalled();
  const button = screen.getByRole("button", { name: "trae.confirm" });
  expect(button).toBeDisabled();
  await userEvent.click(button);
  expect(mocks.confirm).not.toHaveBeenCalled();
  await userEvent.click(checkbox);
  act(() => {
    fireEvent.click(button);
    fireEvent.click(button);
  });
  expect(mocks.confirm).toHaveBeenCalledExactlyOnceWith(flow.flowId);
  expect(mocks.invoke).toHaveBeenCalledWith("confirm_trae_authorization", {
    flowId: flow.flowId,
  });
  await act(async () => pending.resolve(account));
  expect(await screen.findByText("trae.saved")).toBeVisible();
  expect(screen.getByText(account.id)).toBeVisible();
  view.unmount();
  expect(mocks.cancel).not.toHaveBeenCalled();
});

it("passes the explicitly selected US realm and displays only the pinned host", async () => {
  mocks.begin.mockResolvedValue({ ...flow, realm: "us" });
  render(<TraeAccounts />);
  await openDialog("US");
  await userEvent.click(
    screen.getByRole("button", { name: "trae.selectFiles" }),
  );
  await screen.findByRole("checkbox");
  expect(mocks.begin).toHaveBeenCalledWith({
    storagePath,
    productPath,
    realm: "us",
  });
  expect(screen.getByText(new RegExp(TRAE_AUTH_HOST))).toBeVisible();
  expect(screen.queryByRole("link")).not.toBeInTheDocument();
  expect(
    mocks.open.mock.calls.every(
      ([options]) => !Object.hasOwn(options, "defaultPath"),
    ),
  ).toBe(true);
  expect(mocks.confirm).not.toHaveBeenCalled();
});

it.each(["storage", "product"])(
  "cancelled %s selection never begins authorization",
  async (file) => {
    if (file === "product") mocks.open.mockResolvedValueOnce(storagePath);
    mocks.open.mockResolvedValueOnce(null);
    render(<TraeAccounts />);
    await openDialog();
    await userEvent.click(
      screen.getByRole("button", { name: "trae.selectFiles" }),
    );
    await waitFor(() =>
      expect(
        screen.getByRole("button", { name: "trae.selectFiles" }),
      ).toBeEnabled(),
    );
    expect(mocks.open).toHaveBeenCalledTimes(file === "storage" ? 1 : 2);
    expect(mocks.begin).not.toHaveBeenCalled();
    expect(mocks.confirm).not.toHaveBeenCalled();
  },
);

it("rejects a different filename without attempting a credential read", async () => {
  mocks.open.mockResolvedValueOnce("/synthetic/other.json");
  render(<TraeAccounts />);
  await openDialog();
  await userEvent.click(
    screen.getByRole("button", { name: "trae.selectFiles" }),
  );
  expect(await screen.findByRole("alert")).toHaveTextContent("trae.failed");
  expect(mocks.begin).not.toHaveBeenCalled();
  expect(mocks.confirm).not.toHaveBeenCalled();
});

it("synchronously guards duplicate file selection", async () => {
  const pending = deferred<string | null>();
  mocks.open.mockReturnValueOnce(pending.promise);
  render(<TraeAccounts />);
  await openDialog();
  const button = screen.getByRole("button", { name: "trae.selectFiles" });
  act(() => {
    fireEvent.click(button);
    fireEvent.click(button);
  });
  expect(mocks.open).toHaveBeenCalledTimes(1);
  await act(async () => pending.resolve(storagePath));
  await screen.findByRole("checkbox");
  expect(mocks.open).toHaveBeenCalledTimes(2);
  expect(mocks.begin).toHaveBeenCalledTimes(1);
});

it.each(["cancel", "close", "escape", "unmount"])(
  "%s retires a flow returned by a late begin",
  async (action) => {
    const pending = deferred<TraeFlowView>();
    mocks.begin.mockReturnValue(pending.promise);
    const view = render(<TraeAccounts />);
    await openDialog();
    await userEvent.click(
      screen.getByRole("button", { name: "trae.selectFiles" }),
    );
    await waitFor(() => expect(mocks.begin).toHaveBeenCalledTimes(1));
    if (action === "unmount") view.unmount();
    else if (action === "escape") await userEvent.keyboard("{Escape}");
    else
      await userEvent.click(
        screen.getByRole("button", {
          name: action === "close" ? "trae.close" : "trae.cancel",
        }),
      );
    await act(async () => pending.resolve(flow));
    await waitFor(() =>
      expect(mocks.cancel).toHaveBeenCalledExactlyOnceWith(flow.flowId),
    );
    expect(mocks.confirm).not.toHaveBeenCalled();
    expect(screen.queryByText("trae.saved")).not.toBeInTheDocument();
  },
);

it("unmounting an unconfirmed selection cancels it without reading credentials", async () => {
  const view = render(<TraeAccounts />);
  await selectFiles();
  view.unmount();
  expect(mocks.cancel).toHaveBeenCalledExactlyOnceWith(flow.flowId);
  expect(mocks.confirm).not.toHaveBeenCalled();
});

it("closing an empty dialog preserves the initial in-flight list", async () => {
  const pending = deferred<TraeAccountView[]>();
  mocks.list.mockReturnValueOnce(pending.promise);
  render(<TraeAccounts />);
  await openDialog();
  await userEvent.click(screen.getByRole("button", { name: "trae.close" }));
  expect(screen.getByText("trae.loading")).toBeVisible();
  await act(async () => pending.resolve([account]));
  expect(screen.getByText(account.id)).toBeVisible();
  expect(mocks.cancel).not.toHaveBeenCalled();
});

it("closing during the native picker never opens the next picker or begins a flow", async () => {
  const pending = deferred<string | null>();
  mocks.open.mockReturnValueOnce(pending.promise);
  render(<TraeAccounts />);
  await openDialog();
  await userEvent.click(
    screen.getByRole("button", { name: "trae.selectFiles" }),
  );
  await userEvent.click(screen.getByRole("button", { name: "trae.close" }));
  await act(async () => pending.resolve(storagePath));
  expect(mocks.open).toHaveBeenCalledTimes(1);
  expect(mocks.begin).not.toHaveBeenCalled();
});

it.each(["close", "unmount"])(
  "%s cancels a selected flow and suppresses late confirmation",
  async (action) => {
    const pending = deferred<TraeAccountView>();
    mocks.confirm.mockReturnValue(pending.promise);
    const view = render(<TraeAccounts />);
    await authorize();
    if (action === "unmount") view.unmount();
    else
      await userEvent.click(screen.getByRole("button", { name: "trae.close" }));
    expect(mocks.cancel).toHaveBeenCalledExactlyOnceWith(flow.flowId);
    await act(async () => pending.resolve(account));
    expect(screen.queryByText("trae.saved")).not.toBeInTheDocument();
    expect(screen.queryByText(account.id)).not.toBeInTheDocument();
  },
);

it.each([
  { ...flow, authHost: "https://synthetic-untrusted.invalid" },
  { ...flow, realm: "us" },
])(
  "rejects a returned host or realm mismatch and cancels that exact flow",
  async (mismatched) => {
    mocks.begin.mockResolvedValue(mismatched);
    render(<TraeAccounts />);
    await openDialog();
    await userEvent.click(
      screen.getByRole("button", { name: "trae.selectFiles" }),
    );
    expect(await screen.findByRole("alert")).toHaveTextContent("trae.failed");
    expect(mocks.cancel).toHaveBeenCalledExactlyOnceWith(flow.flowId);
    expect(screen.queryByRole("checkbox")).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: "trae.confirm" })).toBeDisabled();
    expect(screen.queryByText(/synthetic-untrusted/)).not.toBeInTheDocument();
    expect(mocks.confirm).not.toHaveBeenCalled();
  },
);

it("never lets an older begin overwrite a newer flow", async () => {
  const oldBegin = deferred<TraeFlowView>();
  const newBegin = deferred<TraeFlowView>();
  const newest = { ...flow, flowId: "synthetic-flow-2" };
  mocks.begin
    .mockReturnValueOnce(oldBegin.promise)
    .mockReturnValueOnce(newBegin.promise);
  render(<TraeAccounts />);
  await openDialog();
  await userEvent.click(
    screen.getByRole("button", { name: "trae.selectFiles" }),
  );
  await userEvent.click(screen.getByRole("button", { name: "trae.cancel" }));
  await openDialog();
  await userEvent.click(
    screen.getByRole("button", { name: "trae.selectFiles" }),
  );
  await act(async () => oldBegin.resolve(flow));
  expect(mocks.cancel).toHaveBeenCalledWith(flow.flowId);
  expect(
    screen.getByRole("button", { name: "trae.selectFiles" }),
  ).toBeDisabled();
  expect(screen.queryByRole("checkbox")).not.toBeInTheDocument();
  await act(async () => newBegin.resolve(newest));
  await userEvent.click(await screen.findByRole("checkbox"));
  await userEvent.click(screen.getByRole("button", { name: "trae.confirm" }));
  expect(mocks.confirm).toHaveBeenCalledExactlyOnceWith(newest.flowId);
});

it.each(["success", "failure"])(
  "suppresses an old confirm %s while a newer confirmation is pending",
  async (outcome) => {
    const oldConfirm = deferred<TraeAccountView>();
    const newConfirm = deferred<TraeAccountView>();
    const newest = { ...flow, flowId: "synthetic-flow-2" };
    const newAccount = { ...account, id: "synthetic-account-2" };
    mocks.begin.mockResolvedValueOnce(flow).mockResolvedValueOnce(newest);
    mocks.confirm
      .mockReturnValueOnce(oldConfirm.promise)
      .mockReturnValueOnce(newConfirm.promise);
    render(<TraeAccounts />);
    await authorize();
    await userEvent.click(screen.getByRole("button", { name: "trae.cancel" }));
    await authorize();
    await act(async () => {
      if (outcome === "success") oldConfirm.resolve(account);
      else oldConfirm.reject(new Error("SYNTHETIC_OLD_SECRET"));
    });
    expect(
      screen.getByRole("button", { name: "trae.confirming" }),
    ).toBeDisabled();
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
    expect(screen.queryByText(account.id)).not.toBeInTheDocument();
    await act(async () => newConfirm.resolve(newAccount));
    expect(await screen.findByText(newAccount.id)).toBeVisible();
    expect(screen.queryByText(account.id)).not.toBeInTheDocument();
  },
);

it("shows a fixed uncertainty warning after failed cancellation, without claiming cancellation succeeded", async () => {
  mocks.cancel.mockRejectedValue(new Error("SYNTHETIC_SECRET_PATH"));
  render(<TraeAccounts />);
  await selectFiles();
  await userEvent.click(screen.getByRole("button", { name: "trae.cancel" }));
  expect(await screen.findByRole("alert")).toHaveTextContent(
    "trae.cancelUncertain",
  );
  expect(
    screen.queryByText(/SYNTHETIC_SECRET_PATH|trae.cancelled|trae.saved/),
  ).not.toBeInTheDocument();
  expect(screen.getByRole("button", { name: "trae.reload" })).toBeEnabled();
});

it.each(["open", "begin", "confirm"] as const)(
  "sanitizes %s failures without rendering native messages",
  async (stage) => {
    mocks[stage].mockRejectedValue(new Error("SYNTHETIC_CREDENTIAL_CANARY"));
    render(<TraeAccounts />);
    if (stage === "confirm") await authorize();
    else {
      await openDialog();
      await userEvent.click(
        screen.getByRole("button", { name: "trae.selectFiles" }),
      );
    }
    expect(await screen.findByRole("alert")).toHaveTextContent("trae.failed");
    expect(
      screen.queryByText(/SYNTHETIC_CREDENTIAL_CANARY/),
    ).not.toBeInTheDocument();
  },
);

it("consumes a failed confirmation once and keeps cancellation/list review available", async () => {
  mocks.confirm.mockRejectedValue("publicationUnknown");
  render(<TraeAccounts />);
  await authorize();
  expect(await screen.findByRole("alert")).toHaveTextContent("trae.failed");
  expect(screen.getByText("trae.flowConsumed")).toBeVisible();
  const button = screen.getByRole("button", { name: "trae.confirm" });
  expect(button).toBeDisabled();
  expect(screen.getByRole("checkbox")).toBeDisabled();
  fireEvent.click(button);
  expect(mocks.confirm).toHaveBeenCalledTimes(1);
  await userEvent.click(
    screen.getByRole("button", { name: "trae.cancelAndReview" }),
  );
  expect(mocks.cancel).toHaveBeenCalledWith(flow.flowId);
  await waitFor(() => expect(mocks.list).toHaveBeenCalledTimes(2));
  expect(screen.getByRole("button", { name: "trae.reload" })).toBeEnabled();
  expect(screen.queryByText("trae.saved")).not.toBeInTheDocument();
});

it.each([
  { ...account, realm: "us" },
  { ...account, status: "refreshPending" },
  { ...account, id: "" },
])(
  "does not claim an inconsistent confirmation DTO was saved",
  async (invalid) => {
    mocks.confirm.mockResolvedValue(invalid);
    render(<TraeAccounts />);
    await authorize();
    expect(await screen.findByRole("alert")).toHaveTextContent("trae.failed");
    expect(screen.queryByText("trae.saved")).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: "trae.confirm" })).toBeDisabled();
    await userEvent.click(
      screen.getByRole("button", { name: "trae.cancelAndReview" }),
    );
    expect(mocks.cancel).toHaveBeenCalledExactlyOnceWith(flow.flowId);
  },
);

it("displays pending review as backend local state and never retries it automatically", async () => {
  mocks.list.mockResolvedValue([{ ...account, status: "refreshPending" }]);
  render(<TraeAccounts />);
  expect(await screen.findByText("trae.status.refreshPending")).toBeVisible();
  expect(mocks.invoke.mock.calls).toEqual([["list_trae_accounts"]]);
  expect(mocks.open).not.toHaveBeenCalled();
});

it("sanitizes a list failure and preserves the last safe snapshot", async () => {
  mocks.list
    .mockResolvedValueOnce([account])
    .mockRejectedValueOnce(new Error("SYNTHETIC_LIST_SECRET"));
  render(<TraeAccounts />);
  await screen.findByText(account.id);
  await userEvent.click(screen.getByRole("button", { name: "trae.reload" }));
  expect(await screen.findByRole("alert")).toHaveTextContent("trae.listFailed");
  expect(screen.getByText(account.id)).toBeVisible();
  expect(screen.queryByText(/SYNTHETIC_LIST_SECRET/)).not.toBeInTheDocument();
});

it("sanitizes a retire failure without inventing a retired state", async () => {
  mocks.list.mockResolvedValue([account]);
  mocks.retire.mockRejectedValue(new Error("SYNTHETIC_RETIRE_SECRET"));
  render(<TraeAccounts />);
  await screen.findByText(account.id);
  await userEvent.click(screen.getByRole("button", { name: "trae.retire" }));
  expect(await screen.findByRole("alert")).toHaveTextContent("trae.failed");
  expect(screen.getByText("trae.status.stored")).toBeVisible();
  expect(screen.queryByText("trae.status.retired")).not.toBeInTheDocument();
  expect(screen.queryByText(/SYNTHETIC_RETIRE_SECRET/)).not.toBeInTheDocument();
});

it.each(["success", "failure"])(
  "a stale initial list %s cannot undo a confirmed account",
  async (outcome) => {
    const initial = deferred<TraeAccountView[]>();
    mocks.list.mockReturnValueOnce(initial.promise);
    render(<TraeAccounts />);
    await authorize();
    expect(await screen.findByText(account.id)).toBeVisible();
    await act(async () => {
      if (outcome === "success") initial.resolve([]);
      else initial.reject(new Error("SYNTHETIC_STALE_LIST_SECRET"));
    });
    expect(screen.getByText(account.id)).toBeVisible();
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
    expect(screen.queryByText("trae.loading")).not.toBeInTheDocument();
  },
);

it("keeps the newest reload in charge of data and loading state", async () => {
  const first = deferred<TraeAccountView[]>();
  const second = deferred<TraeAccountView[]>();
  mocks.list
    .mockReturnValueOnce(first.promise)
    .mockReturnValueOnce(second.promise);
  render(<TraeAccounts />);
  await userEvent.click(screen.getByRole("button", { name: "trae.reload" }));
  await act(async () => first.resolve([]));
  expect(screen.getByText("trae.loading")).toBeVisible();
  expect(screen.queryByText("trae.empty")).not.toBeInTheDocument();
  await act(async () => second.resolve([account]));
  expect(screen.getByText(account.id)).toBeVisible();
  expect(screen.queryByText("trae.loading")).not.toBeInTheDocument();
});

it("ignores a retired StrictMode list and never starts native authorization on mount", async () => {
  const retired = deferred<TraeAccountView[]>();
  mocks.list
    .mockReturnValueOnce(retired.promise)
    .mockResolvedValueOnce([account]);
  render(
    <StrictMode>
      <TraeAccounts />
    </StrictMode>,
  );
  expect(await screen.findByText(account.id)).toBeVisible();
  await act(async () => retired.resolve([]));
  expect(screen.getByText(account.id)).toBeVisible();
  expect(mocks.list).toHaveBeenCalledTimes(2);
  expect(mocks.open).not.toHaveBeenCalled();
  expect(mocks.begin).not.toHaveBeenCalled();
});

it("retires only the requested local account, once, and displays backend readback", async () => {
  const pending = deferred<void>();
  mocks.list
    .mockResolvedValueOnce([account])
    .mockResolvedValueOnce([{ ...account, status: "retired" }]);
  mocks.retire.mockReturnValue(pending.promise);
  render(<TraeAccounts />);
  const row = (await screen.findByText(account.id)).closest("li")!;
  const button = within(row).getByRole("button", { name: "trae.retire" });
  act(() => {
    fireEvent.click(button);
    fireEvent.click(button);
  });
  expect(mocks.retire).toHaveBeenCalledExactlyOnceWith(account.id);
  expect(mocks.invoke).toHaveBeenCalledWith("retire_trae_account", {
    id: account.id,
  });
  await act(async () => pending.resolve());
  expect(await screen.findByText("trae.status.retired")).toBeVisible();
  expect(button).toBeDisabled();
  expect(mocks.begin).not.toHaveBeenCalled();
  expect(mocks.confirm).not.toHaveBeenCalled();
});

it("keeps international realm and local auth status without inventing check-in capability", async () => {
  mocks.list.mockResolvedValue([account]);
  render(<TraeAccounts unified />);
  const auth = await screen.findByRole("region", { name: "授权状态" });
  expect(auth).toHaveTextContent("SG");
  expect(
    within(auth).getByRole("button", { name: "trae.retire" }),
  ).toBeInTheDocument();
  expect(screen.getByRole("region", { name: "权益与签到" })).toHaveTextContent(
    "未提供",
  );
  expect(screen.getByRole("region", { name: "模型与推理" })).toHaveTextContent(
    "尚未验收",
  );
});

it("isolates CN Code accounts and consent from Global, without enabling check-in", async () => {
  const cn = { ...account, id: "synthetic-cn", realm: "cn" as const };
  mocks.list.mockResolvedValue([account, cn]);
  mocks.begin.mockResolvedValue({
    ...flow,
    realm: "cn",
    authHost: "https://api.trae.cn",
  });
  mocks.confirm.mockResolvedValue(cn);
  render(<TraeAccounts edition="cn" />);
  expect(await screen.findByText("synthetic-cn")).toBeVisible();
  expect(screen.queryByText(account.id)).not.toBeInTheDocument();
  expect(
    screen.getByText(/自动签到默认关闭，需在权益区逐账号开启/),
  ).toBeVisible();
  await userEvent.click(screen.getByRole("button", { name: "trae.add" }));
  expect(screen.queryByRole("radio", { name: "SG" })).not.toBeInTheDocument();
  expect(screen.queryByRole("radio", { name: "US" })).not.toBeInTheDocument();
  await userEvent.click(screen.getByRole("radio", { name: "CN" }));
  await userEvent.click(
    screen.getByRole("button", { name: "trae.selectFiles" }),
  );
  expect(mocks.begin).toHaveBeenCalledExactlyOnceWith({
    storagePath,
    productPath,
    realm: "cn",
  });
  expect(mocks.confirm).not.toHaveBeenCalled();
  await userEvent.click(
    await screen.findByRole("checkbox", { name: "trae.consent" }),
  );
  await userEvent.click(screen.getByRole("button", { name: "trae.confirm" }));
  await waitFor(() =>
    expect(mocks.confirm).toHaveBeenCalledExactlyOnceWith(flow.flowId),
  );
});

it("rejects Global authorization host returned to the CN entry without confirming", async () => {
  mocks.begin.mockResolvedValue({ ...flow, realm: "cn" });
  render(<TraeAccounts edition="cn" />);
  await userEvent.click(screen.getByRole("button", { name: "trae.add" }));
  await userEvent.click(screen.getByRole("radio", { name: "CN" }));
  await userEvent.click(
    screen.getByRole("button", { name: "trae.selectFiles" }),
  );
  await waitFor(() => expect(mocks.cancel).toHaveBeenCalledWith(flow.flowId));
  expect(mocks.confirm).not.toHaveBeenCalled();
});

it("one discovered installation skips file dialogs but never confirms without consent", async () => {
  mocks.discover.mockResolvedValue([
    { storagePath, productPath, version: "3.6.2" },
  ]);
  render(<TraeAccounts />);
  await openDialog();
  await userEvent.click(
    screen.getByRole("button", { name: "自动发现本机 TRAE 安装" }),
  );
  await screen.findByRole("checkbox", { name: "trae.consent" });
  expect(mocks.discover).toHaveBeenCalledExactlyOnceWith("sg");
  expect(mocks.open).not.toHaveBeenCalled();
  expect(mocks.begin).toHaveBeenCalledExactlyOnceWith({
    storagePath,
    productPath,
    realm: "sg",
  });
  expect(mocks.confirm).not.toHaveBeenCalled();
});
it("multiple installations require an explicit choice", async () => {
  const second = {
    storagePath,
    productPath: "/synthetic/other/product.json",
    version: "3.6.3",
  };
  mocks.discover.mockResolvedValue([
    { storagePath, productPath, version: "3.6.2" },
    second,
  ]);
  render(<TraeAccounts />);
  await openDialog();
  await userEvent.click(
    screen.getByRole("button", { name: "自动发现本机 TRAE 安装" }),
  );
  await screen.findByText("发现多个安装，请选择本次导入来源：");
  expect(mocks.begin).not.toHaveBeenCalled();
  await userEvent.click(
    screen.getByRole("button", { name: new RegExp("other/product.json") }),
  );
  await screen.findByRole("checkbox", { name: "trae.consent" });
  expect(mocks.begin).toHaveBeenCalledExactlyOnceWith({
    storagePath,
    productPath: second.productPath,
    realm: "sg",
  });
});
it("late discovery after cancellation cannot start an import", async () => {
  const pending = deferred<unknown[]>();
  mocks.discover.mockReturnValue(pending.promise);
  render(<TraeAccounts />);
  await openDialog();
  await userEvent.click(
    screen.getByRole("button", { name: "自动发现本机 TRAE 安装" }),
  );
  await userEvent.click(screen.getByRole("button", { name: "trae.cancel" }));
  await act(async () =>
    pending.resolve([{ storagePath, productPath, version: "3.6.2" }]),
  );
  expect(mocks.begin).not.toHaveBeenCalled();
  expect(mocks.confirm).not.toHaveBeenCalled();
});

it("CN check-in is explicit, at most one pending request and accepted is not credited", async () => {
  const pending = deferred<unknown>();
  mocks.list.mockResolvedValue([
    { ...account, realm: "cn", nativeSourceSupported: true },
  ]);
  const original = mocks.invoke.getMockImplementation()!;
  mocks.invoke.mockImplementation((command, args) =>
    command === "checkin_trae_account"
      ? pending.promise
      : original(command, args),
  );
  render(<TraeAccounts unified edition="cn" />);
  const button = await screen.findByRole("button", { name: "检查并尝试签到" });
  expect(
    mocks.invoke.mock.calls.filter(([cmd]) => cmd === "checkin_trae_account"),
  ).toHaveLength(0);
  fireEvent.click(button);
  fireEvent.click(button);
  expect(
    mocks.invoke.mock.calls.filter(([cmd]) => cmd === "checkin_trae_account"),
  ).toEqual([["checkin_trae_account", { id: account.id }]]);
  await act(async () =>
    pending.resolve({ status: "accepted", reportedCredits: 30 }),
  );
  expect(screen.getByText(/已受理，尚未确认签到或积分到账/)).toBeVisible();
  expect(screen.queryByText(/余额.*30/)).not.toBeInTheDocument();
});

it.each([
  ["done", "服务已确认此账号已签到"],
  ["inactive", "此账号当前未开放签到"],
  ["ineligible", "服务拒绝本次领取或账号不符合条件"],
  ["alreadyAttempted", "已尝试领取或仍有未确认的领取"],
])(
  "CN check-in renders %s without inferring a balance",
  async (status, message) => {
    mocks.list.mockResolvedValue([
      { ...account, realm: "cn", nativeSourceSupported: true },
    ]);
    const original = mocks.invoke.getMockImplementation()!;
    mocks.invoke.mockImplementation((command, args) =>
      command === "checkin_trae_account"
        ? Promise.resolve({ status })
        : original(command, args),
    );
    render(<TraeAccounts unified edition="cn" />);
    fireEvent.click(
      await screen.findByRole("button", { name: "检查并尝试签到" }),
    );
    expect(await screen.findByText(new RegExp(message))).toBeVisible();
    expect(
      mocks.invoke.mock.calls.filter(([cmd]) => cmd === "checkin_trae_account"),
    ).toHaveLength(1);
  },
);

it("global and unverified or retired CN accounts cannot send claims", async () => {
  mocks.list.mockResolvedValue([account]);
  const view = render(<TraeAccounts unified />);
  await screen.findByRole("button", { name: `查看 ${account.id}` });
  expect(
    screen.queryByRole("button", { name: "检查并尝试签到" }),
  ).not.toBeInTheDocument();
  view.unmount();
  mocks.list.mockResolvedValue([
    { ...account, realm: "cn" },
    {
      ...account,
      id: "retired-cn",
      realm: "cn",
      nativeSourceSupported: true,
      status: "retired",
    },
  ]);
  render(<TraeAccounts unified edition="cn" />);
  expect(
    await screen.findByRole("button", { name: "检查并尝试签到" }),
  ).toBeDisabled();
  fireEvent.click(screen.getByRole("button", { name: "查看 retired-cn" }));
  expect(screen.getByRole("button", { name: "检查并尝试签到" })).toBeDisabled();
  expect(
    mocks.invoke.mock.calls.filter(([cmd]) => cmd === "checkin_trae_account"),
  ).toHaveLength(0);
});

it("reload invalidates a pending check-in result without automatic claims", async () => {
  const pending = deferred<unknown>();
  mocks.list.mockResolvedValue([
    { ...account, realm: "cn", nativeSourceSupported: true },
    { ...account, id: "other-cn", realm: "cn", nativeSourceSupported: true },
  ]);
  const original = mocks.invoke.getMockImplementation()!;
  mocks.invoke.mockImplementation((command, args) =>
    command === "checkin_trae_account"
      ? pending.promise
      : original(command, args),
  );
  render(<TraeAccounts unified edition="cn" />);
  fireEvent.click(
    await screen.findByRole("button", { name: "检查并尝试签到" }),
  );
  fireEvent.click(screen.getByRole("button", { name: "查看 other-cn" }));
  expect(screen.getByRole("button", { name: "检查并尝试签到" })).toBeDisabled();
  fireEvent.click(screen.getByRole("button", { name: "trae.reload" }));
  await act(async () => pending.resolve({ status: "done" }));
  expect(screen.queryByText(/服务已确认此账号已签到/)).not.toBeInTheDocument();
  fireEvent.click(screen.getByRole("button", { name: `查看 ${account.id}` }));
  expect(screen.queryByText(/服务已确认此账号已签到/)).not.toBeInTheDocument();
  expect(
    mocks.invoke.mock.calls.filter(([cmd]) => cmd === "checkin_trae_account"),
  ).toHaveLength(1);
});

it("check-in failure is safe and does not auto retry", async () => {
  const pending = deferred<unknown>();
  mocks.list.mockResolvedValue([
    { ...account, realm: "cn", nativeSourceSupported: true },
  ]);
  const original = mocks.invoke.getMockImplementation()!;
  mocks.invoke.mockImplementation((command, args) =>
    command === "checkin_trae_account"
      ? pending.promise
      : original(command, args),
  );
  const view = render(<TraeAccounts unified edition="cn" />);
  fireEvent.click(
    await screen.findByRole("button", { name: "检查并尝试签到" }),
  );
  await act(async () => pending.reject(new Error("secret-upstream-error")));
  expect(screen.getByText(/无法确认签到结果，不会自动重试/)).toBeVisible();
  expect(screen.queryByText(/secret-upstream-error/)).not.toBeInTheDocument();
  expect(
    mocks.invoke.mock.calls.filter(([cmd]) => cmd === "checkin_trae_account"),
  ).toHaveLength(1);
  view.unmount();
});

it("ignores check-in completion after unmount and a fresh account view", async () => {
  const pending = deferred<unknown>();
  mocks.list.mockResolvedValue([
    { ...account, realm: "cn", nativeSourceSupported: true },
  ]);
  const original = mocks.invoke.getMockImplementation()!;
  mocks.invoke.mockImplementation((command, args) =>
    command === "checkin_trae_account"
      ? pending.promise
      : original(command, args),
  );
  const view = render(<TraeAccounts unified edition="cn" />);
  fireEvent.click(
    await screen.findByRole("button", { name: "检查并尝试签到" }),
  );
  view.unmount();
  render(<TraeAccounts unified edition="cn" />);
  await screen.findByRole("button", { name: "检查并尝试签到" });
  await act(async () => pending.resolve({ status: "done" }));
  expect(screen.queryByText(/服务已确认此账号已签到/)).not.toBeInTheDocument();
  expect(
    mocks.invoke.mock.calls.filter(([cmd]) => cmd === "checkin_trae_account"),
  ).toHaveLength(1);
});

it.each(["sg", "us"])(
  "%s accounts never expose or send a CN claim, even with native source metadata",
  async (realm) => {
    mocks.list.mockResolvedValue([
      { ...account, realm, nativeSourceSupported: true },
    ]);
    render(<TraeAccounts unified />);
    await screen.findByRole("button", { name: `查看 ${account.id}` });
    expect(
      screen.queryByRole("button", { name: "检查并尝试签到" }),
    ).not.toBeInTheDocument();
    expect(
      mocks.invoke.mock.calls.filter(([cmd]) => cmd === "checkin_trae_account"),
    ).toHaveLength(0);
  },
);

it("a late check-in result remains attached to its originating account", async () => {
  const pending = deferred<unknown>();
  mocks.list.mockResolvedValue([
    { ...account, realm: "cn", nativeSourceSupported: true },
    { ...account, id: "other-cn", realm: "cn", nativeSourceSupported: true },
  ]);
  const original = mocks.invoke.getMockImplementation()!;
  mocks.invoke.mockImplementation((command, args) =>
    command === "checkin_trae_account"
      ? pending.promise
      : original(command, args),
  );
  render(<TraeAccounts unified edition="cn" />);
  fireEvent.click(
    await screen.findByRole("button", { name: "检查并尝试签到" }),
  );
  fireEvent.click(screen.getByRole("button", { name: "查看 other-cn" }));
  await act(async () => pending.resolve({ status: "done" }));
  expect(screen.queryByText(/服务已确认此账号已签到/)).not.toBeInTheDocument();
  fireEvent.click(screen.getByRole("button", { name: `查看 ${account.id}` }));
  expect(screen.getByText(/服务已确认此账号已签到/)).toBeVisible();
  expect(
    mocks.invoke.mock.calls.filter(([cmd]) => cmd === "checkin_trae_account"),
  ).toEqual([["checkin_trae_account", { id: account.id }]]);
});

it("CN discovery reuses the native import flow without confirming or claiming", async () => {
  mocks.discover.mockResolvedValue([
    { storagePath, productPath, version: "3.6.2" },
  ]);
  mocks.begin.mockResolvedValue({
    ...flow,
    realm: "cn",
    authHost: "https://api.trae.cn",
  });
  render(<TraeAccounts unified edition="cn" />);
  await userEvent.click(screen.getByRole("button", { name: "trae.add" }));
  await userEvent.click(
    screen.getByRole("button", { name: "自动发现本机 TRAE 安装" }),
  );
  await screen.findByRole("checkbox", { name: "trae.consent" });
  expect(mocks.discover).toHaveBeenCalledExactlyOnceWith("cn");
  expect(mocks.begin).toHaveBeenCalledExactlyOnceWith({
    storagePath,
    productPath,
    realm: "cn",
  });
  expect(mocks.open).not.toHaveBeenCalled();
  expect(mocks.confirm).not.toHaveBeenCalled();
  expect(
    mocks.invoke.mock.calls.filter(([cmd]) => cmd === "checkin_trae_account"),
  ).toHaveLength(0);
});

it("automatic CN checkin requires explicit per-account opt-in and sends fenced policy only", async () => {
  const row = {
    ...account,
    realm: "cn",
    nativeSourceSupported: true,
    generation: 7,
    autoCheckinRevision: 3,
    autoCheckinEnabled: false,
    canSetAutomatic: true,
    checkinAttemptStatus: null,
  };
  mocks.list.mockResolvedValue([row]);
  const original = mocks.invoke.getMockImplementation()!;
  mocks.invoke.mockImplementation((command, args) =>
    command === "set_trae_automatic_checkin"
      ? Promise.resolve({
          ...row,
          autoCheckinRevision: 4,
          autoCheckinEnabled: true,
        })
      : original(command, args),
  );
  render(<TraeAccounts unified edition="cn" />);
  const enable = await screen.findByRole("button", { name: "开启自动签到" });
  expect(mocks.invoke.mock.calls).toEqual([["list_trae_accounts"]]);
  fireEvent.click(enable);
  expect(mocks.invoke).toHaveBeenCalledWith("set_trae_automatic_checkin", {
    id: account.id,
    generation: 7,
    policyRevision: 3,
    enabled: true,
  });
  await screen.findByRole("button", { name: "关闭自动签到" });
  expect(
    mocks.invoke.mock.calls.filter(([cmd]) => cmd === "checkin_trae_account"),
  ).toHaveLength(0);
});

it("automatic policy guards repeated clicks and preserves confirmed consent across reload", async () => {
  const row = {
    ...account,
    realm: "cn",
    nativeSourceSupported: true,
    generation: 2,
    autoCheckinRevision: 0,
    autoCheckinEnabled: false,
    canSetAutomatic: true,
    checkinAttemptStatus: "unknown",
  };
  mocks.list.mockResolvedValue([row]);
  const pending = deferred<unknown>();
  const original = mocks.invoke.getMockImplementation()!;
  mocks.invoke.mockImplementation((command, args) =>
    command === "set_trae_automatic_checkin"
      ? pending.promise
      : original(command, args),
  );
  render(<TraeAccounts unified edition="cn" />);
  const enable = await screen.findByRole("button", { name: "开启自动签到" });
  act(() => {
    fireEvent.click(enable);
    fireEvent.click(enable);
  });
  expect(
    mocks.invoke.mock.calls.filter(
      ([cmd]) => cmd === "set_trae_automatic_checkin",
    ),
  ).toHaveLength(1);
  fireEvent.click(screen.getByRole("button", { name: "trae.reload" }));
  await waitFor(() => expect(mocks.list).toHaveBeenCalledTimes(2));
  await act(async () =>
    pending.resolve({
      ...row,
      autoCheckinRevision: 1,
      autoCheckinEnabled: true,
    }),
  );
  expect(screen.getByRole("button", { name: "关闭自动签到" })).toBeVisible();
  expect(screen.getByText(/仍有未确认的领取/)).toBeVisible();
});

it("an older list response cannot overwrite a newly confirmed automatic policy", async () => {
  const row = {
    ...account,
    realm: "cn",
    nativeSourceSupported: true,
    generation: 2,
    autoCheckinRevision: 0,
    autoCheckinEnabled: false,
    canSetAutomatic: true,
    checkinAttemptStatus: null,
  };
  const stale = deferred<unknown[]>();
  mocks.list.mockResolvedValueOnce([row]).mockReturnValueOnce(stale.promise);
  const original = mocks.invoke.getMockImplementation()!;
  mocks.invoke.mockImplementation((command, args) =>
    command === "set_trae_automatic_checkin"
      ? Promise.resolve({
          ...row,
          autoCheckinRevision: 1,
          autoCheckinEnabled: true,
        })
      : original(command, args),
  );
  render(<TraeAccounts unified edition="cn" />);
  await screen.findByRole("button", { name: "开启自动签到" });
  fireEvent.click(screen.getByRole("button", { name: "trae.reload" }));
  fireEvent.click(screen.getByRole("button", { name: "开启自动签到" }));
  await screen.findByRole("button", { name: "关闭自动签到" });
  await act(async () => stale.resolve([row]));
  expect(screen.getByRole("button", { name: "关闭自动签到" })).toBeVisible();
  expect(screen.queryByText("trae.loading")).not.toBeInTheDocument();
});

it("opening and closing an empty authorization dialog does not discard confirmed policy", async () => {
  const row = {
    ...account,
    realm: "cn",
    nativeSourceSupported: true,
    generation: 2,
    autoCheckinRevision: 0,
    autoCheckinEnabled: false,
    canSetAutomatic: true,
    checkinAttemptStatus: null,
  };
  const pending = deferred<unknown>();
  mocks.list.mockResolvedValue([row]);
  const original = mocks.invoke.getMockImplementation()!;
  mocks.invoke.mockImplementation((command, args) =>
    command === "set_trae_automatic_checkin"
      ? pending.promise
      : original(command, args),
  );
  render(<TraeAccounts unified edition="cn" />);
  fireEvent.click(await screen.findByRole("button", { name: "开启自动签到" }));
  fireEvent.click(screen.getByRole("button", { name: "trae.add" }));
  fireEvent.click(screen.getByRole("button", { name: "trae.cancel" }));
  await act(async () =>
    pending.resolve({
      ...row,
      autoCheckinRevision: 1,
      autoCheckinEnabled: true,
    }),
  );
  expect(screen.getByRole("button", { name: "关闭自动签到" })).toBeVisible();
  expect(mocks.confirm).not.toHaveBeenCalled();
});

it("a newer account generation from reload is not overwritten by an older policy reply", async () => {
  const row = {
    ...account,
    realm: "cn",
    nativeSourceSupported: true,
    generation: 2,
    autoCheckinRevision: 0,
    autoCheckinEnabled: false,
    canSetAutomatic: true,
    checkinAttemptStatus: null,
  };
  const pending = deferred<unknown>();
  const refresh = deferred<unknown[]>();
  mocks.list.mockResolvedValueOnce([row]).mockReturnValueOnce(refresh.promise);
  const original = mocks.invoke.getMockImplementation()!;
  mocks.invoke.mockImplementation((command, args) =>
    command === "set_trae_automatic_checkin"
      ? pending.promise
      : original(command, args),
  );
  render(<TraeAccounts unified edition="cn" />);
  fireEvent.click(await screen.findByRole("button", { name: "开启自动签到" }));
  fireEvent.click(screen.getByRole("button", { name: "trae.reload" }));
  await act(async () =>
    pending.resolve({
      ...row,
      autoCheckinRevision: 1,
      autoCheckinEnabled: true,
    }),
  );
  await act(async () =>
    refresh.resolve([{ ...row, generation: 3, autoCheckinRevision: 2 }]),
  );
  expect(screen.getByRole("button", { name: "开启自动签到" })).toBeVisible();
});

const automaticCnRow = () => ({
  ...account,
  realm: "cn",
  nativeSourceSupported: true,
  generation: 2,
  autoCheckinRevision: 0,
  autoCheckinEnabled: false,
  canSetAutomatic: true,
  checkinAttemptStatus: null,
});

it("reconciles a lost automatic policy reply without repeating the setter", async () => {
  const row = automaticCnRow();
  mocks.list
    .mockResolvedValueOnce([row])
    .mockResolvedValueOnce([
      { ...row, autoCheckinRevision: 1, autoCheckinEnabled: true },
    ]);
  const original = mocks.invoke.getMockImplementation()!;
  mocks.invoke.mockImplementation((command, args) =>
    command === "set_trae_automatic_checkin"
      ? Promise.reject(new Error("synthetic lost reply"))
      : original(command, args),
  );
  render(<TraeAccounts unified edition="cn" />);
  fireEvent.click(await screen.findByRole("button", { name: "开启自动签到" }));
  expect(
    await screen.findByRole("button", { name: "关闭自动签到" }),
  ).toBeEnabled();
  expect(mocks.list).toHaveBeenCalledTimes(2);
  expect(
    mocks.invoke.mock.calls.filter(
      ([cmd]) => cmd === "set_trae_automatic_checkin",
    ),
  ).toHaveLength(1);
});

it("keeps failed policy readback unknown and disabled until a fresh reload", async () => {
  const row = automaticCnRow();
  mocks.list
    .mockResolvedValueOnce([row])
    .mockRejectedValueOnce(new Error("synthetic readback failure"))
    .mockResolvedValueOnce([
      { ...row, autoCheckinRevision: 1, autoCheckinEnabled: true },
    ]);
  const original = mocks.invoke.getMockImplementation()!;
  mocks.invoke.mockImplementation((command, args) =>
    command === "set_trae_automatic_checkin"
      ? Promise.reject(new Error("synthetic lost reply"))
      : original(command, args),
  );
  render(<TraeAccounts unified edition="cn" />);
  fireEvent.click(await screen.findByRole("button", { name: "开启自动签到" }));
  const unknown = await screen.findByRole("button", {
    name: "自动签到状态未知",
  });
  expect(unknown).toBeDisabled();
  expect(screen.queryByText(/自动签到已关闭/)).not.toBeInTheDocument();
  fireEvent.click(unknown);
  expect(
    mocks.invoke.mock.calls.filter(
      ([cmd]) => cmd === "set_trae_automatic_checkin",
    ),
  ).toHaveLength(1);
  fireEvent.click(screen.getByRole("button", { name: "trae.reload" }));
  expect(
    await screen.findByRole("button", { name: "关闭自动签到" }),
  ).toBeEnabled();
  expect(mocks.list).toHaveBeenCalledTimes(3);
});

it("a list started before policy uncertainty cannot clear its unknown state", async () => {
  const row = automaticCnRow();
  const stale = deferred<unknown[]>();
  mocks.list
    .mockResolvedValueOnce([row])
    .mockReturnValueOnce(stale.promise)
    .mockRejectedValueOnce(new Error("synthetic readback failure"));
  const original = mocks.invoke.getMockImplementation()!;
  mocks.invoke.mockImplementation((command, args) =>
    command === "set_trae_automatic_checkin"
      ? Promise.reject(new Error("synthetic lost reply"))
      : original(command, args),
  );
  render(<TraeAccounts unified edition="cn" />);
  const enable = await screen.findByRole("button", { name: "开启自动签到" });
  fireEvent.click(screen.getByRole("button", { name: "trae.reload" }));
  fireEvent.click(enable);
  expect(
    await screen.findByRole("button", { name: "自动签到状态未知" }),
  ).toBeDisabled();
  await act(async () => stale.resolve([row]));
  expect(
    screen.getByRole("button", { name: "自动签到状态未知" }),
  ).toBeDisabled();
  expect(screen.queryByText(/自动签到已关闭/)).not.toBeInTheDocument();
});

it("late uncertain-policy readback cannot overwrite a newer account identity", async () => {
  const row = automaticCnRow();
  const readback = deferred<unknown[]>();
  const newer = { ...row, generation: 3, autoCheckinRevision: 2 };
  mocks.list
    .mockResolvedValueOnce([row])
    .mockReturnValueOnce(readback.promise)
    .mockResolvedValueOnce([newer]);
  const original = mocks.invoke.getMockImplementation()!;
  mocks.invoke.mockImplementation((command, args) =>
    command === "set_trae_automatic_checkin"
      ? Promise.reject(new Error("synthetic lost reply"))
      : original(command, args),
  );
  render(<TraeAccounts unified edition="cn" />);
  fireEvent.click(await screen.findByRole("button", { name: "开启自动签到" }));
  await screen.findByRole("button", { name: "自动签到状态未知" });
  fireEvent.click(screen.getByRole("button", { name: "trae.reload" }));
  await screen.findByRole("button", { name: "开启自动签到" });
  await act(async () =>
    readback.resolve([
      { ...row, autoCheckinRevision: 1, autoCheckinEnabled: true },
    ]),
  );
  expect(screen.getByRole("button", { name: "开启自动签到" })).toBeEnabled();
  expect(
    screen.queryByRole("button", { name: "关闭自动签到" }),
  ).not.toBeInTheDocument();
  expect(
    mocks.invoke.mock.calls.filter(
      ([cmd]) => cmd === "set_trae_automatic_checkin",
    ),
  ).toHaveLength(1);
});
