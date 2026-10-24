import { act, fireEvent, render, screen, within } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { QoderCnDeviceLogin } from "@/components/providers/qoder/QoderCnDeviceLogin";
import { QoderAccountPanel } from "@/components/relay/accounts/QoderAccountPanel";
import { PreservedView } from "@/components/ui/PreservedView";
import type { Provider } from "@/types";

const api = vi.hoisted(() => ({
  start: vi.fn(),
  poll: vi.fn(),
  cancel: vi.fn(),
  context: vi.fn(),
  openExternal: vi.fn(),
  getAll: vi.fn(),
}));
vi.mock("@/lib/api/providers", () => ({
  providersApi: { getAll: api.getAll },
}));
vi.mock("@tauri-apps/api/core", () => ({
  invoke: (command: string, args: unknown) => {
    if (command === "qoder_cn_device_login_start") return api.start(args);
    if (command === "qoder_cn_device_login_poll") return api.poll(args);
    if (command === "qoder_cn_device_login_cancel") return api.cancel(args);
    if (command === "qoder_http_identity_context") return api.context(args);
    if (command === "open_external") return api.openExternal(args);
    throw new Error(`Unexpected command: ${command}`);
  },
}));

const addLabel = "添加 Qoder CN 独立授权账号";
const startLabel = "在浏览器中授权";
const doneLabel = "完成，返回账号列表";
const loginLabel = "登录 / 重新授权";
const rebindLabel = "更换绑定账号";
const authUrl =
  "https://qoder.cn/device/selectAccounts?session=synthetic-public-challenge";
const now = Date.UTC(2026, 9, 22);
const started = {
  status: "started",
  sessionId: "session-a",
  providerId: "created-provider",
  credentialGeneration: "generation-a",
  authUrl,
  expiresAtMs: now + 60_000,
  pollIntervalMs: 1_000,
};
const deviceContext = {
  providerId: "device-a",
  credentialGeneration: "generation-a",
  source: "chinaQoderDevice",
  status: "ready",
  expiresAtMs: now + 120_000,
  allowedActions: [],
  allowedDeviceActions: ["login", "rebind"],
};
const device = (id = "device-a", generation = "generation-a"): Provider =>
  ({
    id,
    name: `独立授权 ${id}`,
    settingsConfig: {},
    meta: {
      providerType: "qoder_cn_device",
      qoderCampaignState: { generation },
    },
  }) as Provider;
function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((r) => {
    resolve = r;
  });
  return { promise, resolve };
}
async function click(name: string) {
  await act(async () => {
    fireEvent.click(screen.getByRole("button", { name }));
  });
}
async function tick(ms = 1_000) {
  await act(async () => {
    await vi.advanceTimersByTimeAsync(ms);
  });
}
async function setup(providers: Record<string, Provider> = {}) {
  api.getAll.mockResolvedValue(providers);
  const client = new QueryClient({
    defaultOptions: {
      queries: { retry: false, staleTime: Infinity },
      mutations: { retry: false },
    },
  });
  client.setQueryData(["providers", "codex"], { providers });
  const content = (active: boolean) => (
    <QueryClientProvider client={client}>
      <PreservedView active={active}>
        <QoderAccountPanel />
      </PreservedView>
    </QueryClientProvider>
  );
  const view = render(content(true));
  await act(async () => {});
  return { client, view, content };
}
async function openNew() {
  await click(addLabel);
}
async function startNew() {
  await openNew();
  await click(startLabel);
}
beforeEach(() => {
  vi.useFakeTimers();
  vi.setSystemTime(now);
  api.start.mockReset().mockImplementation(async ({ request }) => ({
    ...started,
    providerId: request.providerId ?? started.providerId,
    credentialGeneration:
      request.credentialGeneration ?? started.credentialGeneration,
  }));
  api.poll.mockReset().mockResolvedValue({ status: "pending" });
  api.cancel.mockReset().mockResolvedValue({ status: "cancelled" });
  api.context.mockReset().mockResolvedValue(deviceContext);
  api.openExternal.mockReset().mockResolvedValue(undefined);
  api.getAll.mockReset().mockResolvedValue({});
});
afterEach(() => vi.useRealTimers());

