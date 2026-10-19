import { parse as parseToml } from "smol-toml";
import { describe, expect, it } from "vitest";
import {
  buildManagedAccountProvider,
  managedAccountProviderId,
  sameManagedAccountProvider,
} from "@/lib/managedAccountProvider";

const account = {
  authProvider: "minimax_code" as const,
  id: "opaque-one",
  label: "Synthetic account",
  available: true,
};

describe("managed account route provider projection", () => {
  it.each(["claude", "codex"] as const)(
    "projects only identity and model for %s",
    (app) => {
      const provider = buildManagedAccountProvider(
        account,
        app,
        "MiniMax-M2",
        "route-one",
      );
      expect(provider.meta?.authBinding).toEqual({
        source: "managed_account",
        authProvider: "minimax_code",
        accountId: "opaque-one",
      });
      expect(provider.meta?.apiFormat).toBe("anthropic");
      expect(provider.meta?.commonConfigEnabled).toBe(false);
      expect(JSON.stringify(provider)).not.toMatch(
        /apiKey|AUTH_TOKEN|base_url|BASE_URL|accessToken|credentialVersion|realUserId/,
      );
      if (app === "codex") {
        expect(provider.settingsConfig.auth).toEqual({});
        expect(provider.settingsConfig.config).toBe('model = "MiniMax-M2"\n');
      } else
        expect(provider.settingsConfig.env.ANTHROPIC_MODEL).toBe("MiniMax-M2");
    },
  );
  it("keeps ZCode identity distinct even when opaque ids match", () => {
    const p = buildManagedAccountProvider(
      { ...account, authProvider: "zcode_start_plan" },
      "claude",
      "glm-4.6",
      "route-two",
    );
    expect(p.meta?.authBinding?.authProvider).toBe("zcode_start_plan");
  });
  it.each(["", "   ", 'model\napi_key="secret"', "a b"])(
    "rejects invalid model %j",
    (model) => {
      expect(() =>
        buildManagedAccountProvider(account, "claude", model, "route-one"),
      ).toThrow();
    },
  );
  it("requires an available, explicitly selected account", () => {
    expect(() =>
      buildManagedAccountProvider(
        { ...account, available: false },
        "claude",
        "model",
        "route-one",
      ),
    ).toThrow();
    expect(() =>
      buildManagedAccountProvider(
        { ...account, id: "" },
        "claude",
        "model",
        "route-one",
      ),
    ).toThrow();
  });
  it("binds stable ids to platform, opaque account, application and normalized model", async () => {
    const original = await managedAccountProviderId(account, "claude", "model");
    expect(
      await managedAccountProviderId(
        { ...account, label: "Renamed" },
        "claude",
        " model ",
      ),
    ).toBe(original);
    for (const [a, app, model] of [
      [
        { ...account, authProvider: "zcode_start_plan" as const },
        "claude",
        "model",
      ],
      [{ ...account, id: "other" }, "claude", "model"],
      [account, "codex", "model"],
      [account, "claude", "other-model"],
    ] as const)
      expect(await managedAccountProviderId(a, app, model)).not.toBe(original);
  });
  it("compares the exact fixed projection, not a name or arbitrary matching id", () => {
    const expected = buildManagedAccountProvider(
      account,
      "claude",
      "model",
      "id",
    );
    expect(
      sameManagedAccountProvider({ ...expected, name: "Renamed" }, expected),
    ).toBe(true);
    expect(
      sameManagedAccountProvider(
        buildManagedAccountProvider(account, "claude", "other", "id"),
        expected,
      ),
    ).toBe(false);
    expect(
      sameManagedAccountProvider(
        {
          ...expected,
          meta: {
            ...expected.meta,
            authBinding: { source: "provider_config" },
          },
        },
        expected,
      ),
    ).toBe(false);
  });
  it("rejects real DTO credential, endpoint and request-override metadata", () => {
    const expected = buildManagedAccountProvider(
      account,
      "claude",
      "model",
      "id",
    );
    for (const metadata of [
      {
        usage_script: {
          enabled: false,
          language: "javascript" as const,
          code: "synthetic",
        },
      },
      {
        custom_endpoints: {
          "https://other.invalid": { url: "https://other.invalid", addedAt: 1 },
        },
      },
      { localProxyRequestOverrides: { body: { model: "other-model" } } },
      { localProxyRequestOverrides: {} },
      { maxOutputTokens: 123 },
      { codexChatReasoning: { supportsThinking: true } },
    ])
      expect(
        sameManagedAccountProvider(
          { ...expected, meta: { ...expected.meta, ...metadata } },
          expected,
        ),
      ).toBe(false);
  });
});

