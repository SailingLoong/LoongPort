import { beforeEach, describe, expect, it, vi } from "vitest";
import { minimaxCodeApi } from "@/lib/api/minimaxCode";

const invoke = vi.hoisted(() => vi.fn());
vi.mock("@tauri-apps/api/core", () => ({ invoke }));
beforeEach(() => {
  invoke.mockReset();
});

describe("MiniMax Code thin command boundary", () => {
  it("lists only the Code catalog with no vendor or API-key fallback", async () => {
    const projection = [{ id: "synthetic-code" }];
    invoke.mockResolvedValue(projection);
    expect(await minimaxCodeApi.list()).toBe(projection);
    expect(invoke.mock.calls).toEqual([["list_minimax_code_accounts"]]);
  });

  it.each([
    ["refresh", "refresh_minimax_code_account"],
    ["claim", "claim_minimax_code_today"],
    ["recheck", "recheck_minimax_code_claim"],
    ["retire", "retire_minimax_code_account"],
  ] as const)(
    "binds %s to the exact account id without replay",
    async (method, command) => {
      invoke.mockRejectedValue("networkUnavailable");
      await expect(minimaxCodeApi[method]("code-account")).rejects.toBe(
        "networkUnavailable",
      );
      expect(invoke.mock.calls).toEqual([[command, { id: "code-account" }]]);
    },
  );

  it.each(["cn", "global"] as const)(
    "starts an explicit %s device flow without receiving credentials",
    async (region) => {
      await minimaxCodeApi.beginLogin(region);
      expect(invoke.mock.calls).toEqual([
        ["begin_minimax_code_authorization", { region }],
      ]);
    },
  );

  it.each([
    ["pollLogin", "poll_minimax_code_authorization"],
    ["cancelLogin", "cancel_minimax_code_authorization"],
  ] as const)(
    "binds %s to the backend-owned flow id",
    async (method, command) => {
      await minimaxCodeApi[method]("synthetic-flow");
      expect(invoke.mock.calls).toEqual([
        [command, { flowId: "synthetic-flow" }],
      ]);
    },
  );
});
