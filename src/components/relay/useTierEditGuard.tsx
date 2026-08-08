import { useCallback, useState } from "react";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";

import { ConfirmDialog } from "@/components/ConfirmDialog";
import { EditProviderDialog } from "@/components/providers/EditProviderDialog";
import { providersApi } from "@/lib/api";
import type { AppId } from "@/lib/api";
import type { Provider } from "@/types";

/**
 * 这个 hook 需要被编辑对象提供的**全部**信息。
 *
 * 有意收窄成三个字段而不是吃整个 `TierInfo`：官网直连行（`VendorAccountRow`）
 * 走的是同一套编辑流程，但它不是档位、没有 `TierInfo` 那些字段（倍率、分组名、
 * websiteUrl）。收窄之后两类行都传得进来，且不必为 vendor 造一个假 `TierInfo`
 * ——那种造法会让「哪些字段是真的」变得不可判。
 *
 * `TierInfo` 与 `VendorAccountRow` 都结构性满足它，不需要各自加 `implements`。
 */
export interface EditableTier {
  /** 要编辑的那条 provider 记录的 id。 */
  providerId: string;
  /** 弹窗标题里显示的名字。 */
  displayName: string;
  /** 是不是当前 tab 正在用的那条 —— 决定要不要多给一句 ChatGPT 重启提示。 */
  isCurrent: boolean;
  /**
   * 警告文案用哪一套。**只影响文案，流程完全相同。**
   *
   * - `"tier"`（默认）：中转站档位 —— 文案提「档位」与「刷新分组」。
   * - `"vendor"`：官网直连账号 —— 它不是档位、没有分组，且一行对应六个平台，
   *   所以文案要说「这个账号在当前这个应用下的配置」。
   *
   * 沿用同一套文案会让官网行的用户看到「刷新分组不会覆盖它」——而那个动作
   * 在他那一行压根不存在（他按的是「获取密钥」）。
   */
  kind?: "tier" | "vendor";
}

/**
 * 「编辑档位配置」的完整流程：**先警告 → 再开 cc-switch 的编辑页 → 保存后刷新**。
 *
 * ## 为什么编辑页是复用的，不是自己做一个
 *
 * `EditProviderDialog` 已经支持全部字段（各 CLI 的表单分支、通用配置片段、
 * 自定义端点、模型目录）。托管档位就是一条正常的 provider 记录
 * （`category = "aggregator"`、写在同一张表里），那页直接就能编辑它 ——
 * 自己再做一个表单等于把上游那套维护责任接过来（CLAUDE.md §一）。
 *
 * ## 为什么必须先弹警告
 *
 * 保存之后这个档位的**维护责任就转移给用户了**：后续「获取密钥」只会更新它的
 * 密钥，不再覆盖其它内容（`provision::patch_api_key`）。这是件用户必须知情的事 ——
 * 他可能只是想看一眼配置，而保存下去就等于接手了。
 *
 * 警告里同时给出**退路**（「恢复默认配置」），因为最常见的坏结局是「改完连不上」，
 * 而那时用户需要知道有一键回退，而不是自己去猜哪个字段改错了。
 *
 * 维护者的原话：「用户点击编辑按钮的时候，最好先弹窗警告一下，编辑成功后不会参与
 * 自动刷新。如果编辑后无法访问，建议恢复默认设置。然后用户点确认才继续。」
 *
 * ## 为什么是 hook + 自带两个弹窗
 *
 * 与 [`useCodexSwitchGuard`] 同一个理由：这套「先警告再执行」的流程是**一个单位**
 * （弹窗文案 + 确认状态 + 真正的动作），摊在宿主里会让文案与它守的那个动作分开维护。
 * 收成 hook 后宿主只多两行：`onEditTier={requestEdit}` 与 `{editDialogs}`。
 */
