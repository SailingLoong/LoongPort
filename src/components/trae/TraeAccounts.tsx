import {
  AccountWorkspace,
  AccountDetailSections,
} from "@/components/relay/accounts/AccountWorkspace";
import { useTraeAccountsViewModel } from "./useTraeAccountsViewModel";
import { useTranslation } from "react-i18next";
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
import { TRAE_AUTHORIZATION_FAILURE_MESSAGES } from "@/lib/traeAuthorizationError";
export function TraeAccounts({
  unified = false,
  edition = "global",
}: { unified?: boolean; edition?: "cn" | "global" } = {}) {
  const AccountsContainer = unified ? "div" : "ul";
  const AccountRow = unified ? "article" : "li";
  const { t } = useTranslation();
  const {
    accounts,
    loading,
    listFailed,
    failed,
    cancelUncertain,
    saved,
    dialogOpen,
    realm,
    flow,
    installations,
    discoveryDone,
    consent,
    attempted,
    busy,
    retiring,
    checkinResults,
    policyIsUnknown,
    setAutomatic,
    checkin,
    loadAccounts,
    beginDialog,
    closeDialog,
    selectFiles,
    discoverFiles,
    confirm,
    retire,
    chooseRealm,
    changeConsent,
    automaticDisabled,
    checkinDisabled,
    authHost,
  } = useTraeAccountsViewModel(edition);
  const failureNotice = failed && (
    <p role="alert" className="text-sm text-destructive">
      {t(`trae.failure.${failed}`, {
        defaultValue: TRAE_AUTHORIZATION_FAILURE_MESSAGES[failed],
      })}
      <span className="mt-1 block text-xs">
        {t("trae.diagnosticCode", { defaultValue: "诊断代码" })}：
        <span>{failed}</span>
      </span>
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
          ? "仅接入中国版 TRAE Code 的本机账号，用于文字模型目录和请求；不复用国际版身份，不支持 SOLO、企业版、工具或图片。自动签到默认关闭，需在权益区逐账号开启；保存账号不代表积分消费已验证。"
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
                    account.realm === "cn" ? (
                      <div className="space-y-2">
                        <p className="text-xs text-muted-foreground">
                          手动检查当前账号，或明确开启此账号的自动签到。服务受理不等于积分到账，未知权益不显示为零。
                        </p>
                        <Button
                          size="sm"
                          variant="outline"
                          disabled={checkinDisabled(account)}
                          onClick={() => void checkin(account)}
                        >
                          检查并尝试签到
                        </Button>
                        <Button
                          size="sm"
                          variant="outline"
                          disabled={automaticDisabled(account)}
                          onClick={() => void setAutomatic(account)}
                        >
                          {policyIsUnknown(account)
                            ? "自动签到状态未知"
                            : account.autoCheckinEnabled
                              ? "关闭自动签到"
                              : "开启自动签到"}
                        </Button>
                        <p className="text-xs text-muted-foreground">
                          {policyIsUnknown(account)
                            ? "自动签到状态未知，请重新载入列表核对；不会自动重试设置"
                            : account.autoCheckinEnabled
                              ? "自动签到已开启"
                              : "自动签到已关闭"}
                          ： 应用运行且凭据保管库解锁时，每 15
                          分钟检查服务端资格。
                          结果未确认时只查询，不重复领取；关闭不会撤销已提交的领取。
                          重新授权后需再次开启，后台结果请重新载入列表查看。
                        </p>
                        {(account.checkinAttemptStatus === "pending" ||
                          account.checkinAttemptStatus === "unknown") && (
                          <p role="status" className="text-xs text-amber-600">
                            仍有未确认的领取，跨日、重启或重新授权均不会重复发送。
                          </p>
                        )}
                        {account.checkinAttemptStatus === "accepted" && (
                          <p
                            role="status"
                            className="text-xs text-muted-foreground"
                          >
                            上次领取已受理，不代表积分到账。
                          </p>
                        )}
                        {account.checkinAttemptStatus === "ineligible" && (
                          <p
                            role="status"
                            className="text-xs text-muted-foreground"
                          >
                            服务拒绝上次领取或账号不符合条件。
                          </p>
                        )}
                        {account.nativeSourceSupported !== true && (
                          <p className="text-xs text-muted-foreground">
                            需先确认受支持的中国版本机账号来源。
                          </p>
                        )}
                        {checkinResults[account.id] && (
                          <p role="status">{checkinResults[account.id]}</p>
                        )}
                      </div>
                    ) : (
                      <p className="text-xs text-muted-foreground">
                        此账号页未提供TRAE
                        国际版的签到领取或完整权益查询能力。未知不会显示为零，也不会套用其他平台的奖励。
                      </p>
                    )
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
                ? "先自动发现本机官方稳定版 Trae CN Code 安装，未找到时可手动选择。发现只检查路径与公开安装信息，明确确认前不会读取凭据；保留原设备密钥。"
                : t("trae.discoveryDescription", {
                    defaultValue:
                      "先自动发现本机官方 Global TRAE 安装，未找到时可手动选择。发现只检查路径与公开安装信息，确认前不会读取凭据。",
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
                      onChange={() => chooseRealm(value)}
                    />
                    {value.toUpperCase()}
                  </label>
                ))}
              </div>
            </fieldset>
            <p>
              {t("trae.authHost", { defaultValue: "授权服务地址" })}: {authHost}
            </p>
            <Button
              disabled={!realm || busy !== null || flow !== null}
              onClick={() => void discoverFiles()}
            >
              自动发现本机 TRAE 安装
            </Button>
            {discoveryDone && installations.length === 0 && (
              <p role="status">
                未发现受支持安装，可手动选择同一安装的文件。当前自动发现支持
                macOS 与 Windows 桌面版；Linux 可手动选择。
              </p>
            )}
            {!flow && installations.length > 1 && (
              <div className="space-y-2">
                <p>发现多个安装，请选择本次导入来源：</p>
                {installations.map((installation) => (
                  <Button
                    key={installation.productPath}
                    variant="outline"
                    className="h-auto whitespace-normal text-left"
                    disabled={busy !== null}
                    onClick={() => void selectFiles(installation)}
                  >
                    {installation.productPath} · {installation.version}
                  </Button>
                ))}
              </div>
            )}
            {flow && installations.length === 1 && (
              <p className="break-all text-xs text-muted-foreground">
                已选安装：{installations[0].productPath} ·{" "}
                {installations[0].version}
              </p>
            )}
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
                  onCheckedChange={changeConsent}
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
                {t("trae.reviewAfterAttempt", {
                  defaultValue:
                    "本次尝试已结束，不会自动重试。关闭并核对列表只会读取本地状态，不会撤销可能已保存的账号；如需停用，请在列表中明确操作。",
                })}
              </p>
            )}
            {cancellationNotice}
          </div>
          <DialogFooter>
            <Button variant="outline" onClick={closeDialog}>
              {attempted && !busy
                ? t("trae.closeAndReview", { defaultValue: "关闭并核对列表" })
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
