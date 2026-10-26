import "@testing-library/jest-dom/vitest";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import {
  cleanup,
  render,
  screen,
  waitFor,
  within,
} from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { invoke } from "@tauri-apps/api/core";
import { clearMocks } from "@tauri-apps/api/mocks";
import { installApplicationsC2Fixture } from "./applications-c2-fixture";
import type { SharedRoutingState } from "@/lib/api/sharedRouting";
import type { QueryClient } from "@tanstack/react-query";

const activeClients: QueryClient[] = [];
const networkAttempts: string[] = [];
let expectedNetworkAttempts: string[] = [];
let restoreBeacon = () => {};
function installDomNetworkBoundary() {
  const blocked = (kind: string): never => {
    networkAttempts.push(kind);
    throw new Error(`C2 DOM renderer network forbidden: ${kind}`);
  };
  vi.stubGlobal(
    "fetch",
    vi.fn(async () => blocked("fetch")),
  );
  vi.spyOn(XMLHttpRequest.prototype, "send").mockImplementation(() =>
    blocked("XMLHttpRequest"),
  );
  vi.stubGlobal(
    "WebSocket",
    class {
      constructor() {
        blocked("WebSocket");
      }
    },
  );
  vi.stubGlobal(
    "EventSource",
    class {
      constructor() {
        blocked("EventSource");
      }
    },
  );
  const originalBeacon = Object.getOwnPropertyDescriptor(
    navigator,
    "sendBeacon",
  );
  Object.defineProperty(navigator, "sendBeacon", {
    configurable: true,
    value: () => blocked("sendBeacon"),
  });
  restoreBeacon = () => {
    if (originalBeacon)
      Object.defineProperty(navigator, "sendBeacon", originalBeacon);
    else Reflect.deleteProperty(navigator, "sendBeacon");
  };
}

beforeEach(() => {
  localStorage.clear();
  sessionStorage.clear();
  networkAttempts.length = 0;
  expectedNetworkAttempts = [];
  installDomNetworkBoundary();
  Element.prototype.scrollIntoView = vi.fn();
  Object.defineProperty(window, "matchMedia", {
    configurable: true,
    value: (query: string) => ({
      matches: false,
      media: query,
      addEventListener() {},
      removeEventListener() {},
    }),
  });
});
afterEach(async () => {
  cleanup();
  for (const client of activeClients.splice(0)) client.clear();
  // App unmount schedules Promise-based Tauri unlisten cleanup. Drain it before
  // removing official mocks, otherwise cleanup itself creates false rejections.
  await new Promise((resolve) => setTimeout(resolve, 0));
  const observedNetworkAttempts = [...networkAttempts];
  restoreBeacon();
  clearMocks();
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
  expect(observedNetworkAttempts).toEqual(expectedNetworkAttempts);
});

describe("C2 browser-only native fixture", () => {
  it("blocks every DOM transport before it can create a network request", async () => {
    // Invalid URL / unopened XHR make this RED probe safe even before the guard
    // exists: neither original transport is allowed to attempt a real request.
    expect(() => new WebSocket("not-a-network-url")).toThrow(
      "C2 DOM renderer network forbidden: WebSocket",
    );
    expect(() => new XMLHttpRequest().send()).toThrow(
      "C2 DOM renderer network forbidden: XMLHttpRequest",
    );
    await expect(fetch("not-a-network-url")).rejects.toThrow(
      "C2 DOM renderer network forbidden: fetch",
    );
    expect(() => new EventSource("not-a-network-url")).toThrow(
      "C2 DOM renderer network forbidden: EventSource",
    );
    expect(() => navigator.sendBeacon("not-a-network-url")).toThrow(
      "C2 DOM renderer network forbidden: sendBeacon",
    );
    expectedNetworkAttempts = [
      "WebSocket",
      "XMLHttpRequest",
      "fetch",
      "EventSource",
      "sendBeacon",
    ];
    expect(networkAttempts).toEqual(expectedNetworkAttempts);
  });
  it("rejects unknown IPC, including unknown event commands, instead of default success", async () => {
    const fixture = installApplicationsC2Fixture();
    await expect(invoke("not_a_fixture_command")).rejects.toThrow(
      "Unexpected synthetic fixture command",
    );
    await expect(invoke("plugin:event|not_a_fixture_command")).rejects.toThrow(
      "Unexpected synthetic fixture command",
    );
    await expect(invoke("set_unknown_fixture_credential")).rejects.toThrow(
      "Unexpected synthetic fixture command",
    );
    await expect(invoke("list_unknown_fixture_accounts")).rejects.toThrow(
      "Unexpected synthetic fixture command",
    );
    expect(fixture.rejectedCommands).toEqual([
      "not_a_fixture_command",
      "plugin:event|not_a_fixture_command",
      "set_unknown_fixture_credential",
      "list_unknown_fixture_accounts",
    ]);
  });
  it("preserves saved manual identity and global member order without projected eligibility or trace", async () => {
    installApplicationsC2Fixture();
    const state = await invoke<SharedRoutingState>("get_shared_routing_state", {
      appType: "codex",
    });
    expect(state.binding!.target).toEqual({
      kind: "manual",
      sourceId: "synthetic-minimax",
      model: "model-a",
    });
    expect(state.groups[0].members).toEqual([
      { sourceId: "synthetic-qoder", model: "fixture-qoder-model" },
      { sourceId: "synthetic-minimax", model: "model-a" },
      { sourceId: "synthetic-workbuddy", model: "model-b" },
    ]);
    expect(Object.keys(state.groups[0].members[2]).sort()).toEqual([
      "model",
      "sourceId",
    ]);
    state.groups[0].members.reverse();
    expect(
      (
        await invoke<SharedRoutingState>("get_shared_routing_state", {
          appType: "codex",
        })
      ).groups[0].members[0].sourceId,
    ).toBe("synthetic-qoder");
  });
  it("requires the exact native prepared receipt before a synthetic apply", async () => {
    installApplicationsC2Fixture();
    await expect(
      invoke("apply_application_route_target", {
        appType: "codex",
        prepared: {
          target: { kind: "orderedGroup", groupId: "synthetic-daily" },
        },
      }),
    ).rejects.toThrow();
  });
});

