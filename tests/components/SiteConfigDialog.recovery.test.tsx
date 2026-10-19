import { describe, it, expect, vi, beforeEach } from "vitest";
import {
  render,
  screen,
  fireEvent,
  waitFor,
  act,
} from "@testing-library/react";
import { SiteConfigDialog } from "@/components/relay/SiteConfigDialog";
const api = vi.hoisted(() => ({
  applySiteConfig: vi.fn(),
  resetSiteConfig: vi.fn(),
}));
vi.mock("@/lib/api/relay", () => ({ relayApi: api }));
const summary = { applied: ["synthetic-provider"] };
beforeEach(() => {
  api.applySiteConfig.mockReset().mockResolvedValue(summary);
  api.resetSiteConfig.mockReset().mockResolvedValue(summary);
});
describe("site configuration saved-state recovery", () => {
  for (const operation of ["apply", "reset"] as const) {
    it(`${operation}: retries refresh without repeating an already committed change`, async () => {
      const refresh = vi
        .fn()
        .mockRejectedValueOnce(new Error("synthetic refresh failure"))
        .mockResolvedValue(undefined);
      const close = vi.fn();
      render(
        <SiteConfigDialog
          relayId={7}
          relayLabel="Synthetic relay"
          open
          onOpenChange={close}
          onApplied={refresh}
        />,
      );
      fireEvent.change(screen.getByRole("textbox"), {
        target: { value: "synthetic site config" },
      });
      fireEvent.click(
        screen.getByRole("button", {
          name: `loongport.siteConfig.${operation}`,
        }),
      );
      await waitFor(() => expect(refresh).toHaveBeenCalledOnce());
      const retry = await screen.findByRole("button", { name: "common.retry" });
      expect(
        screen.getByRole("button", { name: "loongport.siteConfig.apply" }),
      ).toBeDisabled();
      expect(
        screen.getByRole("button", { name: "loongport.siteConfig.reset" }),
      ).toBeDisabled();
      fireEvent.click(retry);
      await waitFor(() => expect(close).toHaveBeenCalledWith(false));
      expect(refresh).toHaveBeenCalledTimes(2);
      expect(api.applySiteConfig).toHaveBeenCalledTimes(
        operation === "apply" ? 1 : 0,
      );
      expect(api.resetSiteConfig).toHaveBeenCalledTimes(
        operation === "reset" ? 1 : 0,
      );
    });
  }
});

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (error: Error) => void;
  const promise = new Promise<T>((yes, no) => {
    resolve = yes;
    reject = no;
  });
  return { promise, resolve, reject };
}
for (const operation of ["apply", "reset"] as const) {
  it(`${operation}: save failure keeps the draft and permits an explicit retry`, async () => {
    const method =
      operation === "apply" ? api.applySiteConfig : api.resetSiteConfig;
    method.mockRejectedValueOnce(new Error("synthetic save failure"));
    const refresh = vi.fn();
    const close = vi.fn();
    render(
      <SiteConfigDialog
        relayId={7}
        relayLabel="Synthetic"
        open
        onOpenChange={close}
        onApplied={refresh}
      />,
    );
    fireEvent.change(screen.getByRole("textbox"), {
      target: { value: "synthetic draft" },
    });
    const button = screen.getByRole("button", {
      name: `loongport.siteConfig.${operation}`,
    });
    fireEvent.click(button);
    await waitFor(() => expect(button).toBeEnabled());
    expect(refresh).not.toHaveBeenCalled();
    expect(close).not.toHaveBeenCalled();
    expect(screen.getByRole("textbox")).toHaveValue("synthetic draft");
    fireEvent.click(button);
    await waitFor(() => expect(close).toHaveBeenCalledWith(false));
    expect(method).toHaveBeenCalledTimes(2);
    expect(refresh).toHaveBeenCalledOnce();
  });
  it(`${operation}: pending save and refresh block duplicate mutations and dismissal`, async () => {
    const method =
      operation === "apply" ? api.applySiteConfig : api.resetSiteConfig;
    const save = deferred<typeof summary>();
    const read = deferred<void>();
    method.mockReturnValueOnce(save.promise);
    const refresh = vi.fn(() => read.promise);
    const close = vi.fn();
    render(
      <SiteConfigDialog
        relayId={7}
        relayLabel="Synthetic"
        open
        onOpenChange={close}
        onApplied={refresh}
      />,
    );
    fireEvent.change(screen.getByRole("textbox"), {
      target: { value: "synthetic draft" },
    });
    const button = screen.getByRole("button", {
      name: `loongport.siteConfig.${operation}`,
    });
    fireEvent.click(button);
    fireEvent.click(button);
    fireEvent.click(screen.getByRole("button", { name: "common.cancel" }));
    fireEvent.keyDown(screen.getByRole("dialog"), { key: "Escape" });
    expect(close).not.toHaveBeenCalled();
    expect(method).toHaveBeenCalledOnce();
    await act(async () => save.resolve(summary));
    expect(refresh).toHaveBeenCalledOnce();
    expect(
      screen.getByRole("button", { name: "common.cancel" }),
    ).toBeDisabled();
    await act(async () => read.resolve());
    expect(close).toHaveBeenCalledWith(false);
    expect(method).toHaveBeenCalledOnce();
  });
  it(`${operation}: late save after unmount cannot close a newly opened dialog`, async () => {
    const method =
      operation === "apply" ? api.applySiteConfig : api.resetSiteConfig;
    const save = deferred<typeof summary>();
    method.mockReturnValueOnce(save.promise);
    const oldRefresh = vi.fn();
    const close = vi.fn();
    const old = render(
      <SiteConfigDialog
        relayId={7}
        relayLabel="Old"
        open
        onOpenChange={close}
        onApplied={oldRefresh}
      />,
    );
    fireEvent.change(screen.getByRole("textbox"), {
      target: { value: "synthetic draft" },
    });
    fireEvent.click(
      screen.getByRole("button", { name: `loongport.siteConfig.${operation}` }),
    );
    old.unmount();
    render(
      <SiteConfigDialog
        relayId={8}
        relayLabel="New"
        open
        onOpenChange={close}
        onApplied={vi.fn()}
      />,
    );
    await act(async () => save.resolve(summary));
    expect(close).not.toHaveBeenCalled();
    expect(oldRefresh).not.toHaveBeenCalled();
    expect(
      screen.getByRole("button", { name: "loongport.siteConfig.reset" }),
    ).toBeEnabled();
  });
}
it("failed refresh retry remains refresh-only and dismissal clears the saved local receipt", async () => {
  const retry = deferred<void>();
  const refresh = vi
    .fn()
    .mockRejectedValueOnce(new Error("first read failure"))
    .mockReturnValueOnce(retry.promise)
    .mockResolvedValue(undefined);
  const close = vi.fn();
  const props = {
    relayId: 7,
    relayLabel: "Synthetic",
    onOpenChange: close,
    onApplied: refresh,
  };
  const view = render(<SiteConfigDialog {...props} open />);
  fireEvent.click(
    screen.getByRole("button", { name: "loongport.siteConfig.reset" }),
  );
  fireEvent.click(await screen.findByRole("button", { name: "common.retry" }));
  await act(async () => retry.reject(new Error("second read failure")));
  expect(
    await screen.findByRole("button", { name: "common.retry" }),
  ).toBeEnabled();
  expect(api.resetSiteConfig).toHaveBeenCalledOnce();
  fireEvent.click(screen.getByRole("button", { name: "common.cancel" }));
  expect(close).toHaveBeenCalledWith(false);
  view.rerender(<SiteConfigDialog {...props} open={false} />);
  view.rerender(<SiteConfigDialog {...props} open />);
  expect(
    screen.queryByRole("button", { name: "common.retry" }),
  ).not.toBeInTheDocument();
  expect(
    screen.getByRole("button", { name: "loongport.siteConfig.reset" }),
  ).toBeEnabled();
});

it("a late refresh from a previous station cannot close the next station's dialog", async () => {
  const read = deferred<void>();
  const close = vi.fn();
  const view = render(
    <SiteConfigDialog
      key="a"
      relayId={7}
      relayLabel="A"
      open
      onOpenChange={close}
      onApplied={() => read.promise}
    />,
  );
  fireEvent.click(
    screen.getByRole("button", { name: "loongport.siteConfig.reset" }),
  );
  await screen.findByRole("status");
  view.rerender(
    <SiteConfigDialog
      key="b"
      relayId={8}
      relayLabel="B"
      open
      onOpenChange={close}
      onApplied={vi.fn()}
    />,
  );
  fireEvent.change(screen.getByRole("textbox"), {
    target: { value: "new station draft" },
  });
  await act(async () => read.resolve());
  expect(close).not.toHaveBeenCalled();
  expect(screen.getByRole("textbox")).toHaveValue("new station draft");
  expect(screen.queryByRole("status")).not.toBeInTheDocument();
});
