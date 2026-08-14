import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { createElement } from "react";
import { beforeEach, describe, expect, it, vi } from "vitest";

const { listModelsDevEntries, importModelsDevPricing } = vi.hoisted(() => ({
  listModelsDevEntries: vi.fn(),
  importModelsDevPricing: vi.fn(),
}));

vi.mock("react-i18next", () => ({
  useTranslation: () => ({
    t: (key: string, fallback?: string) => fallback ?? key,
  }),
}));

vi.mock("sonner", () => ({
  toast: { success: vi.fn(), error: vi.fn() },
}));

vi.mock("@/lib/api/usage", () => ({
  usageApi: { listModelsDevEntries, importModelsDevPricing },
}));

import { ModelsDevPickerDialog } from "@/components/usage/ModelsDevPickerDialog";

const entry = {
  key: "openai/gpt-5",
  providerId: "openai",
  providerName: "OpenAI",
  modelId: "gpt-5",
  normalizedId: "gpt-5",
  modelName: "GPT-5",
  releaseDate: "2025-08-01",
  input: "1",
  output: "2",
  cacheRead: "0",
  cacheWrite: "0",
  isCommon: true,
};

function renderDialog() {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  return render(
    createElement(
      QueryClientProvider,
      { client },
      createElement(ModelsDevPickerDialog, {
        open: true,
        onClose: vi.fn(),
        onImported: vi.fn(),
      }),
    ),
  );
}

describe("ModelsDevPickerDialog", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    vi.stubGlobal("fetch", vi.fn());
    listModelsDevEntries.mockResolvedValue([entry]);
    importModelsDevPricing.mockResolvedValue(1);
  });

  it("imports the selected backend entry without fetching models.dev", async () => {
    renderDialog();

    fireEvent.click(await screen.findByRole("button", { name: /GPT-5/ }));
    fireEvent.click(screen.getByRole("button", { name: "导入" }));

    await waitFor(() =>
      expect(importModelsDevPricing).toHaveBeenCalledWith([entry]),
    );
    expect(fetch).not.toHaveBeenCalled();
  });
});
