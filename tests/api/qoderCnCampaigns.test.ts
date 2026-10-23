import { beforeEach, expect, it, vi } from "vitest";
import { readFileSync } from "node:fs";
import { qoderCnCampaignsApi } from "@/lib/api/qoderCnCampaigns";
const invoke = vi.hoisted(() => vi.fn());
vi.mock("@tauri-apps/api/core", () => ({ invoke }));
beforeEach(() =>
  invoke
    .mockReset()
    .mockResolvedValue({ status: "failed", code: "identity_required" }),
);
it.each(["chinaQoderPat", "chinaQoderDevice"] as const)(
  "queries %s through the real command with only original owner binding",
  async (source) => {
    const declaration = {
      providerId: "cn-synthetic",
      credentialGeneration: "generation",
      source,
    };
    const contaminated = {
      ...declaration,
      token: "SYNTHETIC-SECRET",
      origin: "https://example.invalid",
      uid: "SYNTHETIC-UID",
    };
    await qoderCnCampaignsApi.read(contaminated);
    expect(invoke).toHaveBeenCalledExactlyOnceWith("read_qoder_cn_campaigns", {
      declaration,
    });
    expect(JSON.stringify(invoke.mock.calls)).not.toContain("SYNTHETIC-");
  },
);

it("keeps backend failure codes and the presentation contract exhaustive", () => {
  const rust = readFileSync(
    "src-tauri/src/services/provider/qoder_daily/cn_campaigns.rs",
    "utf8",
  );
  const ts = readFileSync("src/lib/api/qoderCnCampaigns.ts", "utf8");
  const variants = rust.match(/enum FailureCode\s*\{([^}]+)\}/)?.[1];
  const union = ts.match(/type QoderCnCampaignFailure\s*=([^;]+);/)?.[1];
  expect(variants).toBeDefined();
  expect(union).toBeDefined();
  const codes = variants!
    .split(",")
    .map((v) => v.trim())
    .filter(Boolean)
    .map((v) =>
      v.replace(/[A-Z]/g, (c, i) => (i ? "_" : "") + c.toLowerCase()),
    );
  expect([...union!.matchAll(/"([a-z_]+)"/g)].map((m) => m[1]).sort()).toEqual(
    codes.sort(),
  );
});

it("uses an actually registered Tauri command, not a frontend-only placeholder", () => {
  const commands = readFileSync("src-tauri/src/commands/provider.rs", "utf8");
  const registry = readFileSync("src-tauri/src/lib.rs", "utf8");
  expect(commands).toContain("async fn read_qoder_cn_campaigns");
  expect(commands).toContain("cn_campaigns::read(&db, declaration).await");
  expect(registry).toContain("commands::read_qoder_cn_campaigns,");
});
