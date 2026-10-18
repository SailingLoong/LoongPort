import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { QueryClientProvider } from "@tanstack/react-query";
import { expect, it, vi } from "vitest";
import { ProviderForm } from "@/components/providers/forms/ProviderForm";
import { createTestQueryClient } from "../utils/testQueryClient";
import { codexProviderPresets } from "@/config/codexProviderPresets";
it("preserves explicit Qoder type in the actual ordinary Provider form save", async () => {
  const submit = vi.fn();
  render(
    <QueryClientProvider client={createTestQueryClient()}>
      <ProviderForm
        appId="codex"
        submitLabel="save-qoder-fixture"
        onSubmit={submit}
        onCancel={vi.fn()}
        initialData={{
          name: "Synthetic Qoder",
          category: "third_party",
          meta: { providerType: "qoder_sdk" },
          settingsConfig: {
            auth: { OPENAI_API_KEY: "SYNTHETIC-NONSECRET" },
            config:
              'model_provider = "custom"\nmodel = "fixture-model"\n[model_providers.custom]\nname = "Fixture"\nbase_url = "https://openapi.qoder.sh"\nwire_api = "responses"',
          },
        }}
      />
    </QueryClientProvider>,
  );
  fireEvent.click(screen.getByRole("button", { name: "save-qoder-fixture" }));
  await waitFor(() => expect(submit).toHaveBeenCalledTimes(1));
  expect(submit.mock.calls[0][0].meta.providerType).toBe("qoder_sdk");
  expect(submit.mock.calls[0][0].meta.qoderCampaignState).toBeUndefined();
});
it("offers explicit Qoder classification without inventing a supported model or PAT proof", () => {
  const preset = codexProviderPresets.find(
    (p) => p.providerType === "qoder_sdk",
  );
  expect(preset).toBeDefined();
  expect(preset!.auth.OPENAI_API_KEY).toBe("");
  expect(preset!.config).toContain('model = ""');
  expect(preset!.requiresOAuth).not.toBe(true);
});
