import {
  act,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { QoderInferenceIdentity } from "@/components/providers/qoder/QoderInferenceIdentity";
import type {
  QoderHttpIdentityContext,
  QoderHttpIdentityResult,
} from "@/lib/api/qoderHttpAuth";

const api = vi.hoisted(() => ({ context: vi.fn(), action: vi.fn() }));
const nativeDialog = vi.hoisted(() => ({ confirm: vi.fn(), ask: vi.fn() }));
vi.mock("@/lib/api/qoderHttpAuth", () => ({ qoderHttpAuthApi: api }));
vi.mock("@tauri-apps/plugin-dialog", () => nativeDialog);

const unestablished: QoderHttpIdentityContext = {
  providerId: "fixture",
  credentialGeneration: "generation-a",
  status: "unestablished",
  expiresAtMs: null,
  allowedActions: ["establish"],
};
const ready: QoderHttpIdentityContext = {
  ...unestablished,
  status: "ready",
  expiresAtMs: 1_900_000_000_000,
  allowedActions: ["rebind"],
};
const labels = {
  establish: "声明国际版 PAT 并建立",
  recover: "恢复推理身份",
  rebind: "重新绑定账号",
};

function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((r) => {
    resolve = r;
  });
  return { promise, resolve };
}
async function open(providerId = "fixture") {
  fireEvent.click(screen.getByRole("button", { name: "Qoder 推理身份" }));
  await waitFor(() => expect(api.context).toHaveBeenCalledWith(providerId));
}
async function declare() {
  const checkbox = await screen.findByRole("checkbox");
  fireEvent.click(checkbox);
}
async function submit(action: keyof typeof labels = "establish") {
  const button = screen.getByRole("button", { name: labels[action] });
  await waitFor(() => expect(button).toBeEnabled());
  fireEvent.click(button);
}
async function close() {
  fireEvent.keyDown(document, { key: "Escape" });
  await waitFor(() =>
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument(),
  );
}
beforeEach(() => {
  api.context.mockReset().mockResolvedValue(unestablished);
  api.action
    .mockReset()
    .mockResolvedValue({ status: "completed", context: ready });
  nativeDialog.confirm.mockReset();
  nativeDialog.ask.mockReset();
});

