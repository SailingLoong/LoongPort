import { beforeEach, describe, expect, it, vi } from "vitest";
import { providersApi, type ProviderEditRequest } from "@/lib/api/providers";
import type { Provider } from "@/types";
const invoke = vi.hoisted(() => vi.fn());
vi.mock("@tauri-apps/api/core", () => ({ invoke }));
const provider: Provider = {
  id: "original",
  name: "Original",
  settingsConfig: {},
};
const request: ProviderEditRequest = {
  id: "00000000-0000-4000-8000-000000000001",
  providerId: "original",
  draftDigest: "a".repeat(64),
  revision: "b".repeat(64),
};
beforeEach(() => invoke.mockReset());
describe("original provider semantic key-write intent", () => {
  it.each([false, true])(
    "preview preserves explicit intent %s without a private owner revision",
    async (intent) => {
      await providersApi.previewEdit(
        provider,
        "codex",
        provider.id,
        request.id,
        false,
        undefined,
        intent,
      );
      const payload = invoke.mock.calls[0][1];
      expect(invoke.mock.calls[0][0]).toBe("preview_provider_edit");
      if (intent) expect(payload).toHaveProperty("credentialWrite", true);
      else expect(payload).not.toHaveProperty("credentialWrite");
      expect(payload).not.toHaveProperty("credentialRevision");
      expect(payload).not.toHaveProperty("credential_write");
    },
  );
  it.each([false, true])(
    "confirmation binds original intent %s, not client-selected incarnation",
    async (intent) => {
      await providersApi.confirmEdit(
        {
          provider,
          originalId: provider.id,
          edit: { request, deleteCredential: false, credentialWrite: intent },
        },
        "codex",
      );
      const payload = invoke.mock.calls[0][1];
      expect(invoke.mock.calls[0][0]).toBe("confirm_provider_edit");
      if (intent) expect(payload).toHaveProperty("credentialWrite", true);
      else expect(payload).not.toHaveProperty("credentialWrite");
      expect(payload.request).toBe(request);
      expect(payload).not.toHaveProperty("credentialRevision");
    },
  );
  it.each([false, true])(
    "legacy original writer receives semantic intent %s",
    async (intent) => {
      await providersApi.update(provider, "codex", provider.id, intent);
      const payload = invoke.mock.calls[0][1];
      expect(invoke.mock.calls[0][0]).toBe("update_provider");
      if (intent) expect(payload).toHaveProperty("credentialWrite", true);
      else expect(payload).not.toHaveProperty("credentialWrite");
    },
  );
});