it("offers the actual CN entry with no textbox or automatic grant and keeps the dual-region heading", async () => {
  await setup();
  expect(
    screen.getByRole("heading", { name: "Qoder 账号（国际版 / CN）" }),
  ).toBeInTheDocument();
  expect(screen.getByRole("button", { name: addLabel })).toBeEnabled();
  expect(screen.queryAllByRole("textbox")).toHaveLength(0);
  expect(api.start).not.toHaveBeenCalled();
  await openNew();
  expect(screen.getByRole("dialog")).toHaveTextContent("不读取官方认证文件");
  expect(screen.getByRole("dialog")).toHaveTextContent(
    "不能保证服务器会话一定并存",
  );
  expect(screen.getByRole("dialog")).toHaveTextContent(
    "免费福利请在账号的“CN 活动状态”中单独查询或领取",
  );
  expect(screen.getByRole("dialog")).toHaveTextContent("不会随登录自动开启");
  expect(screen.getByRole("dialog")).not.toHaveTextContent("CN 签到仍未接通");
  expect(api.start).not.toHaveBeenCalled();
});
it("starts exactly once on an explicit action and polls only the returned session", async () => {
  const { view, content } = await setup();
  await startNew();
  expect(api.start).toHaveBeenCalledExactlyOnceWith({
    request: { providerId: null, credentialGeneration: null, rebind: false },
  });
  expect(api.openExternal).toHaveBeenCalledExactlyOnceWith({ url: authUrl });
  expect(api.poll).not.toHaveBeenCalled();
  view.rerender(content(true));
  await tick();
  expect(api.poll).toHaveBeenCalledExactlyOnceWith({ sessionId: "session-a" });
  expect(api.start).toHaveBeenCalledTimes(1);
});
it("cancels locally without claiming server revocation and stops polling", async () => {
  await setup();
  await startNew();
  await click("取消本次授权");
  expect(api.cancel).toHaveBeenCalledExactlyOnceWith({
    sessionId: "session-a",
  });
  expect(screen.getByRole("dialog")).toHaveTextContent("不代表撤销服务器授权");
  expect(
    screen.queryByRole("button", { name: doneLabel }),
  ).not.toBeInTheDocument();
  expect(screen.getByRole("button", { name: startLabel })).toBeEnabled();
  await tick(60_000);
  expect(api.poll).not.toHaveBeenCalled();
  expect(api.start).toHaveBeenCalledTimes(1);
});
it("cancels when the actual dialog closes", async () => {
  await setup();
  await startNew();
  await act(async () => {
    fireEvent.keyDown(document, { key: "Escape" });
  });
  expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
  expect(api.cancel).toHaveBeenCalledExactlyOnceWith({
    sessionId: "session-a",
  });
  await tick(10_000);
  expect(api.poll).not.toHaveBeenCalled();
});
it.each(["unmount", "preserve"])(
  "cancels on %s and never resumes automatically",
  async (kind) => {
    const { view, content } = await setup();
    await startNew();
    await act(async () => {
      if (kind === "unmount") view.unmount();
      else view.rerender(content(false));
    });
    expect(api.cancel).toHaveBeenCalledExactlyOnceWith({
      sessionId: "session-a",
    });
    await tick(10_000);
    expect(api.poll).not.toHaveBeenCalled();
    if (kind === "preserve") {
      await act(async () => {
        view.rerender(content(true));
      });
      expect(api.start).toHaveBeenCalledTimes(1);
      expect(screen.queryByText("正在等待浏览器授权…")).not.toBeInTheDocument();
    }
  },
);
it("immediately cancels a late start without opening a browser or poll", async () => {
  const pending = deferred<typeof started>();
  api.start.mockReturnValue(pending.promise);
  await setup();
  await startNew();
  await act(async () => {
    fireEvent.keyDown(document, { key: "Escape" });
  });
  await act(async () => {
    pending.resolve(started);
  });
  expect(api.cancel).toHaveBeenCalledExactlyOnceWith({
    sessionId: "session-a",
  });
  expect(api.openExternal).not.toHaveBeenCalled();
  await tick(10_000);
  expect(api.poll).not.toHaveBeenCalled();
});
it("ignores an old completed poll after the user starts a different session", async () => {
  const pending = deferred<{ status: string; providerId: string }>();
  api.poll
    .mockReturnValueOnce(pending.promise)
    .mockResolvedValue({ status: "pending" });
  const { client } = await setup();
  const invalidate = vi.spyOn(client, "invalidateQueries");
  await startNew();
  await tick();
  await click("取消本次授权");
  api.start.mockResolvedValue({ ...started, sessionId: "session-b" });
  await click(startLabel);
  invalidate.mockClear();
  await act(async () => {
    pending.resolve({ status: "completed", providerId: "created-provider" });
  });
  expect(screen.getByRole("dialog")).toHaveTextContent("正在等待浏览器授权…");
  expect(invalidate).not.toHaveBeenCalled();
  await tick();
  expect(api.poll).toHaveBeenLastCalledWith({ sessionId: "session-b" });
});
it.each(["provider", "generation"])(
  "cancels and ignores the old poll after original %s changes",
  async (kind) => {
    const pending = deferred<{ status: string; providerId: string }>();
    api.poll.mockReturnValue(pending.promise);
    const { client } = await setup({ a: device() });
    await click(loginLabel);
    await click(startLabel);
    await tick();
    const invalidate = vi.spyOn(client, "invalidateQueries");
    await act(async () => {
      client.setQueryData(["providers", "codex"], {
        providers: {
          a:
            kind === "provider"
              ? device("device-b")
              : device("device-a", "generation-b"),
        },
      });
      await vi.advanceTimersByTimeAsync(0);
    });
    expect(api.cancel).toHaveBeenCalledExactlyOnceWith({
      sessionId: "session-a",
    });
    await act(async () => {
      pending.resolve({ status: "completed", providerId: "device-a" });
    });
    expect(invalidate).not.toHaveBeenCalled();
    expect(api.start).toHaveBeenCalledTimes(1);
  },
);
it("passes the displayed backend binding for login, and rebind only after the explicit change-account action", async () => {
  await setup({ a: device("device-a", "lagging-card-generation") });
  await click(loginLabel);
  expect(api.start).not.toHaveBeenCalled();
  await click(startLabel);
  expect(api.start).toHaveBeenLastCalledWith({
    request: {
      providerId: "device-a",
      credentialGeneration: "generation-a",
      rebind: false,
    },
  });
  await act(async () => {
    fireEvent.keyDown(document, { key: "Escape" });
  });
  await click(rebindLabel);
  expect(screen.getByRole("dialog")).toHaveTextContent("替换此账号的绑定");
  await click("确认更换并在浏览器中授权");
  expect(api.start).toHaveBeenLastCalledWith({
    request: {
      providerId: "device-a",
      credentialGeneration: "generation-a",
      rebind: true,
    },
  });
});
it.each([
  { ...deviceContext, providerId: "different-provider" },
  { ...deviceContext, credentialGeneration: "" },
  { ...deviceContext, source: "chinaQoderPat" },
  { ...deviceContext, allowedDeviceActions: [] },
  { ...deviceContext, status: "unknown", allowedDeviceActions: [] },
])(
  "keeps non-authoritative or disallowed device context inert: %j",
  async (context) => {
    api.context.mockResolvedValue(context);
    await setup({ a: device() });
    await click(loginLabel);
    expect(screen.getByRole("button", { name: startLabel })).toBeDisabled();
    expect(api.start).not.toHaveBeenCalled();
  },
);
it("waits for authoritative context instead of starting while loading", async () => {
  api.context.mockReturnValue(new Promise(() => {}));
  await setup({ a: device() });
  await click(loginLabel);
  expect(screen.getByRole("button", { name: startLabel })).toBeDisabled();
  expect(api.start).not.toHaveBeenCalled();
});
it.each(["failed", "rejected"])(
  "does not retry or expose raw start %s details",
  async (kind) => {
    if (kind === "failed")
      api.start.mockResolvedValue({
        status: "failed",
        code: "SYNTHETIC-SECRET",
      });
    else api.start.mockRejectedValue(new Error("SYNTHETIC-SECRET"));
    await setup();
    await startNew();
    expect(
      within(screen.getByRole("dialog")).getByRole("alert"),
    ).not.toBeEmptyDOMElement();
    expect(document.body.textContent).not.toContain("SYNTHETIC-SECRET");
    await tick(10_000);
    expect(api.start).toHaveBeenCalledTimes(1);
    expect(api.poll).not.toHaveBeenCalled();
  },
);
it("does not retry a failed poll, displays safe feedback, and releases the local session", async () => {
  api.poll.mockRejectedValue(new Error("SYNTHETIC-PRIVATE-TOKEN"));
  await setup();
  await startNew();
  await tick(10_000);
  expect(api.poll).toHaveBeenCalledTimes(1);
  expect(api.cancel).toHaveBeenCalledExactlyOnceWith({
    sessionId: "session-a",
  });
  expect(
    within(screen.getByRole("dialog")).getByRole("alert"),
  ).not.toBeEmptyDOMElement();
  expect(document.body.textContent).not.toContain("SYNTHETIC-PRIVATE");
  expect(api.start).toHaveBeenCalledTimes(1);
});
it("expires even when a poll is still in flight and ignores its late success", async () => {
  const pending = deferred<{ status: string; providerId: string }>();
  api.poll.mockReturnValue(pending.promise);
  const { client } = await setup();
  const invalidate = vi.spyOn(client, "invalidateQueries");
  await startNew();
  invalidate.mockClear();
  await tick(60_001);
  expect(api.cancel).toHaveBeenCalledExactlyOnceWith({
    sessionId: "session-a",
  });
  expect(
    within(screen.getByRole("dialog")).getByRole("alert"),
  ).toHaveTextContent("过期");
  await act(async () => {
    pending.resolve({ status: "completed", providerId: "created-provider" });
  });
  expect(invalidate).not.toHaveBeenCalled();
});
it.each([
  "http://qoder.cn/device/selectAccounts",
  "https://evil.example/device/selectAccounts",
  "https://qoder.cn.evil.example/device/selectAccounts",
  "https://qoder.cn/other",
  "https://user:pass@qoder.cn/device/selectAccounts",
])("rejects an unexpected browser target: %s", async (url) => {
  api.start.mockResolvedValue({ ...started, authUrl: url });
  await setup();
  await startNew();
  expect(api.openExternal).not.toHaveBeenCalled();
  expect(api.cancel).toHaveBeenCalledExactlyOnceWith({
    sessionId: "session-a",
  });
  expect(
    within(screen.getByRole("dialog")).getByRole("alert"),
  ).not.toBeEmptyDOMElement();
});
it("cancels after a browser opener failure without polling or restarting", async () => {
  api.openExternal.mockRejectedValue(new Error("SYNTHETIC-PRIVATE"));
  await setup();
  await startNew();
  await tick(10_000);
  expect(api.cancel).toHaveBeenCalledExactlyOnceWith({
    sessionId: "session-a",
  });
  expect(api.poll).not.toHaveBeenCalled();
  expect(api.start).toHaveBeenCalledTimes(1);
  expect(document.body.textContent).not.toContain("SYNTHETIC-PRIVATE");
});
it("completes visibly, refreshes the saved account, and returns without cancelling or restarting authorization", async () => {
  api.poll.mockResolvedValue({
    status: "completed",
    providerId: "created-provider",
  });
  const { client } = await setup();
  const invalidate = vi.spyOn(client, "invalidateQueries");
  await startNew();
  api.getAll.mockResolvedValue({ saved: device("created-provider") });
  await tick();
  expect(invalidate).toHaveBeenCalledWith({ queryKey: ["providers", "codex"] });
  expect(invalidate).toHaveBeenCalledWith({
    queryKey: ["qoderHttpIdentity", "created-provider"],
  });
  expect(screen.getByRole("dialog")).toHaveTextContent(
    "授权已保存，不代表真实推理已验证",
  );
  expect(screen.getByRole("button", { name: doneLabel })).toBeEnabled();
  expect(screen.getByRole("dialog")).toHaveTextContent(
    "免费福利请在账号的“CN 活动状态”中单独查询或领取",
  );
  expect(screen.getByRole("dialog")).toHaveTextContent("不会随登录自动开启");
  expect(screen.getByRole("dialog")).not.toHaveTextContent("CN 签到仍未接通");
  // Flush the query notification scheduled after the completed poll resolves.
  expect(client.getQueryData(["providers", "codex"])).toEqual({
    providers: { saved: device("created-provider") },
  });
  await act(async () => {
    await vi.advanceTimersToNextTimerAsync();
  });
  expect(
    within(screen.getByLabelText("账号详情")).getByText(
      "独立授权 created-provider",
    ),
  ).toBeInTheDocument();
  expect(
    screen.queryByRole("button", { name: startLabel }),
  ).not.toBeInTheDocument();
  expect(
    screen.queryByText("关闭或取消只停止本地等待，不代表撤销服务器授权。"),
  ).not.toBeInTheDocument();
  await click(doneLabel);
  expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
  expect(api.cancel).not.toHaveBeenCalled();
  expect(api.start).toHaveBeenCalledTimes(1);
  await openNew();
  expect(screen.getByRole("button", { name: startLabel })).toBeEnabled();
  expect(
    screen.queryByRole("button", { name: doneLabel }),
  ).not.toBeInTheDocument();
  expect(
    screen.queryByText(/授权已保存，不代表真实推理已验证/),
  ).not.toBeInTheDocument();
  await tick(60_000);
  expect(api.poll).toHaveBeenCalledTimes(1);
  expect(api.start).toHaveBeenCalledTimes(1);
  expect(api.cancel).not.toHaveBeenCalled();
});

