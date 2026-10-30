import { act, render } from "@testing-library/react";
import { afterEach, expect, it, vi } from "vitest";
afterEach(() => vi.restoreAllMocks());
import { PreservedView } from "@/components/ui/PreservedView";
import { AddProviderForm } from "@/components/providers/AddProviderForm";
let lastSubmit: (values: any) => Promise<void>;
vi.mock("@/components/providers/forms/ProviderForm", () => ({
  ProviderForm: ({ onSubmit }: any) => {
    lastSubmit = onSubmit;
    return null;
  },
}));
vi.mock("react-i18next", () => ({
  useTranslation: () => ({ t: (key: string) => key }),
}));
vi.mock("sonner", () => ({
  toast: { error: vi.fn(), warning: vi.fn(), success: vi.fn() },
}));
const values = { name: "Test", settingsConfig: "{}" };
it("rejects a retained submit callback while the manual form is hidden", async () => {
  const save = vi.fn();
  const view = render(
    <PreservedView active>
      <AddProviderForm appId="opencode" onSubmit={save} onDone={vi.fn()} />
    </PreservedView>,
  );
  const retained = lastSubmit;
  view.rerender(
    <PreservedView active={false}>
      <AddProviderForm appId="opencode" onSubmit={save} onDone={vi.fn()} />
    </PreservedView>,
  );
  await act(async () => {
    await retained(values);
  });
  expect(save).not.toHaveBeenCalled();
});

import { useAddProviderFormViewModel } from "@/components/providers/useAddProviderFormViewModel";
import { universalProvidersApi } from "@/lib/api";
import { toast } from "sonner";
let model: ReturnType<typeof useAddProviderFormViewModel>;
function openModal() {
  if (model.activeTab !== "universal") act(() => model.changeTab("universal"));
  act(() => model.openUniversalForm());
}

