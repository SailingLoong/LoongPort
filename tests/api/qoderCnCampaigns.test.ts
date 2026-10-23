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

it.each(["chinaQoderPat", "chinaQoderDevice"] as const)(
  "claims %s only through the fixed backend entry and original owner declaration",
  async (source) => {
    const declaration = {
      providerId: "cn",
      credentialGeneration: "g1",
      source,
    };
    await qoderCnCampaignsApi.claim({
      ...declaration,
      token: "SYNTHETIC",
      campaignId: "injected",
    } as typeof declaration);
    expect(invoke).toHaveBeenCalledExactlyOnceWith("claim_qoder_cn_benefits", {
      declaration,
    });
  },
);
it("saves explicit CN consent with the backend policy revision and actual source", async () => {
  const context = {
    providerId: "cn",
    credentialGeneration: "g1",
    revision: 3,
    policyVersion: 2,
    enabled: false,
    canEnable: true,
    reason: null,
    subject: "synthetic",
  };
  await qoderCnCampaignsApi.setAutomatic(context, true, "chinaQoderDevice");
  expect(invoke).toHaveBeenCalledExactlyOnceWith("set_qoder_daily_automatic", {
    request: {
      providerId: "cn",
      credentialGeneration: "g1",
      expectedRevision: 3,
      policyVersion: 2,
      enabled: true,
      source: "chinaQoderDevice",
    },
  });
});
it("registers the real CN batch service command", () => {
  const commands = readFileSync("src-tauri/src/commands/provider.rs", "utf8");
  const registry = readFileSync("src-tauri/src/lib.rs", "utf8");
  expect(commands).toContain("async fn claim_qoder_cn_benefits");
  expect(commands).toContain("cn_benefits::run(&db, declaration, None).await");
  expect(registry).toContain("commands::claim_qoder_cn_benefits,");
});

it("keeps the database DAO private and uses only its narrow CN operation exports", () => {
  const database = readFileSync("src-tauri/src/database/mod.rs", "utf8");
  expect(database).toMatch(/^mod dao;/m);
  const exports =
    database.match(
      /pub\(crate\) use dao::qoder_campaign_operations::\{([^}]+)\};/g,
    ) ?? [];
  const boundary = exports.join(" ");
  for (const symbol of [
    "CnOperation",
    "CnTarget",
    "read_qoder_cn_operations_on",
    "reserve_qoder_cn_operation_on",
    "finish_qoder_cn_operation_on",
    "reconcile_qoder_cn_operation_on",
    "observe_qoder_cn_claimed_on",
  ]) {
    expect(boundary).toContain(symbol);
  }
  for (const path of [
    "cn_benefits.rs",
    "cn_benefits_contract.rs",
    "cn_benefits_tests.rs",
  ]) {
    const service = readFileSync(
      `src-tauri/src/services/provider/qoder_daily/${path}`,
      "utf8",
    );
    expect(service).not.toMatch(
      /(?:database::dao|dao::qoder_campaign_operations)/,
    );
  }
});
