import {
  useCallback,
  useImperativeHandle,
  useRef,
  useState,
  type Ref,
} from "react";
import { ManagedAccountProviderSection } from "@/components/settings/ManagedAccountProviderSection";
import { ManagedAccountBootstrapSection } from "@/components/relay/ManagedAccountBootstrapSection";
export interface ManagedAccountApplicationHandle {
  readonly hasPendingChanges: boolean;
  readonly submitting: boolean;
  discard: () => void;
}
/** View-only bridge to the shell's existing leave-draft guard. */
export function ManagedAccountApplicationPanel({
  appId,
  ref,
}: {
  appId?: "claude" | "codex";
  ref?: Ref<ManagedAccountApplicationHandle>;
} = {}) {
  const state = useRef({ provider: [false, false], bootstrap: [false, false] });
  const [revision, setRevision] = useState(0);
  const provider = useCallback((dirty: boolean, submitting: boolean) => {
    state.current.provider = [dirty, submitting];
  }, []);
  const bootstrap = useCallback((dirty: boolean, submitting: boolean) => {
    state.current.bootstrap = [dirty, submitting];
  }, []);
  useImperativeHandle(
    ref,
    () => ({
      get hasPendingChanges() {
        return state.current.provider[0] || state.current.bootstrap[0];
      },
      get submitting() {
        return state.current.provider[1] || state.current.bootstrap[1];
      },
      discard() {
        state.current = { provider: [false, false], bootstrap: [false, false] };
        setRevision((value) => value + 1);
      },
    }),
    [],
  );
  return (
    <div key={revision} className="space-y-4">
      <ManagedAccountProviderSection
        appId={appId}
        onInteractionChange={provider}
      />
      <ManagedAccountBootstrapSection
        appId={appId}
        onInteractionChange={bootstrap}
      />
    </div>
  );
}
