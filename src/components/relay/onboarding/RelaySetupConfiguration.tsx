import { useEffect } from "react";
import { useTranslation } from "react-i18next";
import { ArrowLeft, Loader2 } from "lucide-react";
import { SwitchTierConfirmDialog } from "../SwitchTierConfirmDialog";
import { Button } from "@/components/ui/button";
import { getAppDisplayName } from "@/config/appConfig";
import type { AppId } from "@/lib/api/types";
import { isTextEditableTarget } from "@/utils/domUtils";
import type { ConnectedService } from "./useServiceOnboarding";
import { useRelaySetupConfigurationViewModel } from "./useRelaySetupConfigurationViewModel";

/** Preparation is separate from application publication. */
export function RelaySetupConfiguration({
  account,
  sourceAppId,
  onBack,
  onDone,
}: {
  account: ConnectedService;
  sourceAppId: AppId;
  onBack: () => void;
  onDone: () => void;
}) {
  const { t } = useTranslation();
  const vm = useRelaySetupConfigurationViewModel(account, sourceAppId, onDone);
  const text = (key: string, fallback: string) =>
    t(`loongport.setup.${key}`, { defaultValue: fallback });
  const name = (app: AppId) => getAppDisplayName(app, t);

  useEffect(() => {
    const onEscape = (event: KeyboardEvent) => {
      if (
        event.key !== "Escape" ||
        event.defaultPrevented ||
        document.body.style.overflow === "hidden" ||
        isTextEditableTarget(event.target)
      )
        return;
      event.preventDefault();
      if (!vm.busy) onBack();
    };
    document.addEventListener("keydown", onEscape);
    return () => document.removeEventListener("keydown", onEscape);
  }, [onBack, vm.busy]);

  return (
    <div className="mx-auto w-full max-w-2xl pb-6">
      <Button variant="ghost" onClick={onBack} disabled={vm.busy}>
        <ArrowLeft className="h-4 w-4" />
        {t("common.back")}
      </Button>
      <h1 className="mt-5 text-xl font-semibold">
        {text("title", "准备接入配置")}
      </h1>
      <p className="mt-2 text-sm font-medium">{vm.accountLabel}</p>
      {vm.siteOrigin && (
        <p className="mt-1 break-all text-sm text-muted-foreground">
          {vm.siteOrigin}
        </p>
      )}
      <nav
        aria-label={text("stepsLabel", "接入步骤")}
        className="my-5 flex gap-4 text-sm text-muted-foreground"
      >
        <span>{text("stepChoose", "1 · 选择应用与分组")}</span>
        <span>{text("stepPrepare", "2 · 确认准备")}</span>
        <span>{text("stepConnect", "3 · 另行确认接入")}</span>
      </nav>
      {vm.loading && (
        <p role="status" className="my-5 flex items-center gap-2">
          <Loader2 className="h-4 w-4 animate-spin" />
          {text("loading", "正在读取候选与原操作记录…")}
        </p>
      )}
      {vm.readError && (
        <div role="alert" className="my-4 rounded-lg border p-4">
          <p>{text("readFailed", "未能完成读取；已有操作结果仍需核对。")}</p>
          <Button variant="outline" onClick={vm.refreshAll}>
            {text("readAgain", "重新读取")}
          </Button>
        </div>
      )}
      {vm.finishError && (
        <p role="alert" className="my-4 text-sm text-destructive">
          {text(
            "finishFailed",
            "未能完成退出，请重试；接入结果与分享设置未被改写。",
          )}
        </p>
      )}
      {vm.error && (
        <p role="alert" className="my-4 text-sm text-destructive">
          {text(
            `errors.${vm.error}`,
            text(
              "operationUncertain",
              "操作未能确认，请保留原记录并核对结果。",
            ),
          )}
        </p>
      )}

      {vm.selectionVisible && (
        <>
          <p className="text-sm text-muted-foreground">
            {text(
              "selectionHint",
              "每个应用最多选择一个分组，也可以暂不接入。选择与浏览不会创建密钥。",
            )}
          </p>
          {vm.noChoices && (
            <div className="my-5 rounded-lg border p-4">
              <p>{text("empty", "未发现可供这些应用选择的分组")}</p>
              <p className="mt-2 text-sm text-muted-foreground">
                {text(
                  "emptyHint",
                  "这不代表账号没有远端密钥。可检查账号权限或稍后重新读取；这里不会新建远端分组。",
                )}
              </p>
              <Button variant="link" onClick={vm.refreshDiscovery}>
                {text("readAgain", "重新读取")}
              </Button>
            </div>
          )}
          <div className="my-5 divide-y rounded-lg border bg-background px-4">
            {vm.appRows.map(({ app, options, value, hint, disabled }) => {
              return (
                <div key={app} className="py-4">
                  <div className="flex items-center justify-between gap-4">
                    <span className="text-sm font-medium">{name(app)}</span>
                    <select
                      aria-label={name(app)}
                      value={value}
                      disabled={disabled}
                      className="max-w-[65%] rounded-md border bg-background px-3 py-2 text-sm"
                      onChange={(event) =>
                        vm.selectApp(app, event.target.value)
                      }
                    >
                      <option value="">{text("skipApp", "暂不接入")}</option>
                      {options.map((choice) => (
                        <option key={choice.groupId} value={choice.groupId}>
                          {choice.groupName}
                        </option>
                      ))}
                    </select>
                  </div>
                  <p className="mt-2 text-xs text-muted-foreground">
                    {text(hint.key, hint.fallback)}
                  </p>
                </div>
              );
            })}
          </div>
          <div className="flex justify-end gap-3">
            <Button
              variant="outline"
              onClick={() => void vm.finish()}
              disabled={vm.busy}
            >
              {text("skip", "暂不接入")}
            </Button>
            <Button disabled={!vm.reviewEnabled} onClick={vm.reviewSelection}>
              {text("review", "下一步：确认准备")}
            </Button>
          </div>
        </>
      )}

      {vm.reviewing && (
        <section
          className="my-5 rounded-lg border bg-background p-5"
          aria-label={text("reviewTitle", "本次准备范围")}
        >
          <h2 className="font-semibold">
            {text("reviewTitle", "本次准备范围")}
          </h2>
          <ul className="my-4 space-y-2 text-sm">
            {vm.reviewRows.map((row) => (
              <li key={row.app}>
                {name(row.app)} · {row.groupName}
              </li>
            ))}
          </ul>
          <p className="text-sm text-muted-foreground">
            {text(
              "prepareImpact",
              "只为以上选择检查托管密钥并准备本地配置；同一分组跨应用共享一次密钥准备。未选分组不会处理。",
            )}
          </p>
          <p className="mt-2 text-sm text-muted-foreground">
            {text(
              "compatibilityUnknown",
              "应用兼容性尚未验证；密钥可能已准备，但该应用仍无法接入。",
            )}
          </p>
          <p className="mt-2 text-sm text-muted-foreground">
            {text(
              "noPublication",
              "不会切换当前应用、改写现用配置或 MCP，也不会把新配置自动加入模型池。接入应用需要下一步单独确认。",
            )}
          </p>
          <label className="my-5 flex items-start gap-2 text-sm">
            <input
              type="checkbox"
              checked={vm.acknowledged}
              disabled={vm.busy}
              onChange={(event) => vm.setAcknowledged(event.target.checked)}
            />
            <span>
              {text("prepareAck", "我已了解本次只准备所选配置，不会切换应用")}
            </span>
          </label>
          <div className="flex justify-end gap-3">
            <Button
              variant="outline"
              disabled={vm.busy}
              onClick={vm.cancelReview}
            >
              {t("common.cancel")}
            </Button>
            <Button
              disabled={vm.prepareDisabled}
              onClick={() => void vm.confirmPreparation()}
            >
              {vm.busy && <Loader2 className="h-4 w-4 animate-spin" />}
              {text("confirmPrepare", "确认并准备")}
            </Button>
          </div>
        </section>
      )}

      {vm.resultVisible && (
        <section
          className="my-5 space-y-4"
          aria-label={text("resultsTitle", "原操作结果")}
        >
          <h2 className="font-semibold">
            {text("resultsTitle", "原操作结果")}
          </h2>
          {vm.operationId && (
            <>
              <p className="break-all text-xs text-muted-foreground">
                {vm.operationId}
              </p>
              <div className="divide-y rounded-lg border bg-background px-4">
                {vm.resultRows.map((row) => (
                  <div key={row.app} className="py-4 text-sm">
                    <p className="font-medium">
                      {name(row.app)} · {row.groupName}
                    </p>
                    <p role="status" className="mt-2 text-sm">
                      {text(row.result.key, row.result.fallback)}
                    </p>
                    <p className="mt-1 text-xs text-muted-foreground">
                      {text(row.origin.key, row.origin.fallback)}
                    </p>
                    {row.canConnect && (
                      <Button
                        className="mt-3"
                        variant="outline"
                        disabled={vm.busy}
                        onClick={() => vm.openConnectionReview(row.app)}
                      >
                        {t("loongport.setup.connectApp", {
                          defaultValue: `接入 ${name(row.app)}`,
                          app: name(row.app),
                        })}
                      </Button>
                    )}
                    {row.unsupported && (
                      <p className="mt-2 text-xs text-destructive">
                        {text(
                          "unsupported",
                          "实际模型目录不支持该应用；保留已有资源，不自动替换分组。",
                        )}
                      </p>
                    )}
                  </div>
                ))}
              </div>
              <p className="text-xs text-muted-foreground">
                {text(
                  "preparedHint",
                  "“已准备”只表示本地配置存在，不代表密钥已验证、服务在线或应用已经接入。",
                )}
              </p>
            </>
          )}
          {vm.connectionReviewApp && (
            <section className="rounded-lg border bg-background p-5">
              <h2 className="font-semibold">
                {text("connectionTitle", "确认接入应用")}
              </h2>
              <p className="mt-2 text-sm font-medium">
                {name(vm.connectionReviewApp)}
              </p>
              <p className="mt-2 text-sm text-muted-foreground">
                {text(
                  "connectionImpact",
                  "将通过应用原有配置入口启用本次准备的配置，替换该应用当前选择；原入口负责相关配置文件与 MCP。不会修改其他应用、重新创建密钥或改变已准备的分组。",
                )}
              </p>
              <p className="mt-2 text-sm text-muted-foreground">
                {text(
                  "connectionLiveHint",
                  "配置变更不等于网络可用；运行中的应用可能需要重新加载。若需退出 ChatGPT 或处理已有模型目录，将再次说明并由你选择。",
                )}
              </p>
              <label className="my-4 flex items-start gap-2 text-sm">
                <input
                  type="checkbox"
                  checked={vm.connectionAcknowledged}
                  disabled={vm.busy}
                  onChange={(event) =>
                    vm.setConnectionAcknowledged(event.target.checked)
                  }
                />
                {text("connectionAck", "我已了解将替换此应用的当前配置")}
              </label>
              <div className="flex justify-end gap-3">
                <Button
                  variant="outline"
                  disabled={vm.busy}
                  onClick={vm.cancelConnection}
                >
                  {t("common.cancel")}
                </Button>
                <Button
                  disabled={vm.connectionDisabled}
                  onClick={vm.beginConnection}
                >
                  {text("confirmConnect", "确认接入")}
                </Button>
              </div>
            </section>
          )}
          {vm.needsReconciliation && (
            <p className="text-sm">
              {text(
                "reconcileHint",
                "请先核对原操作。即使列表暂未出现密钥，也不会自动再创建一次。",
              )}
            </p>
          )}
          <div className="flex flex-wrap justify-end gap-3">
            <Button variant="outline" disabled={vm.busy} onClick={vm.reconcile}>
              {text("reconcile", "核对原操作")}
            </Button>
            {vm.retryVisible && (
              <Button
                variant="outline"
                disabled={vm.busy}
                onClick={vm.startRetry}
              >
                {text("retryFailed", "仅重试未完成配置")}
              </Button>
            )}
            <Button
              variant="outline"
              disabled={vm.newDraftDisabled}
              onClick={vm.newDraft}
            >
              {text("newDraft", "重新选择")}
            </Button>
            <Button disabled={vm.busy} onClick={() => void vm.finish()}>
              {text("returnAccount", "结束并返回")}
            </Button>
          </div>
        </section>
      )}
      <SwitchTierConfirmDialog
        targetName={
          vm.dialogTargetApp
            ? (vm.dialogTargetName ?? name(vm.dialogTargetApp))
            : null
        }
        disabled={vm.connectionDisabled}
        catalog={vm.dialogCatalog}
        quitRequired={vm.dialogQuitRequired}
        onCancel={vm.cancelDialog}
        onPreviewCatalog={vm.previewCatalog}
        onSwitch={vm.switchConnection}
      />
    </div>
  );
}
