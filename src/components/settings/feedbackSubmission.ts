import { useSyncExternalStore } from "react";
import { feedbackApi } from "@/lib/api";
import type {
  FeedbackPrepareInput,
  FeedbackPreparedPreview,
  FeedbackSubmitResult,
} from "@/lib/api/diagnostics";

type Phase =
  "idle" | "preparing" | "submitting" | "accepted" | "failed" | "unknown";
interface Submission {
  phase: Phase;
  unread: boolean;
  input: FeedbackPrepareInput | null;
  prepared: FeedbackPreparedPreview | null;
  result: FeedbackSubmitResult | null;
}
type Api = Pick<typeof feedbackApi, "prepare" | "submit">;

/** One in-memory attempt, not a queue. Closing a view does not own its lifetime.
 * App exit is not resumable. User content never enters browser persistent storage. */
export function createFeedbackSubmission(api: Api) {
  let state: Submission = {
    phase: "idle",
    unread: false,
    input: null,
    prepared: null,
    result: null,
  };
  const listeners = new Set<() => void>();
  const publish = (next: Submission) => {
    state = next;
    listeners.forEach((listener) => listener());
  };
  const busy = () =>
    state.phase === "preparing" || state.phase === "submitting";
  const run = async (input: FeedbackPrepareInput) => {
    let submitting = false;
    try {
      const prepared = await api.prepare(input);
      publish({ ...state, phase: "submitting", prepared });
      submitting = true;
      const result = await api.submit({ previewId: prepared.previewId });
      publish({
        ...state,
        phase: result.status === "accepted" ? "accepted" : "unknown",
        result,
        unread: true,
      });
    } catch {
      // A rejected IPC/network call after dispatch may already have uploaded.
      // Never call it a definite failure or automatically upload it again.
      publish({
        ...state,
        phase: submitting ? "unknown" : "failed",
        unread: true,
      });
    }
  };
  const start = (input: FeedbackPrepareInput) => {
    if (busy()) return false;
    const frozen = {
      ...input,
      relatedApps: [...input.relatedApps],
      screenshots: input.screenshots.map((shot) => ({ ...shot })),
    };
    publish({
      phase: "preparing",
      unread: false,
      input: frozen,
      prepared: null,
      result: null,
    });
    void run(frozen);
    return true;
  };
  return {
    start,
    retry: () =>
      state.phase === "failed" && state.input !== null
        ? start(state.input)
        : false,
    markRead: () => {
      if (state.unread) publish({ ...state, unread: false });
    },
    getSnapshot: () => state,
    subscribe: (listener: () => void) => {
      listeners.add(listener);
      return () => {
        listeners.delete(listener);
      };
    },
  };
}
export const feedbackSubmission = createFeedbackSubmission(feedbackApi);
export const useFeedbackSubmission = () =>
  useSyncExternalStore(
    feedbackSubmission.subscribe,
    feedbackSubmission.getSnapshot,
  );
