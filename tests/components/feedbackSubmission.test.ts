import { describe, expect, it, vi } from "vitest";
import { createFeedbackSubmission } from "@/components/settings/feedbackSubmission";
import type {
  FeedbackPrepareInput,
  FeedbackPreparedPreview,
} from "@/lib/api/diagnostics";
const input: FeedbackPrepareInput = {
  description: "Synthetic only",
  relatedApps: [],
  includeDiagnostics: false,
  includeSites: false,
  screenshots: [],
};
const preview = {
  previewId: "frozen",
  files: [],
  summary: {},
  totalBytes: 0,
  applicationConfigs: null,
} as unknown as FeedbackPreparedPreview;
const deferred = <T>() => {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((r) => (resolve = r));
  return { promise, resolve };
};
describe("single background feedback owner", () => {
  it("freezes selected input, continues without subscribers and blocks repeated starts", async () => {
    const pending = deferred<FeedbackPreparedPreview>();
    const prepare = vi.fn((_input: FeedbackPrepareInput) => pending.promise);
    const submit = vi.fn().mockResolvedValue({
      status: "accepted",
      localSave: "unavailable",
      receipt: null,
    });
    const owner = createFeedbackSubmission({ prepare, submit });
    const draft = { ...input, relatedApps: [] };
    expect(owner.start(draft)).toBe(true);
    draft.description = "changed";
    expect(owner.start(input)).toBe(false);
    expect(prepare.mock.calls[0][0].description).toBe("Synthetic only");
    pending.resolve(preview);
    await vi.waitFor(() => expect(owner.getSnapshot().phase).toBe("accepted"));
    expect(submit).toHaveBeenCalledExactlyOnceWith({ previewId: "frozen" });
    expect(owner.getSnapshot().unread).toBe(true);
    owner.markRead();
    expect(owner.getSnapshot().unread).toBe(false);
  });
  it("retains a failed prepare for an explicit retry only", async () => {
    const prepare = vi
      .fn()
      .mockRejectedValueOnce(new Error("private"))
      .mockResolvedValue(preview);
    const submit = vi.fn().mockResolvedValue({
      status: "accepted",
      localSave: "stored",
      receipt: null,
    });
    const owner = createFeedbackSubmission({ prepare, submit });
    owner.start(input);
    await vi.waitFor(() => expect(owner.getSnapshot().phase).toBe("failed"));
    expect(submit).not.toHaveBeenCalled();
    expect(owner.retry()).toBe(true);
    await vi.waitFor(() => expect(owner.getSnapshot().phase).toBe("accepted"));
    expect(prepare).toHaveBeenCalledTimes(2);
  });
  it("never automatically retries an uncertain submit", async () => {
    const submit = vi.fn().mockResolvedValue({ status: "unknown" });
    const owner = createFeedbackSubmission({
      prepare: vi.fn().mockResolvedValue(preview),
      submit,
    });
    owner.start(input);
    await vi.waitFor(() => expect(owner.getSnapshot().phase).toBe("unknown"));
    expect(owner.retry()).toBe(false);
    expect(submit).toHaveBeenCalledOnce();
  });
});

it("keeps an accepted upload accepted when saving its local receipt fails", async () => {
  const owner = createFeedbackSubmission({
    prepare: vi.fn().mockResolvedValue(preview),
    submit: vi.fn().mockResolvedValue({
      status: "accepted",
      localSave: "failed",
      receipt: null,
    }),
  });
  owner.start(input);
  await vi.waitFor(() => expect(owner.getSnapshot().phase).toBe("accepted"));
  expect(owner.retry()).toBe(false);
  expect(owner.getSnapshot().result).toMatchObject({ localSave: "failed" });
});

it("does not blindly resend when submit rejects after dispatch", async () => {
  const submit = vi
    .fn()
    .mockRejectedValue(new Error("synthetic transport uncertainty"));
  const owner = createFeedbackSubmission({
    prepare: vi.fn().mockResolvedValue(preview),
    submit,
  });
  owner.start(input);
  await vi.waitFor(() => expect(owner.getSnapshot().phase).toBe("unknown"));
  expect(owner.retry()).toBe(false);
  expect(submit).toHaveBeenCalledOnce();
});
