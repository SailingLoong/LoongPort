// Unexecuted regression specification. Uses the original V25 component/API;
// only Tauri commands are mocked. Backend ownership is covered by Rust tests.
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import { ManagedAccountBootstrapSection } from "@/components/relay/ManagedAccountBootstrapSection";
import type { Provider } from "@/types";
import type {
  ApplicationRouting,
  ApplicationRoutingRequest,
} from "@/lib/api/applicationRouting";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("react-i18next", () => ({
  useTranslation: () => ({
    t: (_: string, o: { defaultValue: string }) => o.defaultValue,
  }),
}));

const reference = (): Provider => ({
  id: "loongport-vendor-d45c7b937d154720",
  name: "Synthetic saved OpenCode Zen account",
  category: "cn_official",
  websiteUrl: "https://opencode.ai",
  settingsConfig: {
    auth: { OPENAI_API_KEY: "" },
    config:
      'model = "synthetic-model"\nmodel_provider = "synthetic"\n[model_providers.synthetic]\nbase_url = "https://opencode.ai/zen/v1"\n',
  },
  meta: {
    authBinding: {
      source: "vendor_api_key",
      authProvider: "opencode",
      accountId: "701",
    },
  },
});
const empty = (): ApplicationRouting => ({
  modeState: {
    status: "ready",
    mode: "direct",
    attached: false,
    currentProviderId: null,
    directProviderId: null,
    publicationStarted: null,
    canWrite: true,
    canRecheck: true,
  },
  autoFailoverEnabled: false,
  routingActive: false,
  model: null,
  modelOptions: [],
  chainIds: [],
  tiers: [],
});
const request: ApplicationRoutingRequest = {
  id: "synthetic-explicit-vendor-enter",
  scope: "applicationRouting",
  draftDigest: "a".repeat(64),
  revision: "b".repeat(64),
};
let rows: Record<string, Provider>;
let routing: ApplicationRouting;
let status: string;
let prepare: () => Promise<ApplicationRoutingRequest>;
const calls = (name: string) =>
  vi.mocked(invoke).mock.calls.filter(([command]) => command === name);
function mount() {
  return render(
    <QueryClientProvider
      client={
        new QueryClient({ defaultOptions: { queries: { retry: false } } })
      }
    >
      <ManagedAccountBootstrapSection appId="codex" />
    </QueryClientProvider>,
  );
}
const enable = () =>
  screen.getByRole("button", { name: "启用本地路由并使用此账号" });
async function chooseReference() {
  await screen.findByRole("option", { name: reference().name });
  fireEvent.change(screen.getByLabelText("已保存的账号档位"), {
    target: { value: reference().id },
  });
}
beforeEach(() => {
  vi.clearAllMocks();
  rows = { [reference().id]: reference() };
  routing = empty();
  status = "notRecorded";
  prepare = async () => request;
  vi.mocked(invoke).mockImplementation(async (command, args) => {
    const a = (args ?? {}) as Record<string, unknown>;
    if (command === "get_providers") return rows;
    if (command === "get_application_routing") return routing;
    if (command === "prepare_proxy_takeover_request") return prepare();
    if (command === "set_proxy_takeover_for_app") {
      const initial = a.initialRoute as { providerId: string };
      routing = {
        ...routing,
        routingActive: true,
        modeState: {
          ...routing.modeState!,
          mode: "proxy",
          attached: true,
          currentProviderId: initial.providerId,
        },
      };
      status = "completed";
      return;
    }
    if (command === "get_application_routing_request_result")
      return { request: a.request, status };
    throw new Error(`Unexpected command: ${command}`);
  });
});

describe("explicit saved OpenCode reference in the original bootstrap UI", () => {
  it("lists the saved reference without selecting, provisioning or enabling it", async () => {
    mount();
    await screen.findByRole("option", { name: reference().name });
    expect(screen.getByLabelText("已保存的账号档位")).toHaveValue("");
    expect(enable()).toBeDisabled();
    expect(calls("prepare_proxy_takeover_request")).toHaveLength(0);
    expect(calls("set_proxy_takeover_for_app")).toHaveLength(0);
    expect(
      vi
        .mocked(invoke)
        .mock.calls.every(([name]) =>
          ["get_providers", "get_application_routing"].includes(name),
        ),
    ).toBe(true);
  });
  it("forwards only the explicit saved identity and original reviewed request", async () => {
    mount();
    await chooseReference();
    fireEvent.click(enable());
    await waitFor(() =>
      expect(calls("set_proxy_takeover_for_app")).toHaveLength(1),
    );
    expect(calls("set_proxy_takeover_for_app")[0][1]).toEqual({
      appType: "codex",
      enabled: true,
      initialRoute: { providerId: reference().id, request },
    });
    await screen.findByText(/已启用本地路由/);
    expect(calls("prepare_proxy_takeover_request")).toHaveLength(1);
  });
  it("does not offer an unbound raw-key preset or incomplete vendor reference", async () => {
    const raw = reference();
    raw.id = "synthetic-raw-preset";
    raw.name = "Synthetic raw-key preset";
    raw.meta = undefined;
    raw.settingsConfig = { auth: { OPENAI_API_KEY: "SYNTHETIC_RAW_KEY" } };
    const missingOwner = reference();
    missingOwner.id = "synthetic-incomplete-reference";
    missingOwner.name = "Synthetic incomplete reference";
    missingOwner.meta!.authBinding!.accountId = "";
    rows = { [raw.id]: raw, [missingOwner.id]: missingOwner };
    mount();
    await waitFor(() =>
      expect(screen.getByLabelText("已保存的账号档位")).not.toBeDisabled(),
    );
    expect(screen.queryByRole("option", { name: raw.name })).toBeNull();
    expect(
      screen.queryByRole("option", { name: missingOwner.name }),
    ).toBeNull();
    expect(enable()).toBeDisabled();
    expect(calls("prepare_proxy_takeover_request")).toHaveLength(0);
  });
  it("does not apply if the backend refuses a missing key, locked vault or invalid owner", async () => {
    prepare = async () => {
      throw new Error("source.invalid_vendor_binding");
    };
    mount();
    await chooseReference();
    fireEvent.click(enable());
    await screen.findByText("准备失败，应用可能已变化。请刷新并重新选择。");
    expect(calls("set_proxy_takeover_for_app")).toHaveLength(0);
  });
  it("rechecks the original saved-row fingerprint before applying", async () => {
    prepare = async () => {
      const changed = reference();
      changed.meta!.authBinding!.accountId = "702";
      rows = { [changed.id]: changed };
      return request;
    };
    mount();
    await chooseReference();
    fireEvent.click(enable());
    await screen.findByText("准备失败，应用可能已变化。请刷新并重新选择。");
    expect(calls("set_proxy_takeover_for_app")).toHaveLength(0);
  });
});
