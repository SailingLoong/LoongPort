import { useQoderCommandAuthority } from "./useQoderCommandAuthority";
import {
  useEffect,
  useLayoutEffect,
  useReducer,
  useRef,
  useState,
} from "react";
import {
  qoderDailyApi,
  type QoderAutomaticContext,
} from "@/lib/api/qoderDaily";

type Session = {
  providerId: string;
  context: QoderAutomaticContext | null;
  uncertain: QoderAutomaticContext | null;
  busy: boolean;
  loading: boolean;
  readError: boolean;
  readSequence: number;
};

export function confirmsQoderAutomaticWrite(
  value: QoderAutomaticContext,
  previous: QoderAutomaticContext,
) {
  return (
    value.providerId === previous.providerId &&
    value.credentialGeneration === previous.credentialGeneration &&
    value.policyVersion === previous.policyVersion &&
    value.revision > previous.revision
  );
}

// Kept by the existing dialog's owner, not its unmounted contents: closing the
// dialog must not forget an in-flight write or turn uncertain consent into off.
export function useQoderAutomaticClaim(
  providerId: string,
  open: boolean,
  disabled: boolean,
  sourceGeneration?: string,
) {
  const authority = useQoderCommandAuthority();
  const sessions = useRef(new Map<string, Session>());
  const mounted = useRef(true);
  const [, redraw] = useReducer((value: number) => value + 1, 0);
  const [declaration, setDeclaration] = useState<QoderAutomaticContext | null>(
    null,
  );
  const session = sessions.current.get(providerId);
  const context = session?.context ?? null;
  const notify = () => {
    if (mounted.current) redraw();
  };
  useEffect(() => {
    mounted.current = true;
    return () => {
      mounted.current = false;
    };
  }, []);

  function acceptRead(target: Session, value: QoderAutomaticContext) {
    if (value.providerId !== target.providerId) {
      target.readError = true;
      return;
    }
    const generationChanged =
      target.context &&
      value.credentialGeneration !== target.context.credentialGeneration;
    if (
      target.uncertain &&
      !confirmsQoderAutomaticWrite(value, target.uncertain)
    ) {
      // A recreated owner can start at revision zero. Show its current state,
      // but never treat another generation as proof of the old write's result.
      if (generationChanged) {
        target.context = value;
        target.readError = false;
      }
      return;
    }
    // Revisions order snapshots only within one credential generation.
    if (
      target.context &&
      !generationChanged &&
      value.revision < target.context.revision
    )
      return;
    target.context = value;
    target.uncertain = null;
    target.readError = false;
  }

  async function read(target: Session) {
    const sequence = ++target.readSequence;
    target.loading = true;
    target.readError = false;
    notify();
    try {
      const value = await qoderDailyApi.automaticContext(target.providerId);
      if (target.readSequence === sequence) acceptRead(target, value);
    } catch {
      if (target.readSequence === sequence) target.readError = true;
    } finally {
      if (target.readSequence === sequence) target.loading = false;
      notify();
    }
  }

  useLayoutEffect(() => {
    setDeclaration(null);
    if (!open || disabled) return;
    let target = sessions.current.get(providerId);
    if (!target) {
      target = {
        providerId,
        context: null,
        uncertain: null,
        busy: false,
        loading: false,
        readError: false,
        readSequence: 0,
      };
      sessions.current.set(providerId, target);
    }
    void read(target);
    // This is a local snapshot read on explicit opening, not scheduler work.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [providerId, open, disabled, sourceGeneration]);

  const blocked =
    !open ||
    disabled ||
    !context ||
    !!session?.busy ||
    !!session?.loading ||
    !!session?.readError ||
    !!session?.uncertain;
  const declared = !!context && declaration === context;
  const canEnable = !blocked && context?.canEnable === true;
  const canChange = !blocked && (context?.enabled || (canEnable && declared));

  async function setEnabled(enabled: boolean) {
    if (
      !authority.isCurrent() ||
      !session ||
      !context ||
      !canChange ||
      session.busy
    )
      return;
    if (enabled && (!canEnable || !declared)) return;
    if (enabled === context.enabled) return;
    session.busy = true;
    session.uncertain = context;
    setDeclaration(null);
    notify();
    try {
      const value = await qoderDailyApi.setAutomatic(context, enabled);
      if (
        confirmsQoderAutomaticWrite(value, context) &&
        (!session.context ||
          (session.context.credentialGeneration ===
            context.credentialGeneration &&
            value.revision >= session.context.revision))
      ) {
        session.context = value;
        session.uncertain = null;
        session.readError = false;
        // Older in-flight context reads cannot overwrite this write response.
        ++session.readSequence;
        session.loading = false;
      }
    } catch {
      // A transport error does not prove the write failed. Query once; never
      // retry the setter, including when its readback is old or unavailable.
      session.uncertain = context;
      await read(session);
    } finally {
      session.busy = false;
      notify();
    }
  }

  return {
    context,
    declared,
    busy: session?.busy ?? false,
    loading: session?.loading ?? false,
    unknown: !!session?.uncertain && !session.busy,
    readError: session?.readError ?? false,
    canDeclare: canEnable,
    canChange: !!canChange,
    canRefresh: open && !disabled && !session?.busy && !session?.loading,
    declare: (checked: boolean) =>
      setDeclaration(checked && canEnable ? context : null),
    setEnabled,
    refresh: () => {
      setDeclaration(null);
      if (session && open && !disabled && !session.busy && !session.loading)
        void read(session);
    },
  };
}
