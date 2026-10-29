import { useEffect, useRef, useState } from "react";
import { useQueryClient } from "@tanstack/react-query";
import { serviceOnboardingApi } from "@/lib/api/serviceOnboarding";
import { serviceOnboardingKey } from "@/lib/query/serviceOnboarding";
import { useTranslation } from "react-i18next";
import { ArrowLeft, Loader2 } from "lucide-react";
import { SwitchTierConfirmDialog } from "../SwitchTierConfirmDialog";
import { Button } from "@/components/ui/button";
import { getAppDisplayName, PROVIDER_STORE_APP_IDS } from "@/config/appConfig";
import type { AppId } from "@/lib/api/types";
import { isTextEditableTarget } from "@/utils/domUtils";
import type { ConnectedService } from "./useServiceOnboarding";
import { useRelaySetup } from "./useRelaySetup";

/** Preparation is separate from application publication. This page never calls
 * the old refresh/switch/complete shortcuts to manufacture a successful setup. */
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
  const setup = useRelaySetup(account.rowId);
  const client = useQueryClient();
  const finishing = useRef(false);
  const [leaving, setLeaving] = useState(false);
  const [finishError, setFinishError] = useState(false);
  const busy = setup.busy || leaving;
  const finish = async () => {
    if (busy || finishing.current) return;
    finishing.current = true;
    setLeaving(true);
    setFinishError(false);
    try {
      // Explicit exit only; dismissal preserves existing sharing preferences.
      // Preparation/application acknowledgement is never telemetry consent.
      await serviceOnboardingApi.dismiss();
      await client.invalidateQueries({ queryKey: serviceOnboardingKey });
      onDone();
    } catch {
      setFinishError(true);
    } finally {
      finishing.current = false;
      setLeaving(false);
    }
  };
  const [acknowledged, setAcknowledged] = useState(false);
  const [connectionAcknowledged, setConnectionAcknowledged] = useState(false);
  const [connectionDialog, setConnectionDialog] = useState(false);
  const [retryApps, setRetryApps] = useState<AppId[]>([]);
  const text = (key: string, fallback: string) =>
    t(`loongport.setup.${key}`, { defaultValue: fallback });
  const name = (app: AppId) => getAppDisplayName(app, t);
  const choices = setup.discovery.data?.choices ?? [];
  const apps = [
    ...new Set([
      sourceAppId,
      ...PROVIDER_STORE_APP_IDS.filter((app) =>
        choices.some((choice) => choice.app === app),
      ),
    ]),
  ];
  const reviewing = setup.phase === "review" || retryApps.length > 0;
  const selected = retryApps.length
    ? (setup.receipt?.selections.filter((selection) =>
        retryApps.includes(selection.app),
      ) ?? [])
    : (setup.reviewed?.selections ?? []);

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
      if (!busy) onBack();
    };
    document.addEventListener("keydown", onEscape);
    return () => document.removeEventListener("keydown", onEscape);
  }, [onBack, busy]);

  return (
    <div className="mx-auto w-full max-w-2xl pb-6">
      <Button variant="ghost" onClick={onBack} disabled={busy}>
        <ArrowLeft className="h-4 w-4" />
        {t("common.back")}
      </Button>
      <h1 className="mt-5 text-xl font-semibold">
        {text("title", "准备接入配置")}
      </h1>
      <p className="mt-2 text-sm font-medium">
        {setup.discovery.data?.account.accountLabel ?? account.name}
      </p>
      {setup.discovery.data && (
        <p className="mt-1 break-all text-sm text-muted-foreground">
          {setup.discovery.data.account.siteOrigin}
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
      {(setup.discovery.isPending || setup.recovery.isPending) && (
        <p role="status" className="my-5 flex items-center gap-2">
          <Loader2 className="h-4 w-4 animate-spin" />
          {text("loading", "正在读取候选与原操作记录…")}
        </p>
      )}
      {(setup.discovery.isError || setup.recovery.isError) && (
        <div role="alert" className="my-4 rounded-lg border p-4">
          <p>{text("readFailed", "未能完成读取；已有操作结果仍需核对。")}</p>
          <Button
            variant="outline"
            onClick={() => {
              void setup.discovery.refetch();
              void setup.recovery.refetch();
            }}
          >
            {text("readAgain", "重新读取")}
          </Button>
        </div>
      )}
      {finishError && (
        <p role="alert" className="my-4 text-sm text-destructive">
          {text(
            "finishFailed",
            "未能完成退出，请重试；接入结果与分享设置未被改写。",
          )}
        </p>
      )}
      {setup.error && !setup.recovery.isError && (
        <p role="alert" className="my-4 text-sm text-destructive">
          {text(
            `errors.${setup.error}`,
            text(
              "operationUncertain",
              "操作未能确认，请保留原记录并核对结果。",
            ),
          )}
        </p>
      )}

      {setup.phase === "selection" &&
        !reviewing &&
        setup.discovery.isSuccess && (
          <>
            <p className="text-sm text-muted-foreground">
              {text(
                "selectionHint",
                "每个应用最多选择一个分组，也可以暂不接入。选择与浏览不会创建密钥。",
              )}
            </p>
            {!choices.length && (
              <div className="my-5 rounded-lg border p-4">
                <p>{text("empty", "未发现可供这些应用选择的分组")}</p>
                <p className="mt-2 text-sm text-muted-foreground">
                  {text(
                    "emptyHint",
                    "这不代表账号没有远端密钥。可检查账号权限或稍后重新读取；这里不会新建远端分组。",
                  )}
                </p>
                <Button
                  variant="link"
                  onClick={() => void setup.discovery.refetch()}
                >
                  {text("readAgain", "重新读取")}
                </Button>
              </div>
            )}
            <div className="my-5 divide-y rounded-lg border bg-background px-4">
              {apps.map((app) => {
                const available = choices.filter(
                  (choice) => choice.app === app,
                );
                const chosen = available.find(
                  (choice) => choice.groupId === setup.draft[app],
                );
                return (
                  <div key={app} className="py-4">
                    <div className="flex items-center justify-between gap-4">
                      <span className="text-sm font-medium">{name(app)}</span>
                      <select
                        aria-label={name(app)}
                        value={setup.draft[app] ?? ""}
                        disabled={busy || !available.length}
                        className="max-w-[65%] rounded-md border bg-background px-3 py-2 text-sm"
                        onChange={(event) =>
                          setup.setDraft({
                            ...setup.draft,
                            [app]: event.target.value,
                          })
                        }
                      >
                        <option value="">{text("skipApp", "暂不接入")}</option>
                        {available.map((choice) => (
                          <option key={choice.groupId} value={choice.groupId}>
                            {choice.groupName}
                          </option>
                        ))}
                      </select>
                    </div>
                    <p className="mt-2 text-xs text-muted-foreground">
                      {!available.length
                        ? text(
                            "appEmpty",
                            "当前应用暂无可选分组，可暂不接入并继续选择其他应用。",
                          )
                        : chosen?.localConfiguration === "present"
                          ? text(
                              "existingHint",
                              "已有本地配置，将保持原样复用；不代表已校验密钥或在线可用。",
                            )
                          : text(
                              "coldHint",
                              "远端密钥状态未知。确认后才核对可认领的托管密钥，必要时按需创建。",
                            )}
                    </p>
                  </div>
                );
              })}
            </div>
            <div className="flex justify-end gap-3">
              <Button
                variant="outline"
                onClick={() => void finish()}
                disabled={busy}
              >
                {text("skip", "暂不接入")}
              </Button>
              <Button
                disabled={
                  busy ||
                  !setup.recovery.isSuccess ||
                  !Object.values(setup.draft).some(Boolean) ||
                  setup.needsReconciliation
                }
                onClick={() => {
                  setAcknowledged(false);
                  setup.reviewSelection();
                }}
              >
                {text("review", "下一步：确认准备")}
              </Button>
            </div>
          </>
        )}

      {reviewing && (
        <section
          className="my-5 rounded-lg border bg-background p-5"
          aria-label={text("reviewTitle", "本次准备范围")}
        >
          <h2 className="font-semibold">
            {text("reviewTitle", "本次准备范围")}
          </h2>
          <ul className="my-4 space-y-2 text-sm">
            {selected.map((selection) => (
              <li key={selection.app}>
                {name(selection.app)} ·{" "}
                {choices.find(
                  (choice) =>
                    choice.app === selection.app &&
                    choice.groupId === selection.groupId,
                )?.groupName ??
                  setup.receipt?.groups.find(
                    (group) => group.groupId === selection.groupId,
                  )?.groupName ??
                  selection.groupId}
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
              checked={acknowledged}
              disabled={busy}
              onChange={(event) => setAcknowledged(event.target.checked)}
            />
            <span>
              {text("prepareAck", "我已了解本次只准备所选配置，不会切换应用")}
            </span>
          </label>
          <div className="flex justify-end gap-3">
            <Button
              variant="outline"
              disabled={busy}
              onClick={() => {
                setRetryApps([]);
                setup.cancelDraft();
              }}
            >
              {t("common.cancel")}
            </Button>
            <Button
              disabled={!acknowledged || busy}
              onClick={async () => {
                if (retryApps.length) {
                  await setup.resumeFailed(retryApps);
                  setRetryApps([]);
                } else await setup.confirmSelection();
              }}
            >
              {busy && <Loader2 className="h-4 w-4 animate-spin" />}
              {text("confirmPrepare", "确认并准备")}
            </Button>
          </div>
        </section>
      )}

      {setup.phase === "result" && !reviewing && (
        <section
          className="my-5 space-y-4"
          aria-label={text("resultsTitle", "原操作结果")}
        >
          <h2 className="font-semibold">
            {text("resultsTitle", "原操作结果")}
          </h2>
          {setup.receipt && (
            <>
              <p className="break-all text-xs text-muted-foreground">
                {setup.receipt.operationId}
              </p>
              <div className="divide-y rounded-lg border bg-background px-4">
                {setup.receipt.targets.map((target) => {
                  const group = setup.receipt!.groups.find(
                    (group) => group.groupId === target.groupId,
                  );
                  const localReady =
                    target.configuration === "prepared" ||
                    target.configuration === "existing";
                  return (
                    <div key={target.app} className="py-4 text-sm">
                      <p className="font-medium">
                        {name(target.app)} ·{" "}
                        {group?.groupName ?? target.groupId}
                      </p>
                      <p role="status" className="mt-2">
                        {target.connection.state === "applied"
                          ? text(
                              "applied",
                              "已记录本次接入成功；当前状态可核对",
                            )
                          : target.connection.state === "failedBeforeWrite"
                            ? text(
                                "connectionNotApplied",
                                "本次未执行接入，请重新检查接入影响",
                              )
                            : target.connection.state === "changed"
                              ? text(
                                  "connectionChanged",
                                  "应用配置已变化，请重新检查接入影响",
                                )
                              : ["unknown", "applying"].includes(
                                    target.connection.state,
                                  )
                                ? target.connection.observedCurrent
                                  ? text(
                                      "currentlySelected",
                                      "上次核对时配置指向此项，但原接入操作结果仍待核对",
                                    )
                                  : text(
                                      "connectionUnknown",
                                      "接入结果待核对，不会自动重复应用",
                                    )
                                : localReady
                                  ? text(
                                      "prepared",
                                      "本地配置已准备；本次尚未执行应用接入",
                                    )
                                  : group?.state === "ready" &&
                                      target.configuration === "failed"
                                    ? text(
                                        "keyOnly",
                                        "密钥已准备，本地配置尚未完成",
                                      )
                                    : target.configuration === "failed"
                                      ? text("failed", "本次未完成配置准备")
                                      : text(
                                          "uncertain",
                                          "结果待核对，不会自动重复创建",
                                        )}
                      </p>
                      <p className="mt-1 text-xs text-muted-foreground">
                        {group?.origin === "created"
                          ? text("keyCreated", "已记录本次创建的密钥事实")
                          : group?.origin === "reused"
                            ? text("keyReused", "复用符合托管条件的已有密钥")
                            : group?.origin === "observed"
                              ? text(
                                  "keyObserved",
                                  "核对时发现托管密钥，未归因为本次创建",
                                )
                              : text("keyUnknown", "本次未确认远端密钥状态")}
                      </p>
                      {localReady &&
                        !["applied", "applying", "unknown"].includes(
                          target.connection.state,
                        ) &&
                        target.compatibility !== "unsupported" && (
                          <Button
                            className="mt-3"
                            variant="outline"
                            disabled={busy}
                            onClick={() => {
                              setConnectionAcknowledged(false);
                              setConnectionDialog(false);
                              void setup.reviewConnection(target.app);
                            }}
                          >
                            {t("loongport.setup.connectApp", {
                              defaultValue: `接入 ${name(target.app)}`,
                              app: name(target.app),
                            })}
                          </Button>
                        )}
                      {target.compatibility === "unsupported" && (
                        <p className="mt-2 text-xs text-destructive">
                          {text(
                            "unsupported",
                            "实际模型目录不支持该应用；保留已有资源，不自动替换分组。",
                          )}
                        </p>
                      )}
                    </div>
                  );
                })}
              </div>
              <p className="text-xs text-muted-foreground">
                {text(
                  "preparedHint",
                  "“已准备”只表示本地配置存在，不代表密钥已验证、服务在线或应用已经接入。",
                )}
              </p>
            </>
          )}
          {setup.connectionReview && (
            <section className="rounded-lg border bg-background p-5">
              <h2 className="font-semibold">
                {text("connectionTitle", "确认接入应用")}
              </h2>
              <p className="mt-2 text-sm font-medium">
                {name(setup.connectionReview.app)}
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
                  checked={connectionAcknowledged}
                  disabled={busy}
                  onChange={(event) =>
                    setConnectionAcknowledged(event.target.checked)
                  }
                />
                {text("connectionAck", "我已了解将替换此应用的当前配置")}
              </label>
              <div className="flex justify-end gap-3">
                <Button
                  variant="outline"
                  disabled={busy}
                  onClick={() => {
                    setup.cancelConnection();
                    setConnectionDialog(false);
                  }}
                >
                  {t("common.cancel")}
                </Button>
                <Button
                  disabled={busy || !connectionAcknowledged}
                  onClick={() => {
                    if (setup.connectionReview?.confirmation)
                      setConnectionDialog(true);
                    else void setup.confirmConnection(false);
                  }}
                >
                  {text("confirmConnect", "确认接入")}
                </Button>
              </div>
            </section>
          )}
          {setup.needsReconciliation && (
            <p className="text-sm">
              {text(
                "reconcileHint",
                "请先核对原操作。即使列表暂未出现密钥，也不会自动再创建一次。",
              )}
            </p>
          )}
          <div className="flex flex-wrap justify-end gap-3">
            <Button
              variant="outline"
              disabled={busy}
              onClick={() => void setup.reconcile()}
            >
              {text("reconcile", "核对原操作")}
            </Button>
            {!setup.needsReconciliation &&
              setup.receipt?.targets.some(
                (target) => target.configuration === "failed",
              ) && (
                <Button
                  variant="outline"
                  disabled={busy}
                  onClick={() => {
                    setRetryApps(
                      setup
                        .receipt!.targets.filter(
                          (target) => target.configuration === "failed",
                        )
                        .map((target) => target.app),
                    );
                    setAcknowledged(false);
                  }}
                >
                  {text("retryFailed", "仅重试未完成配置")}
                </Button>
              )}
            <Button
              variant="outline"
              disabled={busy || setup.needsReconciliation}
              onClick={() => {
                setAcknowledged(false);
                setup.newDraft();
              }}
            >
              {text("newDraft", "重新选择")}
            </Button>
            <Button disabled={busy} onClick={() => void finish()}>
              {text("returnAccount", "结束并返回")}
            </Button>
          </div>
        </section>
      )}
      <SwitchTierConfirmDialog
        targetName={
          connectionDialog && setup.connectionReview
            ? (setup.connectionReview.confirmation?.targetName ??
              name(setup.connectionReview.app))
            : null
        }
        disabled={busy || !connectionAcknowledged}
        catalog={setup.connectionReview?.confirmation?.catalog}
        quitRequired={
          setup.connectionReview?.confirmation?.quitRequired ?? false
        }
        onCancel={() => setConnectionDialog(false)}
        onPreviewCatalog={(decision) => {
          if (setup.connectionReview)
            void setup.reviewConnection(setup.connectionReview.app, decision);
        }}
        onSwitch={(quit, decision) => {
          const app = setup.connectionReview?.app;
          if (!app || !connectionAcknowledged) return;
          const reviewed = setup.receipt?.targets.find(
            (target) => target.app === app,
          )?.connection.intent?.change.catalogDecision;
          if (
            decision &&
            JSON.stringify(decision) !== JSON.stringify(reviewed)
          ) {
            void setup.reviewConnection(app, decision);
            return;
          }
          setConnectionDialog(false);
          void setup.confirmConnection(quit);
        }}
      />
    </div>
  );
}
