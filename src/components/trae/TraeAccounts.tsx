import {
  AccountWorkspace,
  AccountDetailSections,
} from "@/components/relay/accounts/AccountWorkspace";
import { useCallback, useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { open } from "@tauri-apps/plugin-dialog";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import {
  traeAuthHost,
  traeAccountsApi,
  type TraeAccountView,
  type TraeFlowView,
  type TraeRealm,
} from "@/lib/api/traeAccounts";

function isSelectedFile(path: unknown, filename: string): path is string {
  return typeof path === "string" && path.split(/[\\/]/).pop() === filename;
}

export function TraeAccounts({
  unified = false,
  edition = "global",
}: { unified?: boolean; edition?: "cn" | "global" } = {}) {
  const AccountsContainer = unified ? "div" : "ul";
  const AccountRow = unified ? "article" : "li";
  const { t } = useTranslation();
  const [accounts, setAccounts] = useState<TraeAccountView[]>([]);
  const [loading, setLoading] = useState(true);
  const [listFailed, setListFailed] = useState(false);
  const [failed, setFailed] = useState(false);
  const [cancelUncertain, setCancelUncertain] = useState(false);
  const [saved, setSaved] = useState(false);
  const [dialogOpen, setDialogOpen] = useState(false);
  const [realm, setRealm] = useState<TraeRealm | null>(null);
  const [flow, setFlow] = useState<TraeFlowView | null>(null);
  const [consent, setConsent] = useState(false);
  const [attempted, setAttempted] = useState(false);
  const [busy, setBusy] = useState<"selecting" | "confirming" | null>(null);
  const [retiring, setRetiring] = useState<string | null>(null);
  const alive = useRef(false);
  const dialogActive = useRef(false);
  const generation = useRef(0);
  const listGeneration = useRef(0);
  const operation = useRef<number | null>(null);
  const activeFlow = useRef<TraeFlowView | null>(null);
  const consentGiven = useRef(false);
  const confirmationAttempted = useRef(false);
  const retireActive = useRef(false);

  const loadAccounts = useCallback(async () => {
    const request = ++listGeneration.current;
    const isCurrent = () => alive.current && request === listGeneration.current;
    setLoading(true);
    setListFailed(false);
    try {
      const rows = await traeAccountsApi.list();
      if (isCurrent())
        setAccounts(
          rows.filter((row) =>
            edition === "cn"
              ? row.realm === "cn"
              : row.realm === "sg" || row.realm === "us",
          ),
        );
    } catch {
      if (isCurrent()) setListFailed(true);
    } finally {
      if (isCurrent()) setLoading(false);
    }
  }, [edition]);

  const invalidateList = useCallback(() => {
    ++listGeneration.current;
    if (alive.current) {
      setLoading(false);
      setListFailed(false);
    }
  }, []);

  const cancelFlow = useCallback(
    async (flowId: string) => {
      try {
        await traeAccountsApi.cancel(flowId);
        if (alive.current) void loadAccounts();
      } catch {
        // Never claim cancellation succeeded, nor expose raw native errors.
        if (alive.current) setCancelUncertain(true);
      }
    },
    [loadAccounts],
  );

  useEffect(() => {
    alive.current = true;
    void loadAccounts();
    return () => {
      alive.current = false;
      dialogActive.current = false;
      ++generation.current;
      ++listGeneration.current;
      operation.current = null;
      const abandoned = activeFlow.current;
      activeFlow.current = null;
      if (abandoned) void cancelFlow(abandoned.flowId);
    };
  }, [cancelFlow, loadAccounts]);

  const beginDialog = () => {
    if (dialogActive.current || retireActive.current) return;
    ++generation.current;
    dialogActive.current = true;
    activeFlow.current = null;
    consentGiven.current = false;
    confirmationAttempted.current = false;
    operation.current = null;
    setRealm(null);
    setFlow(null);
    setConsent(false);
    setAttempted(false);
    setBusy(null);
    setFailed(false);
    setSaved(false);
    setDialogOpen(true);
  };

  const closeDialog = () => {
    if (!dialogActive.current) return;
    dialogActive.current = false;
    ++generation.current;
    operation.current = null;
    consentGiven.current = false;
    const abandoned = activeFlow.current;
    activeFlow.current = null;
    setFlow(null);
    setConsent(false);
    setBusy(null);
    setDialogOpen(false);
    if (abandoned) {
      invalidateList();
      void cancelFlow(abandoned.flowId);
    }
  };

  const selectFiles = async () => {
    if (
      !realm ||
      !dialogActive.current ||
      operation.current !== null ||
      activeFlow.current
    )
      return;
    const request = generation.current;
    const isCurrent = () =>
      alive.current && dialogActive.current && request === generation.current;
    operation.current = request;
    setBusy("selecting");
    setFailed(false);
    try {
      const storagePath = await open({
        title: t("trae.selectStorage", { defaultValue: "选择 storage.json" }),
        directory: false,
        multiple: false,
        filters: [{ name: "JSON", extensions: ["json"] }],
      });
      if (!isCurrent() || storagePath === null) return;
      if (!isSelectedFile(storagePath, "storage.json")) {
        setFailed(true);
        return;
      }
      const productPath = await open({
        title: t("trae.selectProduct", {
          defaultValue: "选择同一安装的 product.json",
        }),
        directory: false,
        multiple: false,
        filters: [{ name: "JSON", extensions: ["json"] }],
      });
      if (!isCurrent() || productPath === null) return;
      if (!isSelectedFile(productPath, "product.json")) {
        setFailed(true);
        return;
      }
      const selected = await traeAccountsApi.begin({
        storagePath,
        productPath,
        realm,
      });
      if (!isCurrent()) {
        if (selected.flowId) void cancelFlow(selected.flowId);
        return;
      }
      // Validate the consent destination before displaying an actionable flow.
      if (
        !selected.flowId ||
        selected.realm !== realm ||
        selected.authHost !== traeAuthHost(realm)
      ) {
        if (selected.flowId) void cancelFlow(selected.flowId);
        setFailed(true);
        return;
      }
      activeFlow.current = selected;
      setFlow(selected);
    } catch {
      if (isCurrent()) setFailed(true);
    } finally {
      if (operation.current === request) {
        operation.current = null;
        if (isCurrent()) setBusy(null);
      }
    }
  };

  const confirm = async () => {
    const selected = activeFlow.current;
    if (
      !selected ||
      !consentGiven.current ||
      confirmationAttempted.current ||
      !dialogActive.current ||
      operation.current !== null
    )
      return;
    const request = generation.current;
    const isCurrent = () =>
      alive.current && dialogActive.current && request === generation.current;
    operation.current = request;
    confirmationAttempted.current = true;
    setAttempted(true);
    setBusy("confirming");
    setFailed(false);
    invalidateList();
    try {
      const account = await traeAccountsApi.confirm(selected.flowId);
      if (!isCurrent()) return;
      if (
        typeof account.id !== "string" ||
        !account.id.trim() ||
        account.realm !== selected.realm ||
        account.status !== "stored"
      ) {
        setFailed(true);
        return;
      }
      invalidateList();
      setAccounts((rows) =>
        rows.some((row) => row.id === account.id)
          ? rows.map((row) => (row.id === account.id ? account : row))
          : [...rows, account],
      );
      // A completed, accepted flow must not be cancelled on subsequent unmount.
      activeFlow.current = null;
      dialogActive.current = false;
      consentGiven.current = false;
      operation.current = null;
      ++generation.current;
      setFlow(null);
      setConsent(false);
      setBusy(null);
      setDialogOpen(false);
      setSaved(true);
    } catch {
      if (isCurrent()) setFailed(true);
    } finally {
      if (operation.current === request) {
        operation.current = null;
        if (isCurrent()) setBusy(null);
      }
    }
  };

  const retire = async (id: string) => {
    if (retireActive.current || dialogActive.current) return;
    retireActive.current = true;
    setRetiring(id);
    setFailed(false);
    setSaved(false);
    invalidateList();
    try {
      await traeAccountsApi.retire(id);
      if (alive.current) await loadAccounts();
    } catch {
      if (alive.current) setFailed(true);
    } finally {
      retireActive.current = false;
      if (alive.current) setRetiring(null);
    }
  };

  const failureNotice = failed && (
    <p role="alert" className="text-sm text-destructive">
      {t("trae.failed", {
        defaultValue:
          "未能确认操作结果。请检查所选官方安装文件，并重新载入列表核对。",
      })}
    </p>
  );
  const cancellationNotice = cancelUncertain && (
    <p role="alert" className="text-sm text-destructive">
      {t("trae.cancelUncertain", {
        defaultValue:
          "无法确认取消是否完成。账号可能仍已保存，请重新载入列表并核对或停用。",
      })}
    </p>
  );

  return (
    <section
      aria-label={t("trae.title", { defaultValue: "TRAE 账号" })}
      className="space-y-3"
    >
      <header className="flex flex-wrap items-center justify-between gap-3">
        <h2 className="text-sm font-medium">
          {t("trae.title", { defaultValue: "TRAE 账号" })}
        </h2>
        <div className="flex gap-2">
          <Button
            size="sm"
            variant="outline"
            onClick={() => void loadAccounts()}
          >
            {t("trae.reload", { defaultValue: "重新载入列表" })}
          </Button>
          <Button
            size="sm"
            variant="outline"
            disabled={retiring !== null}
            onClick={beginDialog}
          >
            {t("trae.add", { defaultValue: "添加 TRAE 账号" })}
          </Button>
        </div>
      </header>
      <p className="text-sm text-muted-foreground">
        {edition === "cn"
          ? "仅接入中国版 TRAE Code 的本机账号，用于文字模型目录和请求；不复用国际版身份，不支持 SOLO、企业版、工具或图片。签到尚未接入，保存账号不代表积分消费已验证。"
          : t("trae.scope", {
              defaultValue:
                "仅支持 Global TRAE 的 SG / US 文字请求；不支持工具调用或图片等多模态，CN 暂不支持。",
            })}
      </p>
      <p className="text-sm text-muted-foreground">
        {t("trae.localState", {
          defaultValue:
            "以下状态仅表示本地账号存储状态，不代表订阅、额度或请求可用性。",
        })}
      </p>
      {loading && (
        <p role="status">
          {t("trae.loading", { defaultValue: "正在读取本地账号列表…" })}
        </p>
      )}
      {listFailed && (
        <p role="alert">
          {t("trae.listFailed", { defaultValue: "无法读取账号列表，请重试。" })}
        </p>
      )}
      {!dialogOpen && failureNotice}
      {!dialogOpen && cancellationNotice}
      {saved && (
        <p role="status">
          {t("trae.saved", { defaultValue: "账号已加密保存在本地。" })}
        </p>
      )}
      {!loading && !listFailed && accounts.length === 0 && (
        <p className="rounded-xl border border-dashed p-4 text-sm text-muted-foreground">
          {t("trae.empty", { defaultValue: "尚无 TRAE 本地账号。" })}
        </p>
      )}
      <AccountsContainer className="space-y-2">
        {
          <AccountWorkspace
            accounts={accounts}
            label={(account) => account.id}
            enabled={unified}
          >
            {(account) => {
              const nativeAuthorization = (
                <AccountRow
                  key={account.id}
                  className="flex flex-wrap items-center justify-between gap-3 rounded-xl border border-border p-3 text-sm"
                >
                  <div className="min-w-0 space-y-1">
                    <p className="break-all">{account.id}</p>
                    <p>{account.realm.toUpperCase()}</p>
                    <p>
                      {t(`trae.status.${account.status}`, {
                        defaultValue: {
                          stored: "已保存（本地）",
                          refreshPending: "授权待核对（不会自动重试）",
                          retired: "已停用",
                        }[account.status],
                      })}
                    </p>
                  </div>
                  <Button
                    size="sm"
                    variant="outline"
                    disabled={retiring !== null || account.status === "retired"}
                    onClick={() => void retire(account.id)}
                  >
                    {t("trae.retire", { defaultValue: "停用本地账号" })}
                  </Button>
                </AccountRow>
              );
              if (!unified) return nativeAuthorization;
              return (
                <AccountDetailSections
                  authorization={
                    <>
                      {nativeAuthorization}
                      <p className="text-xs text-muted-foreground">
                        此状态仅反映本地账号记录，不等于官方会话或推理已可用。
                      </p>
                    </>
                  }
                  entitlements={
                    <p className="text-xs text-muted-foreground">
                      此账号页未提供TRAE
                      国际版的签到领取或完整权益查询能力。未知不会显示为零，也不会套用其他平台的奖励。
                    </p>
                  }
                  inference={
                    <p className="text-xs text-muted-foreground">
                      真实推理消费尚未验收。模型与流式请求需通过原应用入口独立验证。
                    </p>
                  }
                />
              );
            }}
          </AccountWorkspace>
        }
      </AccountsContainer>
      <Dialog
        open={dialogOpen}
        onOpenChange={(value) => {
          if (!value) closeDialog();
        }}
      >
        <DialogContent
          closeButtonLabel={t("trae.close", { defaultValue: "关闭 TRAE 授权" })}
        >
          <DialogHeader>
            <DialogTitle>
              {t("trae.add", { defaultValue: "添加 TRAE 账号" })}
            </DialogTitle>
            <DialogDescription>
              {edition === "cn"
                ? "请选择同一官方稳定版 Trae CN Code 安装的 storage.json 和 product.json。只记录路径，明确确认前不会读取凭据；必须保留原设备密钥。"
                : t("trae.selectDescription", {
                    defaultValue:
                      "请选择同一正常官方 Global TRAE 安装的 storage.json 和 product.json。选择仅记录路径，确认前不会读取凭据。",
                  })}
            </DialogDescription>
          </DialogHeader>
          <div className="space-y-4 overflow-y-auto px-6 py-4 text-sm">
            <fieldset
              disabled={busy !== null || flow !== null}
              className="space-y-2"
            >
              <legend>{t("trae.realm", { defaultValue: "账号区域" })}</legend>
              <div className="flex gap-4">
                {(edition === "cn"
                  ? (["cn"] as const)
                  : (["sg", "us"] as const)
                ).map((value) => (
                  <label key={value} className="flex items-center gap-2">
                    <input
                      type="radio"
                      name="trae-realm"
                      value={value}
                      checked={realm === value}
                      onChange={() => setRealm(value)}
                    />
                    {value.toUpperCase()}
                  </label>
                ))}
              </div>
            </fieldset>
            <p>
              {t("trae.authHost", { defaultValue: "授权服务地址" })}:{" "}
              {traeAuthHost(realm ?? (edition === "cn" ? "cn" : "sg"))}
            </p>
            <Button
              variant="outline"
              disabled={!realm || busy !== null || flow !== null}
              onClick={() => void selectFiles()}
            >
              {t("trae.selectFiles", {
                defaultValue: "选择 storage.json 和 product.json",
              })}
            </Button>
            {busy === "selecting" && (
              <p role="status">
                {t("trae.selecting", { defaultValue: "正在选择文件…" })}
              </p>
            )}
            {flow && (
              <label className="flex items-start gap-2 rounded-xl border border-border p-3">
                <Checkbox
                  checked={consent}
                  disabled={busy !== null || attempted}
                  onCheckedChange={(checked) => {
                    consentGiven.current = checked;
                    setConsent(checked);
                  }}
                />
                <span>
                  {t("trae.consent", {
                    defaultValue:
                      "我确认这两个文件来自同一官方安装，并授权读取其中的 TRAE 凭据，向上述地址进行正常授权交换和账号资料查询，再加密保存到本地。",
                  })}
                </span>
              </label>
            )}
            {failureNotice}
            {attempted && !busy && (
              <p>
                {t("trae.flowConsumed", {
                  defaultValue:
                    "本次授权已结束，不会自动重试。请取消并核对列表；需要重试时重新选择文件。",
                })}
              </p>
            )}
            {cancellationNotice}
          </div>
          <DialogFooter>
            <Button variant="outline" onClick={closeDialog}>
              {attempted && !busy
                ? t("trae.cancelAndReview", { defaultValue: "取消并核对列表" })
                : t("trae.cancel", { defaultValue: "取消" })}
            </Button>
            <Button
              disabled={!flow || !consent || busy !== null || attempted}
              onClick={() => void confirm()}
            >
              {t(busy === "confirming" ? "trae.confirming" : "trae.confirm", {
                defaultValue:
                  busy === "confirming"
                    ? "正在授权保存…"
                    : "确认读取并授权保存",
              })}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </section>
  );
}