async function mountActualApp(
  scenario: "normal" | "unknown" | "pending" = "normal",
) {
  const fixture = installApplicationsC2Fixture(scenario);
  const { ApplicationsC2App, createApplicationsC2Client } =
    await import("./applications-c2-app");
  const client = createApplicationsC2Client();
  activeClients.push(client);
  render(<ApplicationsC2App client={client} />);
  await screen.findByRole("heading", { name: "调整 Codex 的路由" });
  await waitFor(() =>
    expect(screen.getByRole("combobox", { name: "服务与账号" })).toHaveValue(
      "synthetic-minimax",
    ),
  );
  await waitFor(() => expect(client.isFetching()).toBe(0));
  // Query completion does not finish App's real framer-motion entry transition.
  // Wait for the rendered workspace instead of weakening visibility assertions.
  await waitFor(() =>
    expect(
      screen.getByRole("region", { name: "应用路由工作台" }),
    ).toBeVisible(),
  );
  expect(fixture.rejectedCommands).toEqual([]);
  expect(networkAttempts).toEqual([]);
  return { fixture, client, user: userEvent.setup() };
}

describe("actual App with official Tauri mocks, not a recreated design", () => {
  it("mounts four distinct facts, retains original add/hide semantics, and only applies after explicit review confirmation", async () => {
    const { fixture, client, user } = await mountActualApp();
    expect(screen.getByRole("button", { name: "Codex" })).toHaveAttribute(
      "aria-pressed",
      "true",
    );
    await waitFor(() =>
      expect(screen.getByText("配置指向本地网关")).toBeVisible(),
    );
    expect(screen.getByTestId("actual-request-fact")).toHaveTextContent(
      "未验证",
    );
    const beforeSources = await invoke("get_service_sources");
    await user.click(
      screen.getByRole("button", { name: "从主页面隐藏: Gemini" }),
    );
    await waitFor(() =>
      expect(
        screen.queryByRole("button", { name: "Gemini" }),
      ).not.toBeInTheDocument(),
    );
    await user.click(screen.getByRole("button", { name: "添加应用到主页面" }));
    await user.click(screen.getByRole("button", { name: "Gemini" }));
    await waitFor(() =>
      expect(
        screen.getByRole("button", { name: "Gemini" }),
      ).toBeInTheDocument(),
    );
    expect(await invoke("get_service_sources")).toEqual(beforeSources);
    await user.click(screen.getByRole("button", { name: "Codex" }));
    await waitFor(() =>
      expect(screen.getByRole("combobox", { name: "服务与账号" })).toHaveValue(
        "synthetic-minimax",
      ),
    );
    await user.click(screen.getByRole("radio", { name: "省心 · 按顺序" }));
    await user.selectOptions(
      screen.getByRole("combobox", { name: "共享组" }),
      "synthetic-daily",
    );
    expect(
      within(screen.getByTestId("shared-member-list")).getAllByText("待核验"),
    ).toHaveLength(3);
    expect(
      fixture.calls.filter(
        (c) =>
          c.command === "prepare_application_route_target" ||
          c.command === "apply_application_route_target",
      ),
    ).toHaveLength(0);
    await user.click(screen.getByRole("button", { name: "预览变更" }));
    const dialog = await screen.findByRole("dialog", {
      name: "确认这次路由变更",
    });
    expect(
      fixture.calls.filter(
        (c) => c.command === "apply_application_route_target",
      ),
    ).toHaveLength(0);
    await user.click(
      within(dialog).getByRole("button", { name: "确认保存此应用选择" }),
    );
    await screen.findByText("此应用选择已保存；真实请求仍未验证。");
    expect(
      fixture.calls.filter(
        (c) => c.command === "apply_application_route_target",
      ),
    ).toHaveLength(1);
    expect(screen.getByTestId("actual-request-fact")).toHaveTextContent(
      "未验证",
    );
    expect(fixture.rejectedCommands).toEqual([]);
    client.clear();
  });
  it("keeps an unknown outcome unsaved and only inspects without replaying", async () => {
    const { fixture, client, user } = await mountActualApp("unknown");
    await user.click(screen.getByRole("radio", { name: "省心 · 按顺序" }));
    await user.selectOptions(
      screen.getByRole("combobox", { name: "共享组" }),
      "synthetic-daily",
    );
    await user.click(screen.getByRole("button", { name: "预览变更" }));
    await user.click(
      await screen.findByRole("button", { name: "确认保存此应用选择" }),
    );
    await screen.findByText("原操作结果尚未确认。请检查原记录，不要重复提交。");
    expect(
      screen.queryByText("此应用选择已保存；真实请求仍未验证。"),
    ).not.toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "检查原配置操作" }));
    await waitFor(() => expect(client.isFetching()).toBe(0));
    expect(
      fixture.calls.filter(
        (c) => c.command === "apply_application_route_target",
      ),
    ).toHaveLength(1);
    expect(
      fixture.calls.filter(
        (c) => c.command === "get_application_route_target_result",
      ),
    ).toHaveLength(1);
    expect(
      fixture.calls.filter(
        (c) => c.command === "recover_application_route_target",
      ),
    ).toHaveLength(0);
    expect(fixture.rejectedCommands).toEqual([]);
    client.clear();
  });
  it("recovers only the original app-local pending configuration operation, never a model request", async () => {
    const { fixture, client, user } = await mountActualApp("pending");
    // Real native pending shape: shared state blocks new applies, while the
    // routing read rejects with its structured mode projection (not ready data).
    const pending = await invoke<SharedRoutingState>(
      "get_shared_routing_state",
      {
        appType: "codex",
      },
    );
    expect(pending.canApply).toBe(false);
    expect(pending.blockedReason).toBe("routing_groups.recovery_required");
    expect(pending.pendingRequest?.id).toBe("synthetic-original-pending");
    await expect(
      invoke("get_application_routing", { appType: "codex" }),
    ).rejects.toEqual({
      code: "mode.verification_required",
      modeState: {
        status: "pending",
        writeProtocol: null,
        blockedReason: "mode.verification_required",
        mode: null,
        attached: null,
        currentProviderId: null,
        directProviderId: null,
        publicationStarted: true,
        canWrite: false,
        canRecheck: true,
        legacyCommonConfigWritable: false,
      },
    });
    expect(screen.getByRole("radio", { name: "省心 · 按顺序" })).toBeDisabled();
    const recover = screen.getByRole("button", { name: "恢复原配置操作" });
    expect(recover).toBeEnabled();
    expect(
      fixture.calls.filter(
        (c) => c.command === "recover_application_route_target",
      ),
    ).toHaveLength(0);
    await user.click(recover);
    await screen.findByText("原操作未保存变更。草稿已保留，可检查后重新预览。");
    const recovered = await invoke<SharedRoutingState>(
      "get_shared_routing_state",
      {
        appType: "codex",
      },
    );
    expect(recovered).toMatchObject({
      canApply: true,
      blockedReason: null,
      pendingRequest: null,
    });
    expect(recovered.binding).toEqual(pending.binding);
    await expect(
      invoke("get_application_routing", { appType: "codex" }),
    ).resolves.toMatchObject({
      modeState: { status: "ready", canWrite: true, writeProtocol: "native" },
    });
    expect(
      fixture.calls.filter(
        (c) => c.command === "recover_application_route_target",
      ),
    ).toHaveLength(1);
    expect(
      fixture.calls.filter(
        (c) => c.command === "apply_application_route_target",
      ),
    ).toHaveLength(0);
    expect(screen.getByTestId("actual-request-fact")).toHaveTextContent(
      "未验证",
    );
    expect(fixture.rejectedCommands).toEqual([]);
    client.clear();
  });
});