it("does not deduplicate a new source context onto the previous generation's pending read", async () => {
  const oldContext = deferred<typeof deviceContext>();
  api.context.mockReturnValueOnce(oldContext.promise).mockResolvedValue({
    ...deviceContext,
    credentialGeneration: "generation-b",
  });
  const { client } = await setup({ a: device() });
  await click(loginLabel);
  await act(async () => {
    client.setQueryData(["providers", "codex"], {
      providers: { a: device("device-a", "generation-b") },
    });
    await vi.advanceTimersByTimeAsync(0);
  });
  await click(loginLabel);
  expect(api.context).toHaveBeenCalledTimes(2);
  await act(async () => {
    oldContext.resolve(deviceContext);
  });
  await click(startLabel);
  expect(api.start).toHaveBeenCalledExactlyOnceWith({
    request: {
      providerId: "device-a",
      credentialGeneration: "generation-b",
      rebind: false,
    },
  });
});

it.each(["unknown", "pending"])(
  "allows backend-authorized explicit recovery from %s without retrying the old grant",
  async (status) => {
    api.context.mockResolvedValue({
      ...deviceContext,
      status,
      allowedDeviceActions: ["login"],
    });
    await setup({ a: device() });
    await click(loginLabel);
    await click(startLabel);
    expect(api.start).toHaveBeenCalledExactlyOnceWith({
      request: {
        providerId: "device-a",
        credentialGeneration: "generation-a",
        rebind: false,
      },
    });
  },
);