export function useTierEditGuard(
  appId: AppId,
  onSaved: () => void | Promise<void>,
) {
  const { t } = useTranslation();
  // 待确认的档位（警告弹窗阶段）。
  const [pending, setPending] = useState<EditableTier | null>(null);
  // 正在编辑的 provider（已确认，编辑页阶段）。
  const [editing, setEditing] = useState<Provider | null>(null);

  const requestEdit = useCallback((tier: EditableTier) => {
    setPending(tier);
  }, []);

  /**
   * 用户在警告里点了「继续编辑」：取出那条 provider 记录，开编辑页。
   *
   * **必须现查一次**（`providersApi.getAll`）而不是让 `TierInfo` 带着整个
   * `Provider` 过来：`TierInfo` 是「只读本地、给列表用」的轻契约（后端注释钉过），
   * 往里塞一整个 provider 会让每次列表刷新都多搬一份完整配置。
   * 而编辑是低频动作，多一次查询无所谓。
   */
  const confirmEdit = useCallback(
    async (tier: EditableTier) => {
      setPending(null);
      try {
        const all = await providersApi.getAll(appId);
        const provider = all[tier.providerId];
        if (!provider) {
          // 档位在列表里、DB 里却没有 ⇒ 数据不一致（多半是刚被别处删了）。
          // 明确报出来，别开一个空表单让用户对着填。
          toast.error(t("loongport.tier.editMissing"));
          return;
        }
        setEditing(provider);
      } catch (e) {
        toast.error(String(e));
      }
    },
    [appId, t],
  );

  /**
   * 编辑页点保存。
   *
   * `originalId` 由 `EditProviderDialog` 传（它给的就是 `provider.id`），
   * 后端据此判「改名了没有」—— 托管档位不许改 id（改了会脱管或伪装，
   * 见 `update_provider_internal`），而内容编辑是放开的。
   *
   * ## ⚠️ 保存失败必须**把错误抛出去**，不能只 toast（review 抓出）
   *
   * 初版在这里 `catch` 掉了，注释还写着「不关弹窗 —— 保存失败时关掉等于把用户的
   * 编辑扔了」。**那句话做不到**：`EditProviderDialog` 是
   * `await onSubmit(...); onOpenChange(false);`（`EditProviderDialog.tsx:211-215`）
   * —— 吃掉错误让 promise 正常 resolve ⇒ 那行 `onOpenChange(false)` 照样执行 ⇒
   * 弹窗关闭、用户刚敲的一屏配置全丢，只留一个 toast。
   *
   * `throw` 之后 promise 变 rejected，`onOpenChange(false)` 就不会执行，
   * 弹窗留在原地、内容还在，用户可以改一下再存 —— 那才是注释说的行为。
   *
   * toast 仍然要发：抛出去的错误上游没人再展示它（`EditProviderDialog`
   * 不 catch），只抛不 toast 用户就只看到弹窗不关、不知道为什么。
   */
  const handleSubmit = useCallback(
    async ({
      provider,
      originalId,
    }: {
      provider: Provider;
      originalId?: string;
    }) => {
      try {
        await providersApi.update(provider, appId, originalId);
      } catch (e) {
        toast.error(String(e));
        // 见上：不 throw 的话 `EditProviderDialog` 会照常关掉弹窗。
        throw e;
      }
      setEditing(null);
      // 刷新：配置变了，「已手动维护」那个标记要跟着变（后端每次现算）。
      await onSaved();
    },
    [appId, onSaved],
  );

  const editDialogs = (
    <>
      <ConfirmDialog
        isOpen={pending !== null}
        // `info` 而不是 `destructive`：编辑本身不破坏什么（有一键回退），
        // 用红色警告图标会让用户以为点下去要出事。
        variant="info"
        title={t("loongport.tier.editConfirmTitle", {
          name: pending?.displayName ?? "",
        })}
        // 编辑**当前正在用**的档位时多给一句。
        //
        // 保存会连带重写 live 的 `config.toml`（上游 `ProviderService::update`
        // 对 current provider 会 `write_live_with_common_config`）—— 而
        // **那条路没有「退 ChatGPT → 写 → 重开」的编排**（只有 `switch_provider`
        // 有）。ChatGPT 桌面版与命令行 codex 共用 `~/.codex`，它在跑的时候
        // 退出时会回写 config.toml ⇒ 用户改完可能发现没生效，且不会收到任何提示。
        //
        // 为什么是提示而不是在后端也套一层 `chatgpt_app::around`：那要改上游的
        // `update_provider`（扩大 merge 接触面），而且会让**每一次**编辑都弹
        // 「要不要退出 ChatGPT」——包括编辑非当前档位（那种情况根本不写 live）。
        // 一句话说清 + 用户自己决定要不要重启，成本与收益匹配（尺子2）。
        // 主文案按 `kind` 取（见 `EditableTier.kind`）。ChatGPT 那句附注两类共用 ——
        // 它讲的是 ChatGPT 桌面版会回写 `~/.codex`，与「被编辑的是档位还是官网账号」无关。
        message={(() => {
          const main =
            pending?.kind === "vendor"
              ? t("loongport.vendor.editConfirmMessage")
              : t("loongport.tier.editConfirmMessage");
          return pending?.isCurrent
            ? `${main}\n\n${t("loongport.tier.editCurrentNote")}`
            : main;
        })()}
        confirmText={t("loongport.tier.editConfirmButton")}
        onConfirm={() => pending && void confirmEdit(pending)}
        onCancel={() => setPending(null)}
      />

      {/* 编辑页。**`key` 绑 provider id** —— 上游那个组件用 `useEffect` 按
          `provider` 初始化表单，不换 key 的话连续编辑两个档位会残留上一个的值。 */}
      {editing && (
        <EditProviderDialog
          key={editing.id}
          open
          provider={editing}
          appId={appId}
          onOpenChange={(open) => {
            if (!open) setEditing(null);
          }}
          onSubmit={handleSubmit}
        />
      )}
    </>
  );

  return { requestEdit, editDialogs };
}
