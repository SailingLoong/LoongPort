import {
  render,
  screen,
  fireEvent,
  waitFor,
  within,
} from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { ZCodeBundleImport } from "@/components/zcode/ZCodeBundleImport";
import { zcodeAccountsApi } from "@/lib/api/zcodeAccounts";
vi.mock("@/lib/api/zcodeAccounts", () => ({
  accountErrorText: {},
  safeAccountError: () => ({
    code: "zcode.account.operation_failed",
    remedy: "refreshContext",
    committed: false,
  }),
  zcodeAccountsApi: {
    previewBundle: vi.fn(),
    importBundle: vi.fn(),
    cancelBundle: vi.fn(),
  },
}));
const source = {
  installPath: "/synthetic/ZCode.app",
  dataRoot: "/synthetic/data",
  keyMode: "standard" as const,
};
function mount() {
  render(
    <ZCodeBundleImport
      source={source}
      contextRevision="context"
      catalogRevision="catalog"
      enabled
      onActiveChange={vi.fn()}
      onImported={vi.fn(async () => {})}
    />,
  );
}
async function preview() {
  fireEvent.change(screen.getByLabelText("Account bundle (.zsb)"), {
    target: { files: [new File([new Uint8Array([1, 2, 3])], "synthetic.zsb")] },
  });
  fireEvent.change(screen.getByLabelText("Bundle password"), {
    target: { value: "synthetic-password" },
  });
  fireEvent.click(
    screen.getByRole("button", { name: "Preview account bundle" }),
  );
  await screen.findByRole("dialog");
}
beforeEach(() => {
  vi.mocked(zcodeAccountsApi.previewBundle)
    .mockReset()
    .mockResolvedValue({
      previewId: "lease",
      rows: [
        {
          index: 0,
          id: "opaque",
          label: "a…",
          family: "zai",
          duplicate: true,
          ambiguous: false,
          error: null,
        },
      ],
    });
  vi.mocked(zcodeAccountsApi.importBundle)
    .mockReset()
    .mockResolvedValue(["kept"]);
  vi.mocked(zcodeAccountsApi.cancelBundle)
    .mockReset()
    .mockResolvedValue(undefined);
});
describe("ZCode bundle import", () => {
  it("shows a masked result for each selected account rather than only totals", async () => {
    vi.mocked(zcodeAccountsApi.previewBundle).mockResolvedValue({
      previewId: "lease",
      rows: ["a…", "b…", "c…"].map((label, index) => ({
        index,
        id: `opaque-${index}`,
        label,
        family: "zai" as const,
        duplicate: index > 0,
        ambiguous: false,
        error: null,
      })),
    });
    vi.mocked(zcodeAccountsApi.importBundle).mockResolvedValue([
      "saved",
      "refreshed",
      "kept",
    ]);
    mount();
    await preview();
    fireEvent.click(screen.getByLabelText("Update existing account 2"));
    fireEvent.click(
      screen.getByRole("button", {
        name: "Import selected into encrypted vault",
      }),
    );
    const results = await screen.findByRole("list", {
      name: "Last import results by account",
    });
    const rows = within(results).getAllByRole("listitem");
    expect(rows).toHaveLength(3);
    expect(rows[0]).toHaveTextContent("a…");
    expect(rows[0]).toHaveTextContent("Saved");
    expect(rows[1]).toHaveTextContent("b…");
    expect(rows[1]).toHaveTextContent("Updated");
    expect(rows[2]).toHaveTextContent("c…");
    expect(rows[2]).toHaveTextContent("Kept existing");
  });
  it("keeps a lost import response unconfirmed for every selected account", async () => {
    vi.mocked(zcodeAccountsApi.importBundle).mockRejectedValue(
      new Error("lost private response"),
    );
    mount();
    await preview();
    fireEvent.click(
      screen.getByRole("button", {
        name: "Import selected into encrypted vault",
      }),
    );
    const results = await screen.findByRole("list", {
      name: "Last import results by account",
    });
    expect(within(results).getByRole("listitem")).toHaveTextContent(
      "Unconfirmed — inspect saved accounts",
    );
    expect(
      screen.queryByText(/Imported into the encrypted vault:/),
    ).not.toBeInTheDocument();
    expect(screen.queryByText(/lost private/)).not.toBeInTheDocument();
  });
  it("preserves confirmed import when refreshing the list fails", async () => {
    render(
      <ZCodeBundleImport
        source={source}
        contextRevision="context"
        catalogRevision="catalog"
        enabled
        onActiveChange={vi.fn()}
        onImported={vi.fn(async () => {
          throw new Error("synthetic refresh failure");
        })}
      />,
    );
    await preview();
    fireEvent.click(
      screen.getByRole("button", {
        name: "Import selected into encrypted vault",
      }),
    );
    await screen.findByText(/Import completed, but the saved account list/);
    expect(screen.getByRole("status")).toHaveTextContent("1 kept");
    expect(zcodeAccountsApi.importBundle).toHaveBeenCalledTimes(1);
  });
  it("previews separately, clears password, and defaults duplicates to keep in vault only", async () => {
    mount();
    await preview();
    expect(screen.getByLabelText("Bundle password")).toHaveValue("");
    expect(zcodeAccountsApi.importBundle).not.toHaveBeenCalled();
    expect(
      screen.getByLabelText("Update existing account 1"),
    ).not.toBeChecked();
    fireEvent.click(
      screen.getByRole("button", {
        name: "Import selected into encrypted vault",
      }),
    );
    await waitFor(() =>
      expect(zcodeAccountsApi.importBundle).toHaveBeenCalledWith(
        source,
        "context",
        "catalog",
        "lease",
        [{ index: 0, updateDuplicate: false }],
      ),
    );
  });
  it("cancels the lease without importing", async () => {
    mount();
    await preview();
    fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
    await waitFor(() =>
      expect(zcodeAccountsApi.cancelBundle).toHaveBeenCalledWith("lease"),
    );
    expect(zcodeAccountsApi.importBundle).not.toHaveBeenCalled();
  });
  it("requires explicit choice for ambiguous identity and allows selecting the later record", async () => {
    vi.mocked(zcodeAccountsApi.previewBundle).mockResolvedValue({
      previewId: "lease",
      rows: [0, 1].map((index) => ({
        index,
        id: "same-opaque",
        label: "a…",
        family: "zai" as const,
        duplicate: false,
        ambiguous: true,
        error: null,
      })),
    });
    mount();
    await preview();
    expect(
      screen.getByRole("button", {
        name: "Import selected into encrypted vault",
      }),
    ).toBeDisabled();
    fireEvent.click(screen.getByLabelText("Select account 2"));
    fireEvent.click(
      screen.getByRole("button", {
        name: "Import selected into encrypted vault",
      }),
    );
    await waitFor(() =>
      expect(zcodeAccountsApi.importBundle).toHaveBeenCalledWith(
        source,
        "context",
        "catalog",
        "lease",
        [{ index: 1, updateDuplicate: false }],
      ),
    );
  });
});