it("never overlaps poll requests while the previous poll is pending", async () => {
  const pending = deferred<{ status: string }>();
  api.poll
    .mockReturnValueOnce(pending.promise)
    .mockResolvedValue({ status: "pending" });
  await setup();
  await startNew();
  await tick(10_000);
  expect(api.poll).toHaveBeenCalledTimes(1);
  await act(async () => {
    pending.resolve({ status: "pending" });
  });
  await tick();
  expect(api.poll).toHaveBeenCalledTimes(2);
});

it("ignores a browser opener response after cancel instead of starting an old poll", async () => {
  const pending = deferred<void>();
  api.openExternal.mockReturnValue(pending.promise);
  await setup();
  await startNew();
  await click("取消本次授权");
  await act(async () => {
    pending.resolve();
  });
  await tick(10_000);
  expect(api.poll).not.toHaveBeenCalled();
  expect(api.cancel).toHaveBeenCalledExactlyOnceWith({
    sessionId: "session-a",
  });
});

it("does not announce success for a different provider returned by poll", async () => {
  api.poll.mockResolvedValue({
    status: "completed",
    providerId: "other-provider",
  });
  const { client } = await setup();
  const invalidate = vi.spyOn(client, "invalidateQueries");
  await startNew();
  invalidate.mockClear();
  await tick();
  expect(
    within(screen.getByRole("dialog")).getByRole("alert"),
  ).not.toBeEmptyDOMElement();
  expect(invalidate).not.toHaveBeenCalled();
  expect(api.cancel).toHaveBeenCalledExactlyOnceWith({
    sessionId: "session-a",
  });
});