describe("original Qoder Provider inference identity", () => {
  it("renders inertly and opening reads context without any action", async () => {
    render(<QoderInferenceIdentity providerId="fixture" />);
    expect(api.context).not.toHaveBeenCalled();
    expect(api.action).not.toHaveBeenCalled();
    await open();
    expect(await screen.findByText("尚未建立推理身份。")).toBeInTheDocument();
    expect(api.action).not.toHaveBeenCalled();
    expect(
      screen.getByRole("button", { name: labels.establish }),
    ).toBeDisabled();
  });

  it("requires the explicit source checkbox and submits the displayed generation once without another approval", async () => {
    render(<QoderInferenceIdentity providerId="fixture" />);
    await open();
    await declare();
    await submit();
    expect(api.action).toHaveBeenCalledExactlyOnceWith(
      unestablished,
      "establish",
    );
    expect(
      await screen.findByText("已建立推理身份，可供后续请求复用。"),
    ).toBeInTheDocument();
    expect(nativeDialog.confirm).not.toHaveBeenCalled();
    expect(nativeDialog.ask).not.toHaveBeenCalled();
  });

  it("keeps the closed production gate visibly unavailable and cannot submit", async () => {
    api.context.mockResolvedValue({
      ...unestablished,
      credentialGeneration: "",
      status: "unavailable",
      allowedActions: [],
    });
    render(<QoderInferenceIdentity providerId="fixture" />);
    await open();
    expect(
      await screen.findByText(/当前版本尚未开放 Qoder HTTP 推理身份/),
    ).toBeInTheDocument();
    expect(screen.queryByRole("checkbox")).not.toBeInTheDocument();
    expect(
      screen.queryByRole("button", { name: labels.establish }),
    ).not.toBeInTheDocument();
    expect(api.action).not.toHaveBeenCalled();
  });

  it.each([
    ["ready", "已建立推理身份，可供后续请求复用。"],
    ["refreshNeeded", "等待下一次请求自动刷新。"],
  ] as const)(
    "shows %s without an opening exchange or refresh",
    async (status, message) => {
      api.context.mockResolvedValue({ ...ready, status });
      render(<QoderInferenceIdentity providerId="fixture" />);
      await open();
      expect(await screen.findByText(message)).toBeInTheDocument();
      expect(api.action).not.toHaveBeenCalled();
      expect(
        screen.queryByRole("button", { name: labels.establish }),
      ).not.toBeInTheDocument();
    },
  );

  it("uses one ordinary explicit establishment for access-only expiry", async () => {
    const expired = {
      ...unestablished,
      status: "reestablishRequired" as const,
    };
    api.context.mockResolvedValue(expired);
    render(<QoderInferenceIdentity providerId="fixture" />);
    await open();
    expect(await screen.findByText("请重新建立推理身份。")).toBeInTheDocument();
    await declare();
    await submit();
    expect(api.action).toHaveBeenCalledExactlyOnceWith(expired, "establish");
    expect(nativeDialog.confirm).not.toHaveBeenCalled();
  });

  it.each(["pending", "unknown"] as const)(
    "requires explicit recovery for %s without a second frontend confirmation",
    async (status) => {
      const recoverable: QoderHttpIdentityContext = {
        ...unestablished,
        status,
        allowedActions: ["recover"],
      };
      api.context.mockResolvedValue(recoverable);
      api.action.mockResolvedValue({ status: "cancelled" });
      render(<QoderInferenceIdentity providerId="fixture" />);
      await open();
      expect(api.action).not.toHaveBeenCalled();
      expect(
        screen.queryByRole("button", { name: labels.establish }),
      ).not.toBeInTheDocument();
      await declare();
      await submit("recover");
      expect(api.action).toHaveBeenCalledExactlyOnceWith(
        recoverable,
        "recover",
      );
      expect(
        await screen.findByText("已取消，未提交新的身份建立请求。"),
      ).toBeInTheDocument();
      expect(nativeDialog.confirm).not.toHaveBeenCalled();
      expect(nativeDialog.ask).not.toHaveBeenCalled();
    },
  );

  it("invokes explicit rebind once and leaves its sole confirmation to the backend", async () => {
    api.context.mockResolvedValue(ready);
    render(<QoderInferenceIdentity providerId="fixture" />);
    await open();
    await declare();
    await submit("rebind");
    expect(api.action).toHaveBeenCalledExactlyOnceWith(ready, "rebind");
    expect(nativeDialog.confirm).not.toHaveBeenCalled();
    expect(nativeDialog.ask).not.toHaveBeenCalled();
  });

  it("suppresses repeated submits while the one owner action is unresolved", async () => {
    const action = deferred<QoderHttpIdentityResult>();
    api.action.mockReturnValue(action.promise);
    render(<QoderInferenceIdentity providerId="fixture" />);
    await open();
    await declare();
    const button = screen.getByRole("button", { name: labels.establish });
    fireEvent.click(button);
    fireEvent.click(button);
    expect(api.action).toHaveBeenCalledTimes(1);
    expect(
      await screen.findByText(/关闭此窗口不会撤销已提交的操作/),
    ).toBeInTheDocument();
    await act(async () =>
      action.resolve({ status: "completed", context: ready }),
    );
    expect(
      await screen.findByText("已建立推理身份，可供后续请求复用。"),
    ).toBeInTheDocument();
  });

  it("uses changed context without silently recovering or reusing the old declaration", async () => {
    const changed: QoderHttpIdentityContext = {
      ...unestablished,
      credentialGeneration: "generation-b",
      status: "unknown",
      allowedActions: ["recover"],
    };
    api.action.mockResolvedValue({ status: "changed", context: changed });
    render(<QoderInferenceIdentity providerId="fixture" />);
    await open();
    await declare();
    await submit();
    expect(
      await screen.findByText(/状态已变化，请根据当前状态重新选择操作/),
    ).toBeInTheDocument();
    expect(screen.getByRole("checkbox")).not.toBeChecked();
    expect(screen.getByRole("button", { name: labels.recover })).toBeDisabled();
    expect(api.action).toHaveBeenCalledTimes(1);
    await declare();
    await submit("recover");
    expect(api.action).toHaveBeenLastCalledWith(changed, "recover");
  });

  it("clears declaration on close/reopen and ignores a late action result", async () => {
    const action = deferred<QoderHttpIdentityResult>();
    api.action.mockReturnValue(action.promise);
    render(<QoderInferenceIdentity providerId="fixture" />);
    await open();
    await declare();
    await submit();
    await close();
    await act(async () =>
      action.resolve({ status: "completed", context: ready }),
    );
    await open();
    expect(await screen.findByText("尚未建立推理身份。")).toBeInTheDocument();
    expect(screen.getByRole("checkbox")).not.toBeChecked();
    expect(
      screen.queryByText("已建立推理身份，可供后续请求复用。"),
    ).not.toBeInTheDocument();
    expect(api.action).toHaveBeenCalledTimes(1);
  });

  it("ignores stale contexts across Provider switches and sends only the new displayed generation", async () => {
    const oldContext = deferred<QoderHttpIdentityContext>();
    const next = {
      ...unestablished,
      providerId: "fixture-b",
      credentialGeneration: "generation-b",
    };
    api.context.mockImplementation((id: string) =>
      id === "fixture" ? oldContext.promise : Promise.resolve(next),
    );
    const view = render(<QoderInferenceIdentity providerId="fixture" />);
    await open();
    view.rerender(<QoderInferenceIdentity providerId="fixture-b" />);
    await waitFor(() => expect(api.context).toHaveBeenCalledWith("fixture-b"));
    await act(async () => oldContext.resolve(unestablished));
    await declare();
    await submit();
    expect(api.action).toHaveBeenCalledExactlyOnceWith(next, "establish");
  });

  it("ignores late action results after switching Providers without resubmission", async () => {
    const action = deferred<QoderHttpIdentityResult>();
    api.action.mockReturnValue(action.promise);
    const view = render(<QoderInferenceIdentity providerId="fixture" />);
    await open();
    await declare();
    await submit();
    api.context.mockResolvedValue({
      ...unestablished,
      providerId: "fixture-b",
      credentialGeneration: "generation-b",
    });
    view.rerender(<QoderInferenceIdentity providerId="fixture-b" />);
    await waitFor(() => expect(api.context).toHaveBeenCalledWith("fixture-b"));
    await act(async () =>
      action.resolve({ status: "completed", context: ready }),
    );
    expect(await screen.findByText("尚未建立推理身份。")).toBeInTheDocument();
    expect(screen.getByRole("checkbox")).not.toBeChecked();
    expect(
      screen.queryByText("已建立推理身份，可供后续请求复用。"),
    ).not.toBeInTheDocument();
    expect(api.action).toHaveBeenCalledTimes(1);
  });

  it("does not update the UI or issue another action after unmount", async () => {
    const action = deferred<QoderHttpIdentityResult>();
    api.action.mockReturnValue(action.promise);
    const view = render(<QoderInferenceIdentity providerId="fixture" />);
    await open();
    await declare();
    await submit();
    view.unmount();
    await act(async () =>
      action.resolve({ status: "completed", context: ready }),
    );
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    expect(api.action).toHaveBeenCalledTimes(1);
  });

  it("honors mutationsDisabled and never reads context or submits", async () => {
    render(<QoderInferenceIdentity providerId="fixture" disabled />);
    const trigger = screen.getByRole("button", { name: "Qoder 推理身份" });
    expect(trigger).toBeDisabled();
    fireEvent.click(trigger);
    expect(api.context).not.toHaveBeenCalled();
    expect(api.action).not.toHaveBeenCalled();
  });

  it("does not render raw errors or private identity sentinel fields", async () => {
    api.context.mockResolvedValue({
      ...unestablished,
      uid: "SYNTHETIC-PRIVATE-UID",
      token: "SYNTHETIC-PRIVATE-TOKEN",
    });
    api.action.mockRejectedValue(new Error("SYNTHETIC-PRIVATE-TOKEN"));
    render(<QoderInferenceIdentity providerId="fixture" />);
    await open();
    await declare();
    await submit();
    expect(await screen.findByRole("alert")).toHaveTextContent(
      "操作结果未确认，请重新打开查看状态。",
    );
    expect(document.body.textContent).not.toContain("SYNTHETIC-PRIVATE");
    expect(api.action).toHaveBeenCalledTimes(1);
  });

  it("blocks context or action results for a different Provider", async () => {
    api.action.mockResolvedValue({
      status: "completed",
      context: { ...ready, providerId: "other" },
    });
    render(<QoderInferenceIdentity providerId="fixture" />);
    await open();
    await declare();
    await submit();
    expect(await screen.findByRole("alert")).toHaveTextContent(
      "操作结果未确认，请重新打开查看状态。",
    );
    expect(
      screen.queryByText("已建立推理身份，可供后续请求复用。"),
    ).not.toBeInTheDocument();
  });

  it("shows the fixed rebind-required failure without silently changing accounts", async () => {
    api.action.mockResolvedValue({
      status: "failed",
      code: "identity_rebind_required",
    });
    render(<QoderInferenceIdentity providerId="fixture" />);
    await open();
    await declare();
    await submit();
    expect(await screen.findByRole("alert")).toHaveTextContent(
      /账号身份不一致/,
    );
    expect(api.action).toHaveBeenCalledTimes(1);
    expect(nativeDialog.confirm).not.toHaveBeenCalled();
  });
  it("keeps the card click handler inert when opening or acting in the identity panel", async () => {
    const parentClick = vi.fn();
    render(
      <div onClick={parentClick}>
        <QoderInferenceIdentity providerId="fixture" />
      </div>,
    );
    await open();
    await declare();
    await submit();
    expect(parentClick).not.toHaveBeenCalled();
  });

  it("can reopen an in-flight view to read current status without another submission", async () => {
    const action = deferred<QoderHttpIdentityResult>();
    api.action.mockReturnValue(action.promise);
    render(<QoderInferenceIdentity providerId="fixture" />);
    await open();
    await declare();
    await submit();
    await close();
    api.context.mockResolvedValue({
      ...unestablished,
      status: "pending",
      allowedActions: ["recover"],
    });
    await open();
    const recover = await screen.findByRole("button", { name: labels.recover });
    expect(recover).toBeDisabled();
    expect(screen.getByRole("checkbox")).not.toBeChecked();
    await act(async () =>
      action.resolve({ status: "completed", context: ready }),
    );
    expect(
      screen.queryByText("已建立推理身份，可供后续请求复用。"),
    ).not.toBeInTheDocument();
    expect(api.action).toHaveBeenCalledTimes(1);
  });

  it("revokes the displayed declaration when mutation blocking changes while open", async () => {
    const view = render(<QoderInferenceIdentity providerId="fixture" />);
    await open();
    await declare();
    view.rerender(<QoderInferenceIdentity providerId="fixture" disabled />);
    expect(
      screen.queryByRole("button", { name: labels.establish }),
    ).not.toBeInTheDocument();
    expect(api.context).toHaveBeenCalledTimes(1);
    view.rerender(<QoderInferenceIdentity providerId="fixture" />);
    await waitFor(() => expect(api.context).toHaveBeenCalledTimes(2));
    expect(await screen.findByRole("checkbox")).not.toBeChecked();
    expect(api.action).not.toHaveBeenCalled();
  });

  it.each(["", "different-provider"])(
    "cannot act on context lacking the target binding: %s",
    async (invalid) => {
      api.context.mockResolvedValue(
        invalid
          ? { ...unestablished, providerId: invalid }
          : { ...unestablished, credentialGeneration: "" },
      );
      render(<QoderInferenceIdentity providerId="fixture" />);
      await open();
      if (invalid) {
        expect(await screen.findByRole("alert")).toHaveTextContent(
          "操作结果未确认，请重新打开查看状态。",
        );
        expect(screen.queryByRole("checkbox")).not.toBeInTheDocument();
      } else {
        await declare();
        expect(
          screen.getByRole("button", { name: labels.establish }),
        ).toBeDisabled();
      }
      expect(api.action).not.toHaveBeenCalled();
    },
  );

  it("keeps a failed context read inert and never renders its raw error", async () => {
    api.context.mockRejectedValue(new Error("SYNTHETIC-PRIVATE-TOKEN"));
    render(<QoderInferenceIdentity providerId="fixture" />);
    await open();
    expect(await screen.findByRole("alert")).toHaveTextContent(
      "操作结果未确认，请重新打开查看状态。",
    );
    expect(document.body.textContent).not.toContain("SYNTHETIC-PRIVATE-TOKEN");
    expect(api.action).not.toHaveBeenCalled();
  });

  it.each([
    "unavailable",
    "source_changed",
    "owner_changed",
    "invalid_source",
    "state_changed",
    "outcome_unknown",
  ] as const)(
    "shows a fixed %s failure and requires reopening before another action",
    async (code) => {
      api.action.mockResolvedValue({ status: "failed", code });
      render(<QoderInferenceIdentity providerId="fixture" />);
      await open();
      await declare();
      await submit();
      expect(await screen.findByRole("alert")).not.toBeEmptyDOMElement();
      expect(screen.queryByRole("checkbox")).not.toBeInTheDocument();
      expect(api.action).toHaveBeenCalledTimes(1);
    },
  );
  it("invalidates the old declaration when the same Provider's source generation changes", async () => {
    const next = { ...unestablished, credentialGeneration: "generation-b" };
    const view = render(
      <QoderInferenceIdentity
        providerId="fixture"
        sourceGeneration="generation-a"
      />,
    );
    await open();
    await declare();
    api.context.mockResolvedValue(next);
    view.rerender(
      <QoderInferenceIdentity
        providerId="fixture"
        sourceGeneration="generation-b"
      />,
    );
    await waitFor(() => expect(api.context).toHaveBeenCalledTimes(2));
    expect(await screen.findByRole("checkbox")).not.toBeChecked();
    expect(
      screen.getByRole("button", { name: labels.establish }),
    ).toBeDisabled();
    expect(api.action).not.toHaveBeenCalled();
    await declare();
    await submit();
    expect(api.action).toHaveBeenCalledExactlyOnceWith(next, "establish");
  });

  it.each(["completed", "failed"] as const)(
    "ignores a late %s result after the same Provider changes its source",
    async (status) => {
      const action = deferred<QoderHttpIdentityResult>();
      api.action.mockReturnValue(action.promise);
      const view = render(
        <QoderInferenceIdentity
          providerId="fixture"
          sourceGeneration="generation-a"
        />,
      );
      await open();
      await declare();
      await submit();
      api.context.mockResolvedValue({
        ...unestablished,
        credentialGeneration: "generation-b",
      });
      view.rerender(
        <QoderInferenceIdentity
          providerId="fixture"
          sourceGeneration="generation-b"
        />,
      );
      await waitFor(() => expect(api.context).toHaveBeenCalledTimes(2));
      expect(await screen.findByRole("checkbox")).toBeDisabled();
      const blockedAction = screen.getByRole("button", {
        name: labels.establish,
      });
      expect(blockedAction).toBeDisabled();
      fireEvent.click(blockedAction);
      expect(api.action).toHaveBeenCalledTimes(1);
      await act(async () =>
        action.resolve(
          status === "completed"
            ? { status, context: ready }
            : { status, code: "identity_rebind_required" },
        ),
      );
      expect(await screen.findByText("尚未建立推理身份。")).toBeInTheDocument();
      expect(screen.getByRole("checkbox")).not.toBeChecked();
      expect(
        screen.queryByText("已建立推理身份，可供后续请求复用。"),
      ).not.toBeInTheDocument();
      expect(screen.queryByRole("alert")).not.toBeInTheDocument();
      expect(api.action).toHaveBeenCalledTimes(1);
    },
  );

  it("ignores the previous context read after the same Provider changes its source", async () => {
    const oldContext = deferred<QoderHttpIdentityContext>();
    const next = { ...unestablished, credentialGeneration: "generation-b" };
    api.context.mockReturnValueOnce(oldContext.promise).mockResolvedValue(next);
    const view = render(
      <QoderInferenceIdentity
        providerId="fixture"
        sourceGeneration="generation-a"
      />,
    );
    await open();
    view.rerender(
      <QoderInferenceIdentity
        providerId="fixture"
        sourceGeneration="generation-b"
      />,
    );
    await waitFor(() => expect(api.context).toHaveBeenCalledTimes(2));
    await act(async () => oldContext.resolve(ready));
    expect(await screen.findByText("尚未建立推理身份。")).toBeInTheDocument();
    await declare();
    await submit();
    expect(api.action).toHaveBeenCalledExactlyOnceWith(next, "establish");
  });

  it("uses the context generation when legacy Card metadata is absent or lagging", async () => {
    render(
      <QoderInferenceIdentity
        providerId="fixture"
        sourceGeneration="lagging-card-revision"
      />,
    );
    await open();
    await declare();
    await submit();
    expect(api.action).toHaveBeenCalledExactlyOnceWith(
      unestablished,
      "establish",
    );
  });
});
