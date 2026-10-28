import { fireEvent, render, screen } from "@testing-library/react";
import { expect, it, vi } from "vitest";
import { CodexAccountNativeEntry } from "@/components/providers/forms/CodexAccountNativeEntry";
const row = (id: string) => ({
  id,
  name: "Repeated name",
  presentation: { isCurrent: true },
});
const props = () => ({
  accountId: "account-A",
  needsReauth: false,
  providers: [],
  state: "ready" as const,
  active: true,
  onOpenCodex: vi.fn(),
  onRetry: vi.fn(),
});
it("shows readonly bound configurations independently and never presents application selection as native identity", () => {
  const p = props();
  render(
    <CodexAccountNativeEntry
      {...p}
      providers={[row("first"), row("second")]}
    />,
  );
  expect(screen.getAllByText("Repeated name")).toHaveLength(2);
  expect(screen.getByText("first")).toBeInTheDocument();
  expect(screen.getByText("second")).toBeInTheDocument();
  expect(
    screen.getAllByText("Selected application configuration"),
  ).toHaveLength(2);
  expect(
    screen.getByText(
      /Default only changes LoongPort's default managed-auth account/,
    ),
  ).toBeInTheDocument();
  const native = screen.getByRole("button", {
    name: "Switch official client account",
  });
  expect(native).toBeDisabled();
  fireEvent.click(native);
  expect(p.onOpenCodex).not.toHaveBeenCalled();
  expect(p.onRetry).not.toHaveBeenCalled();
  fireEvent.click(
    screen.getByRole("button", { name: "Go to Codex application page" }),
  );
  expect(p.onOpenCodex).toHaveBeenCalledOnce();
});
it("keeps navigation available for empty or reauthorization-required accounts without creating a configuration", () => {
  const p = props();
  render(<CodexAccountNativeEntry {...p} needsReauth />);
  expect(
    screen.getByText(/No Codex official configuration explicitly bound/),
  ).toBeInTheDocument();
  expect(
    screen.getByText(/Renew authorization before using this account/),
  ).toBeInTheDocument();
  fireEvent.click(
    screen.getByRole("button", { name: "Go to Codex application page" }),
  );
  expect(p.onOpenCodex).toHaveBeenCalledOnce();
});
it("shows a read error rather than stale links and offers only a read retry", () => {
  const p = props();
  render(
    <CodexAccountNativeEntry {...p} state="error" providers={[row("stale")]} />,
  );
  expect(screen.queryByText("stale")).not.toBeInTheDocument();
  expect(screen.getByRole("alert")).toHaveTextContent(
    /Unable to read linked configurations/,
  );
  fireEvent.click(
    screen.getByRole("button", { name: "Retry reading configurations" }),
  );
  expect(p.onRetry).toHaveBeenCalledOnce();
  expect(p.onOpenCodex).not.toHaveBeenCalled();
});
it("explains a missing application navigation context and disables its action", () => {
  render(<CodexAccountNativeEntry {...props()} onOpenCodex={undefined} />);
  expect(
    screen.getByRole("button", { name: "Go to Codex application page" }),
  ).toBeDisabled();
  expect(
    screen.getByText(/This view has no application navigation entry/),
  ).toBeInTheDocument();
});
it("does not expose stale links while loading or dispatch controls while inactive", () => {
  const p = props();
  render(
    <CodexAccountNativeEntry
      {...p}
      state="loading"
      active={false}
      providers={[row("stale")]}
    />,
  );
  expect(screen.queryByText("stale")).not.toBeInTheDocument();
  expect(screen.getByRole("status")).toHaveTextContent(
    /Loading linked Codex configurations/,
  );
  const open = screen.getByRole("button", {
    name: "Go to Codex application page",
  });
  expect(open).toBeDisabled();
  fireEvent.click(open);
  expect(p.onOpenCodex).not.toHaveBeenCalled();
});