it.each(["provider", "generation", "disabled"])(
  "invalidates a pending start on a same-instance %s change",
  async (kind) => {
    const pending = deferred<typeof started>();
    api.start.mockReturnValue(pending.promise);
    const client = new QueryClient({
      defaultOptions: { queries: { retry: false } },
    });
    const content = (changed: boolean) => (
      <QueryClientProvider client={client}>
        <QoderCnDeviceLogin
          providerId={changed && kind === "provider" ? "device-b" : "device-a"}
          sourceGeneration={
            changed && kind === "generation" ? "generation-b" : "generation-a"
          }
          disabled={changed && kind === "disabled"}
        />
      </QueryClientProvider>
    );
    const view = render(content(false));
    await click(loginLabel);
    await click(startLabel);
    await act(async () => {
      view.rerender(content(true));
    });
    await act(async () => {
      pending.resolve({ ...started, providerId: "device-a" });
    });
    expect(api.cancel).toHaveBeenCalledExactlyOnceWith({
      sessionId: "session-a",
    });
    expect(api.openExternal).not.toHaveBeenCalled();
    expect(api.poll).not.toHaveBeenCalled();
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
  },
);

it("prevents parent card actions when opening and submitting device authorization", async () => {
  const parentClick = vi.fn();
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  render(
    <QueryClientProvider client={client}>
      <div onClick={parentClick}>
        <QoderCnDeviceLogin />
      </div>
    </QueryClientProvider>,
  );
  await startNew();
  expect(parentClick).not.toHaveBeenCalled();
});

