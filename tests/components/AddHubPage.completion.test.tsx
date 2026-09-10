import { useState } from "react";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import { AddHubPage } from "@/components/relay/AddHubPage";
vi.mock("react-i18next", () => ({
  useTranslation: () => ({ t: (key: string) => key }),
}));
vi.mock("@/components/relay/directory/RelayDirectoryPage", () => ({
  RelayDirectoryPage: ({ domain, onDomainChange, onConnected }: any) => (
    <>
      <input
        aria-label="domain"
        value={domain}
        onChange={(e) => onDomainChange(e.target.value)}
      />
      <button
        onClick={() =>
          onConnected({ kind: "relay", rowId: 7, name: "Example" })
        }
      >
        Connect
      </button>
    </>
  ),
}));
vi.mock("@/components/relay/OfficialApiPage", () => ({
  OfficialApiPage: () => null,
}));
vi.mock("@/components/relay/onboarding/ServiceConfiguration", () => ({
  ServiceConfiguration: ({ onDone, onBack }: any) => (
    <>
      <button onClick={onDone}>Complete</button>
      <button onClick={onBack}>Return</button>
    </>
  ),
}));
vi.mock("@/components/providers/AddProviderForm", () => ({
  AddProviderForm: ({ onSubmit }: any) => {
    const [value, setValue] = useState("");
    return (
      <>
        <input
          aria-label="manual draft"
          value={value}
          onChange={(e) => setValue(e.target.value)}
        />
        <button
          onClick={() =>
            void Promise.resolve(onSubmit({ name: value })).catch(() => {})
          }
        >
          Save
        </button>
      </>
    );
  },
}));
describe("AddHub completion", () => {
  it("clears a completed connection while preserving a returned draft", async () => {
    const user = userEvent.setup();
    render(
      <AddHubPage
        sourceAppId="codex"
        onBack={vi.fn()}
        onAddProvider={vi.fn()}
      />,
    );
    await user.type(
      screen.getByRole("textbox", { name: "domain" }),
      "panel.example",
    );
    await user.click(screen.getByRole("button", { name: "Connect" }));
    await user.click(screen.getByRole("button", { name: "Return" }));
    expect(screen.getByRole("textbox", { name: "domain" })).toHaveValue(
      "panel.example",
    );
    await user.click(
      screen.getByRole("button", { name: "loongport.onboarding.resume" }),
    );
    await user.click(screen.getByRole("button", { name: "Complete" }));
    expect(
      screen.queryByRole("button", { name: "loongport.onboarding.resume" }),
    ).not.toBeInTheDocument();
    expect(screen.getByRole("textbox", { name: "domain" })).toHaveValue("");
  });
  it("resets manual input after success and preserves it after failure", async () => {
    const user = userEvent.setup();
    const submit = vi
      .fn()
      .mockRejectedValueOnce(new Error("save failed"))
      .mockResolvedValue(undefined);
    render(
      <AddHubPage
        sourceAppId="codex"
        initialTab="manual"
        onBack={vi.fn()}
        onAddProvider={submit}
      />,
    );
    await user.type(
      screen.getByRole("textbox", { name: "manual draft" }),
      "Draft service",
    );
    await user.click(screen.getByRole("button", { name: "Save" }));
    expect(screen.getByRole("textbox", { name: "manual draft" })).toHaveValue(
      "Draft service",
    );
    await user.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() =>
      expect(screen.getByRole("textbox", { name: "manual draft" })).toHaveValue(
        "",
      ),
    );
  });
});
