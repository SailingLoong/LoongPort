import { useCallback, useEffect, useRef, useState } from "react";

import type {
  ProviderCatalogDecision,
  SwitchResult,
} from "@/lib/api/providers";
import type { Provider } from "@/types";

import { SwitchTierConfirmDialog } from "./SwitchTierConfirmDialog";

/**
 * 给 provider 页那条通用切换套上「先退 ChatGPT」的确认框。
 *
 * ## 为什么 cc-switch 自带的供应商也需要它
 *
 * ChatGPT 桌面版自带一份 codex 核心、与命令行 codex **共用同一个 `~/.codex`**。所以它在
 * 跑的时候切**任何** codex 供应商都有同一个问题：它启动时读了旧的 `config.toml`，不重启
 * 就仍连旧的；而且**它退出时会回写那个文件**，可能把我们刚写的覆盖掉。
 *
 * 原来只有 `relay_switch_tier`（LoongPort 档位）编排了「退 → 切 → 重开」，于是从
 * provider 页切 cc-switch 自带的供应商时没有这层保护 —— 维护者实测指出的正是这一点。
 * 后端编排已经共用 `chatgpt_app::around`；这个 hook 补的是前端那一半：**问用户**。
 *
 * ## 为什么是 hook + 自带弹窗，而不是把逻辑摊进 `App.tsx`
 *
 * `App.tsx` 是上游文件。把 state、弹窗、判据摊在那里等于把 LoongPort 的实现搬进上游，
 * 将来 merge 上游时那一片全是冲突。收在这里之后 `App.tsx` 只多一行
 * `onSwitch={guardedSwitch}` 与一个 `{switchDialog}`（AGENTS.md §一「改上游文件时
 * 改动面越小越好」）。
 *
 * ## 判据复用 `chatgptNeedsAttention`，与档位切换完全一致
 *
 * 维护者定的：同一个页面两种行为要一致，用户不该需要记住「哪种 provider 会问我」。
 * 那个字段的语义是「这台机器上切换需要管 ChatGPT 吗」——**不是「装了没有」**
 * （非 macOS 查不到，那边恒为 true）。没装的 macOS 用户不会被打扰。
 */
export function useCodexSwitchGuard(
  switchProvider: (
    provider: Provider,
    quitChatgpt?: boolean,
    catalogDecision?: ProviderCatalogDecision,
    catalogPreviewOnly?: boolean,
  ) => Promise<SwitchResult | undefined>,
  disabled = false,
) {
  const [pending, setPending] = useState<
    | ({
        provider: Provider;
      } & Extract<SwitchResult, { status: "confirmationRequired" }>)
    | null
  >(null);
  const [busy, setBusy] = useState(false);
  const inFlight = useRef(false);
  const lifecycle = useRef(0);
  useEffect(
    () => () => {
      lifecycle.current += 1;
    },
    [],
  );
  const switchRef = useRef(switchProvider);
  switchRef.current = switchProvider;
  const disabledRef = useRef(disabled);
  disabledRef.current = disabled;

  const submit = useCallback(
    async (
      provider: Provider,
      quitChatgpt?: boolean,
      catalogDecision?: ProviderCatalogDecision,
      catalogPreviewOnly = false,
    ) => {
      if (disabledRef.current || inFlight.current) return;
      const generation = lifecycle.current;
      inFlight.current = true;
      setBusy(true);
      try {
        const result = catalogDecision
          ? await switchRef.current(
              provider,
              quitChatgpt,
              catalogDecision,
              catalogPreviewOnly,
            )
          : quitChatgpt === undefined
            ? await switchRef.current(provider)
            : await switchRef.current(provider, quitChatgpt);
        if (generation !== lifecycle.current) return;
        if (result?.status === "confirmationRequired") {
          // A preview response must describe the exact choice/version requested.
          if (
            catalogPreviewOnly &&
            (result.catalog?.choice !== catalogDecision?.choice ||
              result.catalog?.revision !== catalogDecision?.revision)
          ) {
            setPending(null);
            return;
          }
          setPending({ provider, ...result });
        } else {
          // Errors are reported by the mutation. Never leave an old consent reusable.
          setPending(null);
        }
      } catch {
        if (generation === lifecycle.current) setPending(null);
      } finally {
        inFlight.current = false;
        if (generation === lifecycle.current) setBusy(false);
      }
    },
    [],
  );
  const guardedSwitch = useCallback(
    (provider: Provider) => submit(provider),
    [submit],
  );
  const cancel = () => {
    lifecycle.current += 1;
    setPending(null);
    setBusy(false);
  };
  const switchDialog = (
    <SwitchTierConfirmDialog
      targetName={pending?.targetName ?? null}
      catalog={pending?.catalog}
      quitRequired={pending?.quitRequired}
      disabled={disabled || busy}
      onCancel={cancel}
      onPreviewCatalog={(decision) => {
        if (!pending?.catalog || decision.revision !== pending.catalog.revision)
          return;
        return submit(pending.provider, undefined, decision, true);
      }}
      onSwitch={(quitChatgpt, decision) => {
        if (!pending) return;
        if (
          pending.catalog &&
          (decision?.choice !== pending.catalog.choice ||
            decision.revision !== pending.catalog.revision)
        )
          return;
        return submit(pending.provider, quitChatgpt, decision);
      }}
    />
  );

  return { guardedSwitch, switchDialog };
}