function ModelHarness({
  done,
}: {
  done: (reason?: "saved" | "cancel", epoch?: number) => void;
}) {
  model = useAddProviderFormViewModel({
    appId: "codex",
    onSubmit: vi.fn(),
    onDone: done,
  });
  return null;
}
it("finishes the already-dispatched universal sync but does not announce or navigate after hide", async () => {
  let resolve!: (value: boolean) => void;
  vi.spyOn(universalProvidersApi, "upsert").mockImplementation(
    () =>
      new Promise<boolean>((r) => {
        resolve = r;
      }),
  );
  const sync = vi.spyOn(universalProvidersApi, "sync").mockResolvedValue(true);
  const success = vi.spyOn(toast, "success");
  success.mockClear();
  const done = vi.fn();
  const tree = (active: boolean) => (
    <PreservedView active={active}>
      <ModelHarness done={done} />
    </PreservedView>
  );
  const view = render(tree(true));
  openModal();
  let pending!: Promise<void>;
  act(() => {
    pending = model.handleUniversalProviderSave({ id: "synthetic" } as any);
  });
  view.rerender(tree(false));
  await act(async () => {
    resolve(true);
    await pending;
  });
  expect(sync).toHaveBeenCalledWith("synthetic");
  expect(success).not.toHaveBeenCalled();
  expect(done).not.toHaveBeenCalled();
  vi.restoreAllMocks();
});
it("does not revive a retained submit callback after hiding and revealing the form", async () => {
  const save = vi.fn();
  const tree = (active: boolean) => (
    <PreservedView active={active}>
      <AddProviderForm appId="opencode" onSubmit={save} onDone={vi.fn()} />
    </PreservedView>
  );
  const view = render(tree(true));
  const retained = lastSubmit;
  view.rerender(tree(false));
  view.rerender(tree(true));
  await act(async () => {
    await retained(values);
  });
  expect(save).not.toHaveBeenCalled();
});
it("rejects an app-specific command retained after switching to the universal tab", async () => {
  const save = vi.fn();
  function Harness() {
    model = useAddProviderFormViewModel({
      appId: "codex",
      onSubmit: save,
      onDone: vi.fn(),
    });
    return null;
  }
  render(<Harness />);
  const retained = model.handleSubmit;
  act(() => model.changeTab("universal"));
  await act(async () => {
    await retained(values);
  });
  expect(save).not.toHaveBeenCalled();
});
it("does not let an old readiness callback update a restored Pi form", async () => {
  function Harness() {
    model = useAddProviderFormViewModel({
      appId: "pi",
      onSubmit: vi.fn(),
      onDone: vi.fn(),
    });
    return null;
  }
  const tree = (active: boolean) => (
    <PreservedView active={active}>
      <Harness />
    </PreservedView>
  );
  const view = render(tree(true));
  const retained = model.handleSubmitReadyChange;
  view.rerender(tree(false));
  view.rerender(tree(true));
  act(() => retained(true));
  expect(model.isFormReady).toBe(false);
});
it("rejects a delayed close callback belonging to an older universal draft", () => {
  const done = vi.fn();
  render(<ModelHarness done={done} />);
  openModal();
  const closeOld = model.handleUniversalFormClose;
  act(() => closeOld());
  openModal();
  expect(model.universalFormOpen).toBe(true);
  act(() => closeOld());
  expect(model.universalFormOpen).toBe(true);
});
it("retired app submit cannot dispatch after cancel in the same turn", async () => {
  const save = vi.fn(),
    done = vi.fn();
  function Harness() {
    model = useAddProviderFormViewModel({
      appId: "opencode",
      onSubmit: save,
      onDone: done,
    });
    return null;
  }
  render(<Harness />);
  const retained = model.handleSubmit;
  await act(async () => {
    model.cancel();
    await retained(values);
  });
  expect(done).toHaveBeenCalledWith("cancel");
  expect(save).not.toHaveBeenCalled();
});
it("rejects a retained universal save after a user closes its draft", async () => {
  const upsert = vi.spyOn(universalProvidersApi, "upsert");
  render(<ModelHarness done={vi.fn()} />);
  openModal();
  const retained = model.handleUniversalProviderSave;
  act(() => model.handleUniversalFormClose());
  await act(async () => {
    await retained({ id: "synthetic" } as any);
  });
  expect(upsert).not.toHaveBeenCalled();
  vi.restoreAllMocks();
});
it("admits one universal save per draft and preserves save-then-close completion", async () => {
  let resolve!: (value: boolean) => void;
  const upsert = vi.spyOn(universalProvidersApi, "upsert").mockImplementation(
    () =>
      new Promise<boolean>((r) => {
        resolve = r;
      }),
  );
  const sync = vi.spyOn(universalProvidersApi, "sync").mockResolvedValue(true);
  const done = vi.fn();
  render(<ModelHarness done={done} />);
  openModal();
  const save = model.handleUniversalProviderSave,
    close = model.handleUniversalFormClose;
  let first!: Promise<void>, duplicate!: Promise<void>;
  act(() => {
    first = save({ id: "synthetic" } as any);
    duplicate = save({ id: "synthetic" } as any);
    close();
  });
  expect(upsert).toHaveBeenCalledTimes(1);
  await act(async () => {
    resolve(true);
    await Promise.all([first, duplicate]);
  });
  expect(sync).toHaveBeenCalledTimes(1);
  expect(done).toHaveBeenCalledTimes(1);
  vi.restoreAllMocks();
});
it("admits one app submission while its save is pending", async () => {
  let resolve!: () => void;
  const save = vi.fn(
    () =>
      new Promise<void>((r) => {
        resolve = r;
      }),
  );
  const done = vi.fn();
  function Harness() {
    model = useAddProviderFormViewModel({
      appId: "opencode",
      onSubmit: save,
      onDone: done,
    });
    return null;
  }
  render(<Harness />);
  let first!: Promise<void>, duplicate!: Promise<void>;
  act(() => {
    first = model.handleSubmit(values);
    duplicate = model.handleSubmit(values);
  });
  expect(save).toHaveBeenCalledTimes(1);
  await act(async () => {
    resolve();
    await Promise.all([first, duplicate]);
  });
  expect(done).toHaveBeenCalledTimes(1);
});
it("rejects retained form state callbacks after changing drafts", () => {
  render(<ModelHarness done={vi.fn()} />);
  const auth = model.setAuthSettingsTarget,
    submitting = model.setIsFormSubmitting;
  act(() => model.changeTab("universal"));
  act(() => {
    auth("codex_oauth");
    submitting(true);
  });
  expect(model.authSettingsTarget).toBeNull();
  expect(model.isFormSubmitting).toBe(false);
});
it("allows a new draft while an old universal save is pending and preserves the new epoch", async () => {
  const resolves: Array<(value: boolean) => void> = [];
  const upsert = vi
    .spyOn(universalProvidersApi, "upsert")
    .mockImplementation(() => new Promise<boolean>((r) => resolves.push(r)));
  const sync = vi.spyOn(universalProvidersApi, "sync").mockResolvedValue(true);
  let epoch = 10;
  const done = vi.fn();
  function Harness() {
    model = useAddProviderFormViewModel({
      appId: "codex",
      onSubmit: vi.fn(),
      onDone: done,
      onSaveStart: () => epoch++,
    });
    return null;
  }
  render(<Harness />);
  openModal();
  let first!: Promise<void>, second!: Promise<void>;
  act(() => {
    first = model.handleUniversalProviderSave({ id: "first" } as any);
    model.handleUniversalFormClose();
  });
  openModal();
  act(() => {
    second = model.handleUniversalProviderSave({ id: "second" } as any);
    model.handleUniversalFormClose();
  });
  expect(upsert).toHaveBeenCalledTimes(2);
  await act(async () => {
    resolves[0](true);
    await first;
  });
  expect(done).not.toHaveBeenCalled();
  await act(async () => {
    resolves[1](true);
    await second;
  });
  expect(done).toHaveBeenCalledWith("saved", 11);
  expect(sync).toHaveBeenCalledTimes(2);
  vi.restoreAllMocks();
});
it("permits retry after an app-specific submission rejects", async () => {
  const save = vi
    .fn()
    .mockRejectedValueOnce(new Error("save failed"))
    .mockResolvedValueOnce(undefined);
  const done = vi.fn();
  function Harness() {
    model = useAddProviderFormViewModel({
      appId: "opencode",
      onSubmit: save,
      onDone: done,
    });
    return null;
  }
  render(<Harness />);
  await act(async () => {
    await expect(model.handleSubmit(values)).rejects.toThrow("save failed");
  });
  await act(async () => {
    await model.handleSubmit(values);
  });
  expect(save).toHaveBeenCalledTimes(2);
  expect(done).toHaveBeenCalledTimes(1);
});
it("rejects the current app submit while the universal tab owns the surface", async () => {
  const save = vi.fn();
  function Harness() {
    model = useAddProviderFormViewModel({
      appId: "codex",
      onSubmit: save,
      onDone: vi.fn(),
    });
    return null;
  }
  render(<Harness />);
  act(() => model.changeTab("universal"));
  await act(async () => {
    await model.handleSubmit(values);
  });
  expect(save).not.toHaveBeenCalled();
});
it("rejects the current universal save when no modal draft is open", async () => {
  const upsert = vi.spyOn(universalProvidersApi, "upsert");
  render(<ModelHarness done={vi.fn()} />);
  await act(async () => {
    await model.handleUniversalProviderSave({ id: "synthetic" } as any);
  });
  expect(upsert).not.toHaveBeenCalled();
  vi.restoreAllMocks();
});
it("does not duplicate an in-flight app save after hide and reveal", async () => {
  let resolve!: () => void;
  const save = vi.fn(
    () =>
      new Promise<void>((r) => {
        resolve = r;
      }),
  );
  function Harness() {
    model = useAddProviderFormViewModel({
      appId: "opencode",
      onSubmit: save,
      onDone: vi.fn(),
    });
    return null;
  }
  const tree = (active: boolean) => (
    <PreservedView active={active}>
      <Harness />
    </PreservedView>
  );
  const view = render(tree(true));
  let first!: Promise<void>;
  act(() => {
    first = model.handleSubmit(values);
  });
  view.rerender(tree(false));
  view.rerender(tree(true));
  let second!: Promise<void>;
  act(() => {
    second = model.handleSubmit(values);
  });
  expect(save).toHaveBeenCalledTimes(1);
  await act(async () => {
    resolve();
    await Promise.all([first, second]);
  });
});
it("records app save completed while hidden without navigating or allowing its draft to replay", async () => {
  let resolve!: () => void;
  const save = vi.fn(
    () =>
      new Promise<void>((r) => {
        resolve = r;
      }),
  );
  const done = vi.fn();
  function Harness() {
    model = useAddProviderFormViewModel({
      appId: "opencode",
      onSubmit: save,
      onDone: done,
    });
    return null;
  }
  const tree = (active: boolean) => (
    <PreservedView active={active}>
      <Harness />
    </PreservedView>
  );
  const view = render(tree(true));
  let first!: Promise<void>;
  act(() => {
    first = model.handleSubmit(values);
  });
  view.rerender(tree(false));
  await act(async () => {
    resolve();
    await first;
  });
  view.rerender(tree(true));
  expect(done).not.toHaveBeenCalled();
  expect(model.appSaveCompleted).toBe(true);
  await act(async () => {
    await model.handleSubmit(values);
  });
  expect(save).toHaveBeenCalledTimes(1);
});
it("blocks replay in the same turn when a hidden app save resolves after reveal", async () => {
  let resolve!: () => void;
  const save = vi.fn(
    () =>
      new Promise<void>((r) => {
        resolve = r;
      }),
  );
  const done = vi.fn();
  function Harness() {
    model = useAddProviderFormViewModel({
      appId: "opencode",
      onSubmit: save,
      onDone: done,
    });
    return null;
  }
  const tree = (active: boolean) => (
    <PreservedView active={active}>
      <Harness />
    </PreservedView>
  );
  const view = render(tree(true));
  let first!: Promise<void>;
  act(() => {
    first = model.handleSubmit(values);
  });
  view.rerender(tree(false));
  view.rerender(tree(true));
  const current = model.handleSubmit;
  await act(async () => {
    resolve();
    await first;
    void current(values);
  });
  expect(save).toHaveBeenCalledTimes(1);
  expect(done).not.toHaveBeenCalled();
});
it("preserves an open modal on hide but retires callbacks from the previous visible owner", async () => {
  const upsert = vi.spyOn(universalProvidersApi, "upsert");
  const tree = (active: boolean) => (
    <PreservedView active={active}>
      <ModelHarness done={vi.fn()} />
    </PreservedView>
  );
  const view = render(tree(true));
  openModal();
  const stale = model.handleUniversalProviderSave;
  view.rerender(tree(false));
  view.rerender(tree(true));
  expect(model.universalFormOpen).toBe(true);
  await act(async () => {
    await stale({ id: "synthetic" } as any);
  });
  expect(upsert).not.toHaveBeenCalled();
  await act(async () => {
    await model.handleUniversalProviderSave({ id: "synthetic" } as any);
  });
  expect(upsert).toHaveBeenCalledTimes(1);
});
it("does not repeat an accepted universal save across Activity visibility changes", async () => {
  let resolve!: (value: boolean) => void;
  const upsert = vi.spyOn(universalProvidersApi, "upsert").mockImplementation(
    () =>
      new Promise<boolean>((r) => {
        resolve = r;
      }),
  );
  vi.spyOn(universalProvidersApi, "sync").mockResolvedValue(true);
  const tree = (active: boolean) => (
    <PreservedView active={active}>
      <ModelHarness done={vi.fn()} />
    </PreservedView>
  );
  const view = render(tree(true));
  openModal();
  let first!: Promise<void>;
  act(() => {
    first = model.handleUniversalProviderSave({ id: "synthetic" } as any);
  });
  view.rerender(tree(false));
  view.rerender(tree(true));
  let second!: Promise<void>;
  act(() => {
    second = model.handleUniversalProviderSave({ id: "synthetic" } as any);
  });
  expect(upsert).toHaveBeenCalledTimes(1);
  await act(async () => {
    resolve(true);
    await Promise.all([first, second]);
  });
});
