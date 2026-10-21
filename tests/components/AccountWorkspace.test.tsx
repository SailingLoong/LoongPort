import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { expect, it } from "vitest";
import { AccountWorkspace } from "@/components/relay/accounts/AccountWorkspace";
const rows = [
  { id: "a", label: "Alpha" },
  { id: "b", label: "Beta" },
];
it("shows one original account detail and selects by stable identity, not row position", async () => {
  const user = userEvent.setup();
  const view = (accounts: typeof rows) => (
    <AccountWorkspace accounts={accounts} label={(row) => row.label}>
      {(row) => <article>Details {row.id}</article>}
    </AccountWorkspace>
  );
  const { rerender } = render(view(rows));
  expect(screen.getByText("Details a")).toBeInTheDocument();
  await user.click(screen.getByRole("button", { name: "查看 Beta" }));
  expect(screen.getByText("Details b")).toBeInTheDocument();
  expect(screen.queryByText("Details a")).not.toBeInTheDocument();
  rerender(view([...rows].reverse()));
  expect(screen.getByText("Details b")).toBeInTheDocument();
  rerender(view([rows[0]]));
  expect(screen.getByText("Details a")).toBeInTheDocument();
});
it("search only filters navigation and never mutates accounts", async () => {
  const user = userEvent.setup();
  render(
    <AccountWorkspace accounts={rows} label={(row) => row.label}>
      {(row) => <article>Details {row.id}</article>}
    </AccountWorkspace>,
  );
  await user.type(screen.getByRole("searchbox"), "missing");
  expect(screen.getByText("没有匹配的账号")).toBeInTheDocument();
  expect(rows).toHaveLength(2);
});
