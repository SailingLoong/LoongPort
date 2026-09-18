import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { ModelMismatchBanner } from "@/components/ModelMismatchBanner";
import {
  modelAlignmentApi,
  type ModelMismatch,
} from "@/lib/api/modelAlignment";
import { relayApi } from "@/lib/api/relay";

vi.mock("react-i18next", () => ({
  useTranslation: () => ({
    t: (key: string, params?: Record<string, string>) =>
      params ? `${key}:${Object.values(params).join(",")}` : key,
  }),
}));

vi.mock("sonner", () => ({ toast: { error: vi.fn() } }));

vi.mock("@/lib/api/modelAlignment", () => ({
  modelAlignmentApi: {
    list: vi.fn(),
    dismiss: vi.fn(),
  },
}));

vi.mock("@/lib/api/relay", () => ({
  relayApi: {
    switchTierModel: vi.fn(),
  },
}));

vi.mock("@/hooks/useTauriEvent", () => ({
  useTauriEvent: vi.fn(),
}));

const mismatch = (overrides: Partial<ModelMismatch> = {}): ModelMismatch => ({
  appType: "codex",
  providerId: "loongport-relay",
  providerName: "Example Relay",
  requestedModel: "gpt-6-astra",
  sentModel: "gpt-5.6-sol",
  canSwitchToRequested: true,
  ...overrides,
});

describe("ModelMismatchBanner", () => {
  it("renders nothing when there are no active mismatches", async () => {
    vi.mocked(modelAlignmentApi.list).mockResolvedValue([]);
    render(<ModelMismatchBanner />);
    await waitFor(() => expect(modelAlignmentApi.list).toHaveBeenCalled());
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
  });

  it("shows the mismatch and adopts the requested model through the tier switch", async () => {
    vi.mocked(modelAlignmentApi.list).mockResolvedValue([mismatch()]);
    vi.mocked(relayApi.switchTierModel).mockResolvedValue({
      status: "switched",
    } as never);
    vi.mocked(modelAlignmentApi.dismiss).mockResolvedValue();

    render(<ModelMismatchBanner />);

    const alert = await screen.findByRole("alert");
    expect(alert).toHaveTextContent(
      "modelMismatch.body:codex,Example Relay,gpt-6-astra,gpt-5.6-sol",
    );

    fireEvent.click(
      screen.getByRole("button", {
        name: "modelMismatch.adopt:gpt-6-astra",
      }),
    );
    await waitFor(() =>
      expect(relayApi.switchTierModel).toHaveBeenCalledWith(
        "loongport-relay",
        "codex",
        "gpt-6-astra",
      ),
    );
    await waitFor(() => expect(modelAlignmentApi.dismiss).toHaveBeenCalled());
  });

  it("keeps the tier model on dismiss without touching the switch", async () => {
    vi.mocked(modelAlignmentApi.list).mockResolvedValue([mismatch()]);
    vi.mocked(modelAlignmentApi.dismiss).mockClear();
    vi.mocked(modelAlignmentApi.dismiss).mockResolvedValue();

    render(<ModelMismatchBanner />);

    fireEvent.click(
      await screen.findByRole("button", {
        name: "modelMismatch.keep:gpt-5.6-sol",
      }),
    );
    await waitFor(() => expect(modelAlignmentApi.dismiss).toHaveBeenCalled());
    expect(relayApi.switchTierModel).not.toHaveBeenCalled();
  });

  it("disables adopting a model the tier does not offer", async () => {
    vi.mocked(modelAlignmentApi.list).mockResolvedValue([
      mismatch({ canSwitchToRequested: false }),
    ]);

    render(<ModelMismatchBanner />);

    const adopt = await screen.findByRole("button", {
      name: "modelMismatch.adopt:gpt-6-astra",
    });
    expect(adopt).toBeDisabled();
    expect(screen.getByText(/modelMismatch.notAvailable/)).toBeInTheDocument();
  });
});
