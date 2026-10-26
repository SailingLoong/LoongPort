import { render, screen, fireEvent } from "@testing-library/react";
import { describe, it, expect, vi, beforeEach } from "vitest";
import { ImportExportSection } from "@/components/settings/ImportExportSection";

const tMock = vi.fn((key: string) => key);

vi.mock("react-i18next", () => ({
  useTranslation: () => ({ t: tMock }),
}));

describe("ImportExportSection Component", () => {
  const baseProps = {
    status: "idle" as const,
    selectedFile: "",
    errorMessage: null,
    backupId: null,
    isImporting: false,
    onSelectFile: vi.fn(),
    onImport: vi.fn(),
    onExport: vi.fn(),
    onClear: vi.fn(),
  };

  beforeEach(() => {
    tMock.mockImplementation((key: string) => key);
    baseProps.onSelectFile.mockReset();
    baseProps.onImport.mockReset();
    baseProps.onExport.mockReset();
    baseProps.onClear.mockReset();
  });

  it("passes literal mixed-line-ending source input and clears before import resolves", async () => {
    let finish!: () => void;
    baseProps.onImport.mockImplementation(
      () =>
        new Promise<void>((resolve) => {
          finish = resolve;
        }),
    );
    render(
      <ImportExportSection {...baseProps} selectedFile="/tmp/synthetic.sql" />,
    );
    const field = screen.getByLabelText("portableBackup.password");
    const raw = "\r\n中文🔑 aB\rline\n ";
    fireEvent.paste(field, { clipboardData: { getData: () => raw } });
    fireEvent.click(screen.getByRole("button", { name: /settings\.import/ }));
    fireEvent.click(screen.getByRole("button", { name: /settings\.import/ }));
    expect(baseProps.onImport).toHaveBeenCalledTimes(1);
    expect(baseProps.onImport).toHaveBeenCalledWith(raw);
    expect(field).toHaveValue("");
    finish();
  });

  it("clears the original source text on file switch, clear selection and unmount", () => {
    const view = render(
      <ImportExportSection {...baseProps} selectedFile="/tmp/one.sql" />,
    );
    const field = screen.getByLabelText("portableBackup.password");
    fireEvent.paste(field, { clipboardData: { getData: () => "one\r\nkey" } });
    view.rerender(
      <ImportExportSection {...baseProps} selectedFile="/tmp/two.sql" />,
    );
    expect(field).toHaveValue("");
    fireEvent.paste(field, { clipboardData: { getData: () => "two\rkey" } });
    fireEvent.click(screen.getByRole("button", { name: "common.clear" }));
    expect(field).toHaveValue("");
    fireEvent.paste(field, { clipboardData: { getData: () => "three\nkey" } });
    view.unmount();
    expect(field).toHaveValue("");
  });

  it("passes the backup password on deliberate import and clears it after the attempt", async () => {
    baseProps.onImport.mockResolvedValue(undefined);
    render(
      <ImportExportSection {...baseProps} selectedFile="/tmp/portable.sql" />,
    );
    const password = screen.getByLabelText("portableBackup.password");
    fireEvent.change(password, {
      target: { value: "backup recovery password" },
    });
    fireEvent.click(screen.getByRole("button", { name: /settings\.import/ }));
    expect(baseProps.onImport).toHaveBeenCalledWith("backup recovery password");
    await vi.waitFor(() => expect(password).toHaveValue(""));
  });

  it("should disable import button and show placeholder when no file selected", () => {
    render(<ImportExportSection {...baseProps} />);

    // When no file selected, button shows "selectConfigFile" and clicking it opens file dialog
    expect(
      screen.getByRole("button", { name: /settings\.selectConfigFile/ }),
    ).toBeInTheDocument();
    fireEvent.click(
      screen.getByRole("button", { name: "settings.exportConfig" }),
    );
    expect(baseProps.onExport).toHaveBeenCalledTimes(1);

    fireEvent.click(
      screen.getByRole("button", { name: /settings\.selectConfigFile/ }),
    );
    expect(baseProps.onSelectFile).toHaveBeenCalledTimes(1);
  });

  it("should show filename and enable import/clear when file is selected", () => {
    render(
      <ImportExportSection
        {...baseProps}
        selectedFile={"/tmp/test/config.json"}
      />,
    );

    expect(screen.getByText(/config\.json/)).toBeInTheDocument();
    const importButton = screen.getByRole("button", {
      name: /settings\.import/,
    });
    expect(importButton).toBeEnabled();
    fireEvent.click(importButton);
    expect(baseProps.onImport).toHaveBeenCalledTimes(1);

    fireEvent.click(screen.getByRole("button", { name: "common.clear" }));
    expect(baseProps.onClear).toHaveBeenCalledTimes(1);
  });

  it("should show loading text and disable import button during import", () => {
    render(
      <ImportExportSection
        {...baseProps}
        selectedFile={"/tmp/test/config.json"}
        isImporting
        status="importing"
      />,
    );

    const importingLabels = screen.getAllByText("settings.importing");
    expect(importingLabels.length).toBeGreaterThanOrEqual(2);
    expect(
      screen.getByRole("button", { name: "settings.importing" }),
    ).toBeDisabled();
    expect(screen.getByText("common.loading")).toBeInTheDocument();
  });

  it("should display backup information on successful import", () => {
    render(
      <ImportExportSection
        {...baseProps}
        selectedFile={"/tmp/test/config.json"}
        status="success"
        backupId="backup-001"
      />,
    );

    expect(screen.getByText("settings.importSuccess")).toBeInTheDocument();
    expect(screen.getByText(/backup-001/)).toBeInTheDocument();
    expect(screen.getByText("settings.autoReload")).toBeInTheDocument();
  });

  it("should display error message when import fails", () => {
    render(
      <ImportExportSection
        {...baseProps}
        status="error"
        errorMessage="Parse failed"
      />,
    );

    expect(screen.getByText("settings.importFailed")).toBeInTheDocument();
    expect(screen.getByText("Parse failed")).toBeInTheDocument();
  });
});
