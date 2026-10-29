import {
  act,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@testing-library/react";
import {
  QueryClient,
  QueryClientProvider,
  useQuery,
} from "@tanstack/react-query";
import { describe, expect, it, vi } from "vitest";
import { QoderInferenceIdentity } from "@/components/providers/qoder/QoderInferenceIdentity";
import type {
  QoderHttpIdentityContext,
  QoderHttpIdentityResult,
} from "@/lib/api/qoderHttpAuth";

const api = vi.hoisted(() => ({ context: vi.fn(), action: vi.fn() }));
vi.mock("@/lib/api/qoderHttpAuth", () => ({ qoderHttpAuthApi: api }));
const initial: QoderHttpIdentityContext = {
  source: "internationalQoderPat",
  providerId: "fixture",
  credentialGeneration: "revision-a",
  status: "unestablished",
  expiresAtMs: null,
  allowedActions: ["establish"],
};
const ready: QoderHttpIdentityContext = {
  ...initial,
  status: "ready",
  expiresAtMs: 1900000000000,
  allowedActions: ["rebind"],
};
const labels = {
  establish: "声明对应区域 PAT 并建立",
  recover: "恢复推理身份",
  rebind: "重新绑定账号",
};
function setup(context = initial) {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false, staleTime: Infinity } },
  });
  client.setQueryData(["applicationOverview", "codex"], { configurations: [] });
  client.setQueryData(["applicationOverview", "claude"], {
    configurations: [],
  });
  client.setQueryData(["serviceSources"], []);
  client.setQueryData(
    ["serviceSources", "qoderApplicationEntry", "fixture", "revision-a"],
    [],
  );
  api.context.mockReset().mockResolvedValue(context);
  api.action
    .mockReset()
    .mockResolvedValue({ status: "completed", context: ready });
  const view = render(
    <QueryClientProvider client={client}>
      <QoderInferenceIdentity
        providerId="fixture"
        sourceGeneration="revision-a"
      />
    </QueryClientProvider>,
  );
  return { client, ...view };
}
async function submit(action: keyof typeof labels) {
  fireEvent.click(screen.getByRole("button", { name: "Qoder 推理身份" }));
  fireEvent.click(await screen.findByRole("checkbox"));
  const button = screen.getByRole("button", { name: labels[action] });
  await waitFor(() => expect(button).toBeEnabled());
  fireEvent.click(button);
}
function invalidated(client: QueryClient, key: unknown[]) {
  return client.getQueryState(key)?.isInvalidated;
}

