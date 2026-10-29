import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import {
  ZCodeNativeSwitchSection,
  type ZCodeNativeSwitchSectionProps,
} from "@/components/zcode/ZCodeNativeSwitchSection";
import type { SwitchOperationStatus } from "@/lib/api/zcodeAccounts";

const operation = (
  phase: SwitchOperationStatus["phase"],
  restartRequested = true,
): SwitchOperationStatus => ({
  requestId: "original-request",
  target: "opaque-removed-target",
  phase,
  refreshed: false,
  restartRequested,
});
const props = (
  overrides: Partial<ZCodeNativeSwitchSectionProps> = {},
): ZCodeNativeSwitchSectionProps => ({
  currentIdentity: null,
  identityReading: false,
  operation: null,
  queryState: "idle",
  operationPending: false,
  controlsDisabled: false,
  hasInspectedSource: true,
  onReadIdentity: vi.fn(),
  onQueryOriginal: vi.fn(),
  onViewRecovery: vi.fn(),
  ...overrides,
});
describe("ZCode native switch evidence", () => {
  it("keeps identity unknown until an explicit local read, with no online claim", () => {
    render(<ZCodeNativeSwitchSection {...props()} />);
    expect(screen.getByText("Local identity: unknown")).toBeInTheDocument();
    expect(
      screen.getByText(
        "A local read does not verify official-client sign-in or online availability.",
      ),
    ).toBeInTheDocument();
  });
  it("labels identity as a timestamped local observation, not the selected target", () => {
    render(
      <ZCodeNativeSwitchSection
        {...props({
          currentIdentity: {
            contextRevision: "ctx",
            id: "observed-id",
            label: "Observed account",
            family: "zai",
            readAt: 1_800_000_000_000,
          },
          operation: operation("restartVerified"),
        })}
      />,
    );
    expect(
      screen.getByText(/Local identity \(last read\): Observed account · zai/),
    ).toBeInTheDocument();
    expect(screen.getByText(/2027-01-15T08:00:00.000Z/)).toBeInTheDocument();
    expect(screen.getByText(/opaque-removed-target/)).toBeInTheDocument();
  });
  it.each([
    [
      "accepted",
      true,
      "Original request accepted; completion is not confirmed.",
    ],
    [
      "exited",
      true,
      "Client exit recorded; local write completion is not confirmed.",
    ],
    [
      "transactionUncertain",
      true,
      "The original write result is unknown. Query the original operation; do not switch again.",
    ],
    [
      "committed",
      true,
      "Local write committed; the requested restart result is still unknown.",
    ],
    ["committed", false, "Local write committed; no restart was requested."],
    [
      "restartVerified",
      true,
      "Local write committed and client process reopened. Account sign-in remains unverified.",
    ],
    [
      "restartFailed",
      true,
      "Local write committed; client reopening was not verified.",
    ],
    [
      "failed",
      false,
      "The original operation ended without a confirmed commit. The previous identity is not verified.",
    ],
  ] as const)(
    "renders %s using its real evidence (%s)",
    (phase, restartRequested, text) => {
      render(
        <ZCodeNativeSwitchSection
          {...props({
            operation: operation(phase, restartRequested),
            queryState: "ready",
          })}
        />,
      );
      expect(screen.getByRole("status")).toHaveTextContent(text);
      expect(screen.getByText("Local identity: unknown")).toBeInTheDocument();
      expect(
        screen.queryByText(/signed in successfully|account is online/i),
      ).not.toBeInTheDocument();
    },
  );
  it("does not turn a missing or failed operation query into success", () => {
    const { rerender } = render(
      <ZCodeNativeSwitchSection {...props({ queryState: "ready" })} />,
    );
    expect(screen.getByRole("status")).toHaveTextContent(
      "No confirmed original-operation record is available. This does not mean the switch succeeded.",
    );
    rerender(
      <ZCodeNativeSwitchSection
        {...props({
          operation: operation("transactionUncertain"),
          queryState: "error",
        })}
      />,
    );
    expect(screen.getByRole("status")).toHaveTextContent(
      "Unable to refresh the original operation. Previous evidence is retained; the latest status is unconfirmed.",
    );
    expect(screen.getByRole("status")).toHaveTextContent("original-request");
  });
  it("shows indeterminate pending state instead of completed stages and prevents callbacks", () => {
    const p = props({
      operationPending: true,
      operation: operation("restartVerified"),
    });
    render(<ZCodeNativeSwitchSection {...p} />);
    expect(screen.getByRole("status")).toHaveTextContent(
      "Operation in progress. Do not repeat it.",
    );
    expect(screen.queryByText(/process reopened/)).not.toBeInTheDocument();
    for (const name of [
      "Read local identity",
      "Query original operation",
      "View recovery records",
    ]) {
      const b = screen.getByRole("button", { name });
      expect(b).toBeDisabled();
      fireEvent.click(b);
    }
    expect(p.onReadIdentity).not.toHaveBeenCalled();
    expect(p.onQueryOriginal).not.toHaveBeenCalled();
    expect(p.onViewRecovery).not.toHaveBeenCalled();
  });
  it.each([
    { hasInspectedSource: false },
    { controlsDisabled: true },
    { identityReading: true },
    { queryState: "pending" as const },
  ])("locks native reads while unavailable: %o", (state) => {
    render(<ZCodeNativeSwitchSection {...props(state)} />);
    expect(
      screen.getByRole("button", { name: "Read local identity" }),
    ).toBeDisabled();
    expect(
      screen.getByRole("button", { name: "Query original operation" }),
    ).toBeDisabled();
  });
  it("exposes only the real read/query/navigation callbacks, with unsupported writes disabled", () => {
    const p = props({ operation: operation("restartFailed") });
    render(<ZCodeNativeSwitchSection {...p} />);
    fireEvent.click(
      screen.getByRole("button", { name: "Read local identity" }),
    );
    fireEvent.click(
      screen.getByRole("button", { name: "Query original operation" }),
    );
    fireEvent.click(
      screen.getByRole("button", { name: "View recovery records" }),
    );
    expect(p.onReadIdentity).toHaveBeenCalledTimes(1);
    expect(p.onQueryOriginal).toHaveBeenCalledTimes(1);
    expect(p.onViewRecovery).toHaveBeenCalledTimes(1);
    expect(
      screen.getByRole("button", { name: "Restore pre-switch data" }),
    ).toBeDisabled();
    expect(
      screen.getByRole("button", { name: "Retry startup" }),
    ).toBeDisabled();
    expect(
      screen.getByText(
        "This version cannot automatically restore pre-switch data or retry startup separately. Review recovery records and verify the account in the official client.",
      ),
    ).toBeInTheDocument();
    expect(
      screen.getByText(
        /Opening the client manually does not clear the original operation/,
      ),
    ).toBeInTheDocument();
  });
});

it("labels a changed original pointer without treating old unresolved evidence as completed", () => {
  render(
    <ZCodeNativeSwitchSection
      {...props({
        operation: operation("transactionUncertain"),
        queryState: "changed",
      })}
    />,
  );
  expect(screen.getByRole("status")).toHaveTextContent(
    "The original request changed. Earlier unresolved evidence is retained; verify the original operation before switching again.",
  );
  expect(screen.getByRole("status")).toHaveTextContent("original-request");
});
