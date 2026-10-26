import { fireEvent, render, screen } from "@testing-library/react";
import { afterEach, expect, it, vi } from "vitest";
import { CredentialKeyRecoveryHelp } from "@/components/relay/CredentialKeyRecoveryHelp";
afterEach(() => vi.unstubAllGlobals());
it.each([
  ["Mozilla/5.0 (Macintosh; Intel Mac OS X)", "MacIntel", "mac"],
  ["Mozilla/5.0 (Windows NT 10.0; Win64; x64)", "Win32", "windows"],
  ["Mozilla/5.0 (X11; Linux x86_64)", "Linux x86_64", "linux"],
  ["unknown", "", "unknown"],
])(
  "shows only the current system recovery instructions (%s)",
  (userAgent, platform, expected) => {
    vi.stubGlobal("navigator", { userAgent, platform });
    render(<CredentialKeyRecoveryHelp />);
    expect(
      screen.queryByText(`storageProtection.recovery.${expected}`),
    ).not.toBeInTheDocument();
    fireEvent.click(
      screen.getByRole("button", { name: "storageProtection.forgotKey" }),
    );
    expect(
      screen.getByText(`storageProtection.recovery.${expected}`),
    ).toBeVisible();
    for (const other of ["mac", "windows", "linux", "unknown"].filter(
      (os) => os !== expected,
    )) {
      expect(
        screen.queryByText(`storageProtection.recovery.${other}`),
      ).not.toBeInTheDocument();
    }
    expect(
      screen.getByText("storageProtection.recovery.binaryNotice"),
    ).toBeVisible();
    expect(
      screen.queryByRole("button", { name: /copy|show password|显示密码/i }),
    ).not.toBeInTheDocument();
  },
);