it.each(["claude", "codex"] as const)(
  "projects WorkBuddy as fixed Chat transport with distinct same-id identity for %s",
  async (app) => {
    const wb = { ...account, authProvider: "workbuddy" as const };
    const id = await managedAccountProviderId(wb, app, "live-model");
    const provider = buildManagedAccountProvider(wb, app, "live-model", id);
    expect(provider.meta?.apiFormat).toBe("openai_chat");
    expect(provider.meta?.authBinding).toEqual({
      source: "managed_account",
      authProvider: "workbuddy",
      accountId: account.id,
    });
    expect(id).not.toBe(
      await managedAccountProviderId(account, app, "live-model"),
    );
    expect(JSON.stringify(provider)).not.toMatch(
      /token|Token|BASE_URL|base_url/,
    );
  },
);

describe("TRAE fixed text-only route projection", () => {
  it.each(["claude", "codex"] as const)(
    "uses opaque TRAE identity and fixed Chat for %s",
    async (app) => {
      const trae = { ...account, authProvider: "trae" as const };
      const id = await managedAccountProviderId(
        trae,
        app,
        "explicit-global-model",
      );
      const p = buildManagedAccountProvider(
        trae,
        app,
        "explicit-global-model",
        id,
      );
      expect(p.meta).toEqual({
        authBinding: {
          source: "managed_account",
          authProvider: "trae",
          accountId: account.id,
        },
        apiFormat: "openai_chat",
        commonConfigEnabled: false,
      });
      expect(id).not.toBe(
        await managedAccountProviderId(account, app, "explicit-global-model"),
      );
      expect(JSON.stringify(p)).not.toMatch(
        /base_url|BASE_URL|accessToken|refreshToken|apiKey|authHost|realm|inference_verified|discovered/,
      );
      expect(() => buildManagedAccountProvider(trae, app, "", "id")).toThrow();
    },
  );
});

it.each(["claude", "codex"] as const)(
  "preserves exact TRAE discovered text IDs for %s without changing other platform validation",
  async (app) => {
    const a = { ...account, authProvider: "trae" as const };
    const model = ' 模型 "Global" / v1 ';
    const id = await managedAccountProviderId(a, app, model);
    const p = buildManagedAccountProvider(a, app, model, id);
    if (app === "codex") {
      expect(p.settingsConfig.config).toBe(
        `model = ${JSON.stringify(model)}\n`,
      );
      expect(parseToml(p.settingsConfig.config as string).model).toBe(model);
    } else expect(p.settingsConfig.env.ANTHROPIC_MODEL).toBe(model);
    expect(id).not.toBe(await managedAccountProviderId(a, app, model.trim()));
    expect(() =>
      buildManagedAccountProvider(account, app, model, "id"),
    ).toThrow();
  },
);
it.each(["", "  ", "x\n", "x\u007f", "x\u0085", "界".repeat(86)])(
  "rejects unsafe TRAE discovered model text %j",
  (model) => {
    expect(() =>
      buildManagedAccountProvider(
        { ...account, authProvider: "trae" },
        "codex",
        model,
        "id",
      ),
    ).toThrow();
  },
);