it("keeps the backend-saved pending row visible after cancelling a new grant instead of implying no account was saved", async () => {
  await setup();
  api.getAll.mockResolvedValue({ pending: device("created-provider") });
  await startNew();
  await tick(0);
  await click("取消本次授权");
  await act(async () => {
    fireEvent.keyDown(document, { key: "Escape" });
  });
  expect(
    within(screen.getByLabelText("账号详情")).getByText(
      "独立授权 created-provider",
    ),
  ).toBeInTheDocument();
  expect(screen.getByRole("button", { name: loginLabel })).toBeInTheDocument();
  expect(api.start).toHaveBeenCalledTimes(1);
  expect(
    screen.queryByText(/授权已保存，不代表真实推理已验证/),
  ).not.toBeInTheDocument();
});

it("invalidates pending records when start returns after unmount without reviving the old UI", async () => {
  const pending = deferred<typeof started>();
  api.start.mockReturnValue(pending.promise);
  const { client, view } = await setup();
  const invalidate = vi.spyOn(client, "invalidateQueries");
  await startNew();
  view.unmount();
  await act(async () => {
    pending.resolve(started);
  });
  expect(invalidate).toHaveBeenCalledWith({ queryKey: ["providers", "codex"] });
  expect(client.getQueryState(["providers", "codex"])?.isInvalidated).toBe(
    true,
  );
  expect(api.cancel).toHaveBeenCalledExactlyOnceWith({
    sessionId: "session-a",
  });
  expect(api.openExternal).not.toHaveBeenCalled();
  expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
});

it("refreshes backend-saved pending records after a failed start without retrying or claiming authorization success", async () => {
  api.start.mockResolvedValue({ status: "failed", code: "unavailable" });
  await setup();
  api.getAll.mockResolvedValue({ pending: device("created-provider") });
  await startNew();
  await tick(0);
  expect(
    within(screen.getByRole("dialog")).getByRole("alert"),
  ).not.toBeEmptyDOMElement();
  await act(async () => {
    fireEvent.keyDown(document, { key: "Escape" });
  });
  expect(
    within(screen.getByLabelText("账号详情")).getByText(
      "独立授权 created-provider",
    ),
  ).toBeInTheDocument();
  expect(api.start).toHaveBeenCalledTimes(1);
  expect(api.openExternal).not.toHaveBeenCalled();
});

// A completion exit is not a substitute for retry/cancel while authorization is unresolved.
it.each(["pending", "failed"])(
  "does not offer completed exit for a %s authorization",
  async (status) => {
    api.poll.mockResolvedValue(
      status === "pending" ? { status } : { status, code: "unavailable" },
    );
    await setup();
    await startNew();
    await tick();
    expect(
      screen.queryByRole("button", { name: doneLabel }),
    ).not.toBeInTheDocument();
    const start = screen.getByRole("button", { name: startLabel });
    if (status === "pending") expect(start).toBeDisabled();
    else expect(start).toBeEnabled();
  },
);
