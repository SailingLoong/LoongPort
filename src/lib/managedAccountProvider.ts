import type { Provider } from "@/types";

export type ManagedInferencePlatform =
  "minimax_code" | "zcode_start_plan" | "workbuddy" | "trae";
export type ManagedInferenceApp = "claude" | "codex";
export interface ManagedInferenceAccount {
  authProvider: ManagedInferencePlatform;
  id: string;
  label: string;
  available: boolean;
}

/** Original native catalog/protocol text bounds, not an entitlement check. */
function isDiscoveredModelText(
  value: string,
  nativeWhitespace = false,
): boolean {
  return (
    typeof value === "string" &&
    (nativeWhitespace ? /\P{White_Space}/u.test(value) : !!value.trim()) &&
    new TextEncoder().encode(value).length <= 256 &&
    !/[\u0000-\u001f\u007f-\u009f]/u.test(value)
  );
}

// Reuse byte/control bounds without changing TRAE's existing whitespace rule.
export const isTraeModelText = (value: string): boolean =>
  isDiscoveredModelText(value);
// Rust str::trim follows Unicode White_Space; unlike JS trim, U+FEFF is content.
export const isWorkBuddyModelText = (value: string): boolean =>
  isDiscoveredModelText(value, true);

/** Model-only application projection. Never accept a token or editable endpoint. */
export function buildManagedAccountProvider(
  account: ManagedInferenceAccount,
  app: ManagedInferenceApp,
  requestedModel: string,
  id: string,
): Provider {
  const model = ["trae", "workbuddy"].includes(account.authProvider)
    ? requestedModel
    : requestedModel.trim();
  if (
    !account.available ||
    !account.id.trim() ||
    !id.trim() ||
    !["minimax_code", "zcode_start_plan", "workbuddy", "trae"].includes(
      account.authProvider,
    ) ||
    !["claude", "codex"].includes(app) ||
    !(account.authProvider === "workbuddy"
      ? isWorkBuddyModelText(model)
      : account.authProvider === "trae"
        ? isTraeModelText(model)
        : /^[A-Za-z0-9._:/@+-]{1,200}$/.test(model))
  ) {
    throw new Error("managedAccount.invalidSelection");
  }
  return {
    id,
    name: `${account.label} · ${model}`,
    category: "custom",
    settingsConfig:
      app === "claude"
        ? {
            env: {
              ANTHROPIC_MODEL: model,
              ANTHROPIC_DEFAULT_SONNET_MODEL: model,
              ANTHROPIC_DEFAULT_OPUS_MODEL: model,
              ANTHROPIC_DEFAULT_HAIKU_MODEL: model,
            },
          }
        : { auth: {}, config: `model = ${JSON.stringify(model)}\n` },
    meta: {
      authBinding: {
        source: "managed_account",
        authProvider: account.authProvider,
        accountId: account.id,
      },
      apiFormat: ["workbuddy", "trae"].includes(account.authProvider)
        ? "openai_chat"
        : "anthropic",
      commonConfigEnabled: false,
    },
  };
}

/** Stable identity in the original Provider store; no separate account registry. */
export async function managedAccountProviderId(
  account: ManagedInferenceAccount,
  app: ManagedInferenceApp,
  model: string,
): Promise<string> {
  // Validate with the same projection before hashing. JSON preserves boundaries.
  buildManagedAccountProvider(account, app, model, "validation-only");
  const identity = JSON.stringify([
    "managed-inference-v1",
    app,
    account.authProvider,
    account.id,
    ["trae", "workbuddy"].includes(account.authProvider) ? model : model.trim(),
  ]);
  const digest = await crypto.subtle.digest(
    "SHA-256",
    new TextEncoder().encode(identity),
  );
  return `account-route-${Array.from(new Uint8Array(digest), (b) => b.toString(16).padStart(2, "0")).join("")}`;
}

function canonical(value: unknown): string {
  if (Array.isArray(value)) return `[${value.map(canonical).join(",")}]`;
  if (value !== null && typeof value === "object")
    return `{${Object.entries(value)
      .sort(([a], [b]) => a.localeCompare(b))
      .map(([k, v]) => `${JSON.stringify(k)}:${canonical(v)}`)
      .join(",")}}`;
  return JSON.stringify(value) ?? "undefined";
}

/** Verify actual identity/model, never turn an arbitrary save failure into success. */
export function sameManagedAccountProvider(
  actual: Provider,
  expected: Provider,
): boolean {
  const meta = actual.meta;
  const normalized = { ...meta };
  if (
    normalized.custom_endpoints &&
    Object.keys(normalized.custom_endpoints).length === 0
  )
    delete normalized.custom_endpoints;
  return (
    canonical(normalized) === canonical(expected.meta) &&
    actual.category === "custom" &&
    canonical(meta?.authBinding) === canonical(expected.meta?.authBinding) &&
    meta?.apiFormat === expected.meta?.apiFormat &&
    meta?.commonConfigEnabled === false &&
    !meta?.providerType &&
    !meta?.localProxyRequestOverrides &&
    !meta?.usage_script &&
    (!meta?.custom_endpoints ||
      Object.keys(meta.custom_endpoints).length === 0) &&
    canonical(actual.settingsConfig) === canonical(expected.settingsConfig)
  );
}
