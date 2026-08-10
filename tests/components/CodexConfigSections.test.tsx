import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { parse as parseToml } from "smol-toml";
import { describe, expect, it, vi } from "vitest";
import { CodexConfigSection } from "@/components/providers/forms/CodexConfigSections";

vi.mock("@/components/JsonEditor", () => ({
  default: ({ value }: { value: string }) => (
    <textarea aria-label="config-editor" value={value} readOnly />
  ),
}));

function renderSection(value: string, onChange = vi.fn()) {
  render(
    <CodexConfigSection
      value={value}
      onChange={onChange}
      useCommonConfig={false}
      onCommonConfigToggle={() => {}}
      onEditCommonConfig={() => {}}
    />,
  );
  return onChange;
}

describe("CodexConfigSection million-token opt-in", () => {
  it("recognizes the legacy 1,000,000-token opt-in", () => {
    renderSection(`model_context_window = 1000000
model_auto_compact_token_limit = 900000`);

    expect(
      screen.getByRole("checkbox", {
        name: "codexConfig.contextWindow1M",
      }),
    ).toBeChecked();
  });

  it("is disabled by default and writes aligned top-level limits when enabled", async () => {
    const user = userEvent.setup();
    const onChange = renderSection(`model = "gpt-5.6-sol"

[model_providers.custom]
name = "custom"
base_url = "https://example.com/v1"`);

    const toggle = screen.getByRole("checkbox", {
      name: "codexConfig.contextWindow1M",
    });
    expect(toggle).not.toBeChecked();

    await user.click(toggle);

    expect(onChange).toHaveBeenCalledTimes(1);
    const parsed = parseToml(onChange.mock.calls[0][0]) as {
      model_context_window?: number;
      model_auto_compact_token_limit?: number;
      model_providers?: { custom?: Record<string, unknown> };
    };
    expect(parsed.model_context_window).toBe(1_050_000);
    expect(parsed.model_auto_compact_token_limit).toBe(900_000);
    expect(
      parsed.model_providers?.custom?.model_context_window,
    ).toBeUndefined();
    expect(
      parsed.model_providers?.custom?.model_auto_compact_token_limit,
    ).toBeUndefined();
  });
});
