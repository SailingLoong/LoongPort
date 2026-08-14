import {
  QueryClient,
  QueryClientProvider,
  useQuery,
} from "@tanstack/react-query";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { createElement } from "react";
import { beforeEach, describe, expect, it, vi } from "vitest";

const { listModelsDevEntries, importModelsDevPricing, getModelPricing } =
  vi.hoisted(() => ({
    listModelsDevEntries: vi.fn(),
    importModelsDevPricing: vi.fn(),
    getModelPricing: vi.fn(),
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
  usageApi: { listModelsDevEntries, importModelsDevPricing, getModelPricing },
}));

import { ModelsDevPickerDialog } from "@/components/usage/ModelsDevPickerDialog";
import { usageApi } from "@/lib/api/usage";
import { usageKeys } from "@/lib/query/usage";

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

function PricingProbe() {
  const { data = [] } = useQuery({
    queryKey: usageKeys.pricing(),
    queryFn: usageApi.getModelPricing,
  });
  return createElement(
    "output",
    { "data-testid": "pricing-models" },
    data.map((model) => model.modelId).join(",") || "none",
  );
}

function renderDialog(includePricingProbe = false) {
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
      includePricingProbe ? createElement(PricingProbe) : null,
    ),
  );
}

describe("ModelsDevPickerDialog", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    vi.stubGlobal("fetch", vi.fn());
    listModelsDevEntries.mockResolvedValue([entry]);
    importModelsDevPricing.mockResolvedValue(1);
    getModelPricing.mockResolvedValue([]);
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

  it("refreshes usage pricing after importing a model", async () => {
    getModelPricing.mockResolvedValueOnce([]).mockResolvedValueOnce([
      {
        modelId: "gpt-5",
        displayName: "GPT-5",
        inputCostPerMillion: "1",
        outputCostPerMillion: "2",
        cacheReadCostPerMillion: "0",
        cacheCreationCostPerMillion: "0",
      },
    ]);
    renderDialog(true);

    expect(await screen.findByTestId("pricing-models")).toHaveTextContent(
      "none",
    );
    fireEvent.click(await screen.findByRole("button", { name: /GPT-5/ }));
    fireEvent.click(screen.getByRole("button", { name: "导入" }));

    await waitFor(() =>
      expect(screen.getByTestId("pricing-models")).toHaveTextContent("gpt-5"),
    );
  });
});
