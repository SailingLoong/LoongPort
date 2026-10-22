import {
  useEffect,
  useLayoutEffect,
  useReducer,
  useRef,
  useState,
} from "react";
import { useTranslation } from "react-i18next";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import { Switch } from "@/components/ui/switch";
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

function confirms(
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
) {
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
    if (target.uncertain && !confirms(value, target.uncertain)) {
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
  }, [providerId, open, disabled]);

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
    if (!session || !context || !canChange || session.busy) return;
    if (enabled && (!canEnable || !declared)) return;
    if (enabled === context.enabled) return;
    session.busy = true;
    session.uncertain = context;
    setDeclaration(null);
    notify();
    try {
      const value = await qoderDailyApi.setAutomatic(context, enabled);
      if (
        confirms(value, context) &&
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

export function QoderAutomaticClaim({
  state,
}: {
  state: ReturnType<typeof useQoderAutomaticClaim>;
}) {
  const { t } = useTranslation();
  return (
    <section className="space-y-3 border-t border-border-default px-6 py-4">
      <div className="flex items-center justify-between gap-3">
        <span className="text-sm font-medium">
          {t("qoderDaily.automatic.title", { defaultValue: "自动签到" })}
        </span>
        <Switch
          aria-label={t("qoderDaily.automatic.title", {
            defaultValue: "自动签到",
          })}
          checked={state.context?.enabled ?? false}
          disabled={!state.canChange}
          onCheckedChange={(checked) => void state.setEnabled(checked)}
        />
      </div>
      <p className="text-xs text-muted-foreground">
        {t("qoderDaily.automatic.window", {
          defaultValue:
            "开启后仅在 LoongPort 运行期间自动处理。以服务端实际活动窗口为准，UTC+8 每日 10:00 为活动边界；不补造活动，也不保证每次新增积分。",
        })}
      </p>
      <p className="text-xs text-muted-foreground">
        {t("qoderDaily.automatic.device", {
          defaultValue:
            "目前仅支持已验证官方组件的 Qoder Desktop 0.4.8（macOS ARM64）。此功能仅管理签到，不代表已接通原生推理消费。",
        })}
      </p>
      {!state.context?.enabled && (
        <label className="flex items-start gap-2 text-sm">
          <Checkbox
            aria-label={t("qoderDaily.automatic.consent", {
              defaultValue: "确认国际版 Qoder PAT 并持续授权自动签到",
            })}
            checked={state.declared}
            disabled={!state.canDeclare}
            onCheckedChange={(value) => state.declare(value === true)}
          />
          <span>
            {t("qoderDaily.automatic.consentDetail", {
              defaultValue:
                "我确认当前凭据是国际版 Qoder PAT，并授权 LoongPort 在运行期间定期查询并领取当前账号的每日活动积分，直到我关闭自动签到或凭据变化。此持续授权会保存，与上方仅本次手动领取的声明独立。",
            })}
          </span>
        </label>
      )}
      {state.context?.subject && (
        <p className="text-xs text-muted-foreground">{state.context.subject}</p>
      )}
      {state.context?.reason && (
        <p role="status" className="text-xs text-muted-foreground">
          {t("qoderDaily.automatic.unavailable", {
            defaultValue:
              "当前无法开启自动签到，请检查已保存凭据与官方设备组件。",
          })}
        </p>
      )}
      {state.unknown ? (
        <p role="alert" className="text-sm">
          {t("qoderDaily.automatic.unknown", {
            defaultValue:
              "自动签到设置结果尚未确认。请刷新状态确认，期间不会重复提交设置。",
          })}
        </p>
      ) : state.readError ? (
        <p role="alert" className="text-sm">
          {t("qoderDaily.automatic.readError", {
            defaultValue: "无法确认自动签到状态，请刷新后再操作。",
          })}
        </p>
      ) : state.busy ? (
        <p role="status" className="text-sm">
          {t("qoderDaily.automatic.saving", {
            defaultValue: "正在保存自动签到授权…",
          })}
        </p>
      ) : null}
      <Button
        type="button"
        size="sm"
        variant="outline"
        disabled={!state.canRefresh}
        onClick={state.refresh}
      >
        {t("qoderDaily.automatic.refresh", {
          defaultValue: "刷新自动签到状态",
        })}
      </Button>
    </section>
  );
}