describe("Qoder identity consumer refresh", () => {
  it.each(["establish", "recover", "rebind"] as const)(
    "invalidates the original Codex overview and source queries after successful %s",
    async (action) => {
      const context: QoderHttpIdentityContext =
        action === "establish"
          ? initial
          : {
              ...initial,
              status: action === "recover" ? "unknown" : "ready",
              allowedActions: [action],
            };
      const { client } = setup(context);
      await submit(action);
      await screen.findByText("已建立推理身份，可供后续请求复用。");
      expect(invalidated(client, ["applicationOverview", "codex"])).toBe(true);
      expect(invalidated(client, ["serviceSources"])).toBe(true);
      expect(
        invalidated(client, [
          "serviceSources",
          "qoderApplicationEntry",
          "fixture",
          "revision-a",
        ]),
      ).toBe(true);
      expect(invalidated(client, ["applicationOverview", "claude"])).toBe(
        false,
      );
      expect(api.action).toHaveBeenCalledExactlyOnceWith(context, action);
    },
  );
  it.each(["failed", "cancelled", "changed", "wrong-provider"] as const)(
    "does not claim or invalidate on %s",
    async (status) => {
      const { client } = setup();
      api.action.mockResolvedValue(
        status === "failed"
          ? { status, code: "invalid_source" }
          : status === "cancelled"
            ? { status }
            : status === "changed"
              ? { status, context: initial }
              : {
                  status: "completed",
                  context: { ...ready, providerId: "other" },
                },
      );
      await submit("establish");
      await waitFor(() => expect(api.action).toHaveBeenCalledTimes(1));
      if (status === "cancelled")
        await screen.findByText("已取消，未提交新的身份建立请求。");
      else if (status === "changed")
        await screen.findByText("状态已变化，请根据当前状态重新选择操作。");
      else await screen.findByRole("alert");
      expect(invalidated(client, ["applicationOverview", "codex"])).toBe(false);
      expect(invalidated(client, ["serviceSources"])).toBe(false);
      expect(
        screen.queryByText("已建立推理身份，可供后续请求复用。"),
      ).not.toBeInTheDocument();
    },
  );
  it("refreshes authoritative queries after a late completion without showing its old context in the new Provider", async () => {
    let resolve!: (result: QoderHttpIdentityResult) => void;
    const { client, rerender } = setup();
    api.action.mockReturnValue(
      new Promise<QoderHttpIdentityResult>((done) => {
        resolve = done;
      }),
    );
    await submit("establish");
    api.context.mockResolvedValue({
      ...initial,
      providerId: "new-provider",
      credentialGeneration: "revision-b",
    });
    rerender(
      <QueryClientProvider client={client}>
        <QoderInferenceIdentity
          providerId="new-provider"
          sourceGeneration="revision-b"
        />
      </QueryClientProvider>,
    );
    await waitFor(() =>
      expect(api.context).toHaveBeenCalledWith("new-provider"),
    );
    await act(async () => resolve({ status: "completed", context: ready }));
    expect(invalidated(client, ["applicationOverview", "codex"])).toBe(true);
    expect(invalidated(client, ["serviceSources"])).toBe(true);
    expect(
      screen.queryByText("已建立推理身份，可供后续请求复用。"),
    ).not.toBeInTheDocument();
    expect(screen.getByText("尚未建立推理身份。")).toBeInTheDocument();
    expect(api.action).toHaveBeenCalledTimes(1);
  });
  it("refetches mounted original consumers from their backend queries", async () => {
    let backendVersion = "old";
    const client = new QueryClient({
      defaultOptions: { queries: { retry: false, staleTime: Infinity } },
    });
    client.setQueryData(["applicationOverview", "codex"], "old");
    client.setQueryData(["serviceSources"], "old");
    api.context.mockReset().mockResolvedValue(initial);
    api.action.mockReset().mockImplementation(async () => {
      backendVersion = "new";
      return { status: "completed", context: ready };
    });
    const overview = vi.fn(async () => backendVersion);
    const sources = vi.fn(async () => backendVersion);
    function Consumers() {
      const app = useQuery({
        queryKey: ["applicationOverview", "codex"],
        queryFn: overview,
      });
      const source = useQuery({
        queryKey: ["serviceSources"],
        queryFn: sources,
      });
      return (
        <p data-testid="consumer-state">
          {app.data}/{source.data}
        </p>
      );
    }
    render(
      <QueryClientProvider client={client}>
        <Consumers />
        <QoderInferenceIdentity providerId="fixture" />
      </QueryClientProvider>,
    );
    expect(screen.getByTestId("consumer-state")).toHaveTextContent("old/old");
    await submit("establish");
    await waitFor(() =>
      expect(screen.getByTestId("consumer-state")).toHaveTextContent("new/new"),
    );
    expect(overview).toHaveBeenCalledTimes(1);
    expect(sources).toHaveBeenCalledTimes(1);
  });

  it("coalesces repeated clicks and invalidates each consumer key once on completion", async () => {
    const { client } = setup();
    let resolve!: (result: QoderHttpIdentityResult) => void;
    const invalidations = vi.spyOn(client, "invalidateQueries");
    api.action.mockReturnValue(
      new Promise<QoderHttpIdentityResult>((done) => {
        resolve = done;
      }),
    );
    await submit("establish");
    fireEvent.click(screen.getByRole("button", { name: labels.establish }));
    expect(api.action).toHaveBeenCalledTimes(1);
    await act(async () => resolve({ status: "completed", context: ready }));
    await screen.findByText("已建立推理身份，可供后续请求复用。");
    expect(invalidations).toHaveBeenCalledTimes(2);
    expect(invalidations).toHaveBeenCalledWith({
      queryKey: ["applicationOverview", "codex"],
    });
    expect(invalidations).toHaveBeenCalledWith({
      queryKey: ["serviceSources"],
    });
  });

  it("keeps a replacement source context intact after the previous generation completes", async () => {
    let resolve!: (result: QoderHttpIdentityResult) => void;
    const { client, rerender } = setup();
    api.action.mockReturnValue(
      new Promise<QoderHttpIdentityResult>((done) => {
        resolve = done;
      }),
    );
    await submit("establish");
    api.context.mockResolvedValue({
      ...initial,
      credentialGeneration: "revision-b",
    });
    rerender(
      <QueryClientProvider client={client}>
        <QoderInferenceIdentity
          providerId="fixture"
          sourceGeneration="revision-b"
        />
      </QueryClientProvider>,
    );
    await waitFor(() => expect(api.context).toHaveBeenCalledTimes(2));
    await act(async () => resolve({ status: "completed", context: ready }));
    expect(invalidated(client, ["applicationOverview", "codex"])).toBe(true);
    expect(invalidated(client, ["serviceSources"])).toBe(true);
    expect(screen.getByText("尚未建立推理身份。")).toBeInTheDocument();
    expect(
      screen.queryByText("已建立推理身份，可供后续请求复用。"),
    ).not.toBeInTheDocument();
  });
});
