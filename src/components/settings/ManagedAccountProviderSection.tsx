import { useCallback, useEffect, useRef, useState } from "react";
import { useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { minimaxCodeApi } from "@/lib/api/minimaxCode";
import {
  workbuddyApi,
  type WorkBuddyInferenceModel,
} from "@/lib/api/workbuddy";
import { zcodeAccountsApi } from "@/lib/api/zcodeAccounts";
import { traeAccountsApi } from "@/lib/api/traeAccounts";
import { traeModelsApi } from "@/lib/api/traeModels";
import { providersApi } from "@/lib/api/providers";
import {
  applicationRoutingApi,
  type ApplicationRoutingRequest,
} from "@/lib/api/applicationRouting";
import { orderProfilesApi } from "@/lib/api/orderProfiles";
import type { Provider } from "@/types";
import {
  buildManagedAccountProvider,
  isTraeModelText,
  managedAccountProviderId,
  sameManagedAccountProvider,
  type ManagedInferenceAccount,
  type ManagedInferenceApp,
  type ManagedInferencePlatform,
} from "@/lib/managedAccountProvider";

const key = (account: ManagedInferenceAccount) =>
  JSON.stringify([account.authProvider, account.id]);
async function readAccounts(
  platform: ManagedInferencePlatform,
): Promise<ManagedInferenceAccount[]> {
  if (platform === "minimax_code")
    return (await minimaxCodeApi.list()).map((account) => ({
      authProvider: platform,
      id: account.id,
      label: account.label,
      available: account.authorized && account.status !== "needsLogin",
    }));
  if (platform === "trae")
    return (await traeAccountsApi.list()).map((row) => ({
      authProvider: platform,
      id: row.id,
      label: `${row.realm === "sg" ? "SG" : row.realm === "us" ? "US" : "不支持的区域"} · ${row.id} · ${row.status === "stored" ? "已保存" : row.status === "refreshPending" ? "刷新结果待确认" : row.status === "retired" ? "已退役" : "状态未识别"}`,
      // Local stored status permits submitting a reference, not a claim that a
      // model is entitled or usable. The original backend owner admits requests.
      available:
        (row.realm === "sg" || row.realm === "us") && row.status === "stored",
    }));
  if (platform === "workbuddy") {
    const [rows, inference] = await Promise.all([
      workbuddyApi.list(),
      workbuddyApi.listInferenceAccounts(),
    ]);
    const states = new Map(inference.map((row) => [row.id, row.state]));
    return rows.map((row) => ({
      authProvider: platform,
      id: row.id,
      label: row.label,
      // Old check-in canRefresh/canClaim says nothing about the new audience.
      available: ["ready", "refreshRequired"].includes(
        states.get(row.id) ?? "",
      ),
    }));
  }
  // This route's backend acquires the original default namespace. A custom
  // library path must never be smuggled into provider metadata or request bodies.
  const catalog = await zcodeAccountsApi.library();
  return catalog.profiles.map((account) => ({
    authProvider: platform,
    id: account.id,
    label:
      account.label ||
      account.officialLabel ||
      `${account.family} · ${account.id}`,
    available:
      !catalog.pending &&
      account.capabilities?.start.check.state !== "unavailable",
  }));
}

/** Explicit, one-account entry into the existing application provider chain. */
export function ManagedAccountProviderSection() {
  const { t } = useTranslation();
  const copy = (name: string, defaultValue: string) =>
    t(`managedAccount.${name}`, { defaultValue });
  const queryClient = useQueryClient();
  const [accounts, setAccounts] = useState<ManagedInferenceAccount[]>([]);
  const [selected, setSelected] = useState("");
  const [app, setApp] = useState<ManagedInferenceApp | "">("");
  const [model, setModel] = useState("");
  const [models, setModels] = useState<WorkBuddyInferenceModel[]>([]);
  const [modelsOwner, setModelsOwner] = useState("");
  const [modelsBusy, setModelsBusy] = useState(false);
  const [busy, setBusy] = useState(false);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [pending, setPending] = useState<{
    provider: Provider;
    app: ManagedInferenceApp;
    chainOnly?: boolean;
    chainRequest?: ApplicationRoutingRequest;
  } | null>(null);
  const alive = useRef(false);
  const inFlight = useRef(false);
  const generation = useRef(0);
  const modelGeneration = useRef(0);
  const modelFlight = useRef<number | null>(null);
  const traeModelRequest = useRef<string | null>(null);
  const cancelTraeModels = useCallback(() => {
    const requestId = traeModelRequest.current;
    traeModelRequest.current = null;
    // No cancellation-success claim; a failure must not mutate newer UI state.
    if (requestId) void traeModelsApi.cancel(requestId).catch(() => {});
  }, []);
  const readTraeModels = async (accountId: string, current: () => boolean) => {
    const { requestId } = await traeModelsApi.begin(accountId);
    if (!current()) {
      void traeModelsApi.cancel(requestId).catch(() => {});
      return [];
    }
    traeModelRequest.current = requestId;
    try {
      // Single-use request. No automatic retry or polling after any outcome.
      const found = await traeModelsApi.list(requestId);
      if (
        found.length > 512 ||
        found.some(
          (row) => !isTraeModelText(row.id) || !isTraeModelText(row.name),
        )
      )
        throw new Error("managedAccount.invalidModels");
      return found;
    } finally {
      if (traeModelRequest.current === requestId)
        traeModelRequest.current = null;
    }
  };
  const clearModels = useCallback(() => {
    cancelTraeModels();
    modelGeneration.current += 1;
    modelFlight.current = null;
    setModels([]);
    setModelsOwner("");
    setModelsBusy(false);
    setModel("");
  }, [cancelTraeModels]);

  const refresh = useCallback(async () => {
    const epoch = ++generation.current;
    setLoading(true);
    setSelected("");
    clearModels();
    setError(null);
    const results = await Promise.allSettled([
      readAccounts("minimax_code"),
      readAccounts("zcode_start_plan"),
      readAccounts("workbuddy"),
      readAccounts("trae"),
    ]);
    if (!alive.current || generation.current !== epoch) return;
    setAccounts(
      results.flatMap((result) =>
        result.status === "fulfilled" ? result.value : [],
      ),
    );
    if (results.some((result) => result.status === "rejected"))
      setError("catalog");
    setLoading(false);
  }, [clearModels]);
  useEffect(() => {
    alive.current = true;
    void refresh();
    return () => {
      alive.current = false;
      generation.current += 1;
      modelGeneration.current += 1;
      modelFlight.current = null;
      cancelTraeModels();
    };
  }, [cancelTraeModels, refresh]);

  const account = accounts.find((item) => key(item) === selected);
  const workbuddy = account?.authProvider === "workbuddy";
  const trae = account?.authProvider === "trae";
  const modelSelected =
    !(workbuddy || trae) ||
    (modelsOwner === selected && models.some((item) => item.id === model));
  const discover = async () => {
    if (
      !account?.available ||
      !(workbuddy || trae) ||
      busy ||
      loading ||
      pending ||
      modelFlight.current !== null
    )
      return;
    const stamp = ++modelGeneration.current;
    const owner = key(account);
    modelFlight.current = stamp;
    setModelsBusy(true);
    setModels([]);
    setModelsOwner("");
    setModel("");
    setError(null);
    try {
      const current = () => alive.current && modelGeneration.current === stamp;
      const found = trae
        ? await readTraeModels(account.id, current)
        : await workbuddyApi.discoverModels(account.id);
      if (!alive.current || modelGeneration.current !== stamp) return;
      const unique = [
        ...new Map(
          found
            .filter(
              (item) => trae || /^[A-Za-z0-9._:/@+-]{1,200}$/.test(item.id),
            )
            .map((item) => [item.id, item]),
        ).values(),
      ];
      setModels(unique);
      setModelsOwner(owner);
      if (!unique.length) setError(trae ? "emptyModels" : "models");
    } catch {
      if (alive.current && modelGeneration.current === stamp)
        setError("models");
    } finally {
      if (alive.current && modelGeneration.current === stamp) {
        modelFlight.current = null;
        setModelsBusy(false);
      }
    }
  };
  const announceSaved = (target: ManagedInferenceApp) => {
    setPending(null);
    setSelected("");
    clearModels();
    setError(null);
    setNotice(target);
    void queryClient.invalidateQueries({ queryKey: ["providers", target] });
    void queryClient.invalidateQueries({ queryKey: ["applicationRouting"] });
  };
  type Attempt = {
    provider: Provider;
    app: ManagedInferenceApp;
    chainOnly?: boolean;
    chainRequest?: ApplicationRoutingRequest;
  };
  const recheck = async (attempt: Attempt, epoch = generation.current) => {
    const current = () => alive.current && generation.current === epoch;
    try {
      const rows = await providersApi.getAll(attempt.app);
      if (!current()) return;
      const actual = rows[attempt.provider.id];
      if (!actual || !sameManagedAccountProvider(actual, attempt.provider)) {
        setPending(null);
        setError(actual ? "conflict" : "save");
        return;
      }
      // A row commit and original best-effort chain append are separate facts.
      // Never call a saved row "in the chain" without querying its real owner.
      const routing = await applicationRoutingApi.get(attempt.app);
      if (!current()) return;
      if (routing.chainIds.includes(actual.id)) announceSaved(attempt.app);
      else {
        setPending({ ...attempt, chainOnly: true });
        setError("chain");
      }
    } catch {
      if (current()) {
        setPending(attempt);
        setError(attempt.chainOnly ? "chain" : "unknown");
      }
    }
  };
  const restoreChain = async (attempt: Attempt) => {
    if (inFlight.current) return;
    inFlight.current = true;
    setBusy(true);
    try {
      if (attempt.chainRequest) {
        const result = await applicationRoutingApi.queryRequest(
          attempt.app,
          attempt.chainRequest,
        );
        if (
          !["completed", "notRecorded", "discarded", "abandoned"].includes(
            result.status,
          )
        ) {
          setError("chain");
          return;
        }
        attempt = { ...attempt, chainRequest: undefined };
      }
      const rows = await providersApi.getAll(attempt.app);
      const actual = rows[attempt.provider.id];
      if (!actual || !sameManagedAccountProvider(actual, attempt.provider)) {
        setPending(null);
        setError("conflict");
        return;
      }
      const routing = await applicationRoutingApi.get(attempt.app);
      if (!routing.chainIds.includes(actual.id)) {
        const profiles = await orderProfilesApi.list(attempt.app);
        const change = {
          order: {
            profileName: profiles.current,
            providerIds: [...routing.chainIds, actual.id],
          },
        };
        const request = await applicationRoutingApi.prepareRequest(
          attempt.app,
          change,
          crypto.randomUUID(),
        );
        const [latest, latestProfiles] = await Promise.all([
          applicationRoutingApi.get(attempt.app),
          orderProfilesApi.list(attempt.app),
        ]);
        // Preparation pins original owner revision. Detect a change between our
        // initial read and preparation; the original request CAS covers after it.
        if (
          JSON.stringify(latest.chainIds) !==
            JSON.stringify(routing.chainIds) ||
          JSON.stringify(latestProfiles) !== JSON.stringify(profiles)
        ) {
          setError("chain");
          return;
        }
        attempt = { ...attempt, chainRequest: request };
        setPending(attempt);
        await applicationRoutingApi.apply(
          attempt.app,
          change,
          undefined,
          request,
        );
      }
      await recheck(attempt);
    } catch {
      if (alive.current) {
        setPending(attempt);
        setError("chain");
      }
    } finally {
      inFlight.current = false;
      if (alive.current) setBusy(false);
    }
  };
  const save = async () => {
    if (
      inFlight.current ||
      pending ||
      loading ||
      !account?.available ||
      !app ||
      !model.trim() ||
      !modelSelected ||
      modelsBusy
    )
      return;
    inFlight.current = true;
    setBusy(true);
    setNotice(null);
    setError(null);
    let attempt: Attempt | null = null;
    const epoch = generation.current;
    const stamp = modelGeneration.current;
    const currentScope = () =>
      alive.current &&
      generation.current === epoch &&
      modelGeneration.current === stamp;
    try {
      // The list can become stale after logout/retirement. Re-read its real owner;
      // the inference lease remains authoritative for credential admission.
      const current = await readAccounts(account.authProvider);
      if (!currentScope()) return;
      const chosen = current.find(
        (item) => key(item) === selected && item.available,
      );
      if (!chosen) {
        setSelected("");
        clearModels();
        setError("account");
        return;
      }
      if (chosen.authProvider === "workbuddy") {
        const live = await workbuddyApi.discoverModels(chosen.id);
        if (!currentScope()) return;
        if (!live.some((item) => item.id === model)) {
          clearModels();
          setError("models");
          return;
        }
      }
      if (chosen.authProvider === "trae") {
        try {
          const live = await readTraeModels(chosen.id, currentScope);
          if (!currentScope()) return;
          if (!live.some((item) => item.id === model)) {
            clearModels();
            setError(live.length ? "models" : "emptyModels");
            return;
          }
        } catch {
          if (currentScope()) {
            clearModels();
            setError("models");
          }
          return;
        }
      }
      const id = await managedAccountProviderId(chosen, app, model);
      if (!currentScope()) return;
      const provider = buildManagedAccountProvider(chosen, app, model, id);
      attempt = { provider, app };
      const rows = await providersApi.getAll(app);
      if (!currentScope()) return;
      const sameId = rows[provider.id];
      if (sameId && !sameManagedAccountProvider(sameId, provider)) {
        setError("conflict");
        return;
      }
      // Reuse an older matching row too; label changes do not create a new route.
      const existing = Object.values(rows).find((row) =>
        sameManagedAccountProvider(row, provider),
      );
      if (existing)
        attempt = { provider: { ...provider, id: existing.id }, app };
      else if (!(await providersApi.add(provider, app, false)))
        throw new Error("save failed");
      if (currentScope()) await recheck(attempt, epoch);
    } catch {
      if (currentScope()) {
        if (attempt) await recheck(attempt, epoch);
        else setError("account");
      }
    } finally {
      inFlight.current = false;
      if (alive.current) setBusy(false);
    }
  };

  const fieldClass =
    "w-full rounded-md border border-input bg-background px-3 py-2 text-sm";
  const locked = busy || loading || pending !== null;
  return (
    <section className="space-y-4 rounded-xl border border-border/60 bg-card/60 p-6">
      <h4 className="font-medium">{copy("title", "将账号加入应用路由链")}</h4>
      <p className="text-sm text-muted-foreground">
        {copy(
          "description",
          "选择一个已保存账号和官方模型。加入后可参与该应用已启用的故障切换链；不会切换当前配置、开启本地路由或切换原生账号。",
        )}
      </p>
      <p className="text-sm text-muted-foreground">
        {copy(
          "namespace",
          "ZCode 仅使用默认账号库，当前生产支持 macOS；自定义账号库不会自动映射。请先完成原账号登录；WorkBuddy 请先在原账号行授权推理，再显式获取实时模型；这里不会新建密钥。",
        )}
      </p>
      {trae && (
        <p className="text-sm text-muted-foreground">
          {copy(
            "traeTextOnly",
            "TRAE Global（SG/US）仅纯文本，不支持工具调用、图片或附件，也不提供用量统计。请显式获取并选择此账号的已发现模型；发现列表尚未验证模型可用。客户端自动携带工具时，请求会被拒绝。",
          )}
        </p>
      )}
      <div className="grid gap-3 md:grid-cols-3">
        <label className="space-y-1">
          <span>{copy("account", "账号")}</span>
          <select
            aria-label={copy("account", "账号")}
            className={fieldClass}
            value={selected}
            disabled={locked}
            onChange={(event) => {
              clearModels();
              setSelected(event.target.value);
              setError(null);
              setNotice(null);
            }}
          >
            <option value="">{copy("chooseAccount", "请选择账号")}</option>
            {accounts.map((item) => (
              <option
                key={key(item)}
                value={key(item)}
                disabled={!item.available}
              >
                {item.authProvider === "minimax_code"
                  ? "MiniMax Code"
                  : item.authProvider === "workbuddy"
                    ? "WorkBuddy"
                    : item.authProvider === "trae"
                      ? "TRAE Global"
                      : "ZCode Start Plan"}{" "}
                · {item.label}
                {item.available
                  ? ""
                  : ` (${copy("unavailable", "需重新登录或暂不可用")})`}
              </option>
            ))}
          </select>
        </label>
        <label className="space-y-1">
          <span>{copy("app", "目标应用")}</span>
          <select
            aria-label={copy("app", "目标应用")}
            className={fieldClass}
            value={app}
            disabled={locked}
            onChange={(event) =>
              setApp(event.target.value as ManagedInferenceApp | "")
            }
          >
            <option value="">{copy("chooseApp", "请选择目标应用")}</option>
            <option value="claude">Claude Code</option>
            <option value="codex">Codex</option>
          </select>
        </label>
        <label className="space-y-1">
          <span>{copy("model", "官方模型 ID")}</span>
          {workbuddy || trae ? (
            <select
              aria-label={copy("model", "官方模型 ID")}
              className={fieldClass}
              value={model}
              disabled={locked || modelsBusy || modelsOwner !== selected}
              onChange={(event) => setModel(event.target.value)}
            >
              <option value="">
                {copy("chooseLiveModel", "请获取并选择实时模型")}
              </option>
              {modelsOwner === selected &&
                models.map((item) => (
                  <option key={item.id} value={item.id}>
                    {item.name}
                  </option>
                ))}
            </select>
          ) : (
            <input
              aria-label={copy("model", "官方模型 ID")}
              className={fieldClass}
              value={model}
              disabled={locked}
              maxLength={200}
              onChange={(event) => setModel(event.target.value)}
            />
          )}
        </label>
      </div>
      <div className="flex gap-3">
        {(workbuddy || trae) && (
          <button
            type="button"
            className="text-sm underline disabled:opacity-50"
            disabled={locked || modelsBusy || !account?.available}
            onClick={() => void discover()}
          >
            {trae
              ? copy("discoverTraeModels", "获取 TRAE 模型")
              : copy("discoverWorkbuddyModels", "获取 WorkBuddy 模型")}
          </button>
        )}
        <button
          type="button"
          className="rounded-md bg-primary px-3 py-2 text-sm text-primary-foreground disabled:opacity-50"
          disabled={
            locked ||
            modelsBusy ||
            !account?.available ||
            !app ||
            !model.trim() ||
            !modelSelected
          }
          onClick={() => void save()}
        >
          {copy("add", "加入应用路由链")}
        </button>
        <button
          type="button"
          className="text-sm underline disabled:opacity-50"
          disabled={locked}
          onClick={() => void refresh()}
        >
          {copy("refresh", "刷新账号列表")}
        </button>
        {pending && (
          <button
            type="button"
            disabled={busy}
            onClick={() => {
              if (pending.chainOnly) {
                void restoreChain(pending);
                return;
              }
              if (inFlight.current) return;
              inFlight.current = true;
              setBusy(true);
              void recheck(pending).finally(() => {
                inFlight.current = false;
                if (alive.current) setBusy(false);
              });
            }}
          >
            {pending.chainOnly
              ? copy("restoreChain", "重试加入路由链")
              : copy("recheck", "核对保存结果")}
          </button>
        )}
      </div>
      {loading && <p>{copy("loading", "正在读取账号库…")}</p>}
      {error && (
        <p role="alert" className="text-sm text-destructive">
          {error === "emptyModels"
            ? copy(
                "emptyModels",
                "未发现可选模型，请重新获取；不会回退到手动填写。",
              )
            : error === "models"
              ? copy(
                  "modelsError",
                  "未确认当前账号的实时模型，请重新获取并选择模型。",
                )
              : error === "chain"
                ? copy(
                    "chainPending",
                    "已保存，加入路由待恢复。重试仅更新原路由链，不会重复创建档位。",
                  )
                : error === "conflict"
                  ? copy(
                      "identityConflict",
                      "已有档位身份或模型与本次选择不同，未覆盖原记录。请到应用页核对。",
                    )
                  : error === "unknown"
                    ? copy(
                        "unknown",
                        "保存结果暂无法确认。请核对保存结果，不要重复提交。",
                      )
                    : error === "catalog"
                      ? copy(
                          "catalogError",
                          "部分账号库不可用；仅显示本次成功读取的账号。请确认原账号库已解锁。",
                        )
                      : copy(
                          "saveError",
                          "未确认保存成功。请检查账号状态和模型 ID 后重试。",
                        )}
        </p>
      )}
      {notice && (
        <p role="status" className="text-sm">
          {copy(
            "saved",
            "已加入应用路由链，未切换当前配置。请到目标应用页启用原本地路由并选择该档位；已有路由将按原链规则使用。没有原应用配置时，需先完成应用初始化。",
          )}
        </p>
      )}
    </section>
  );
}
