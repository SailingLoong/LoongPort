import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

/**
 * 闸：**「添加中转站」这条路登录成功后必须就地备好密钥。**
 *
 * ## 它守的是什么缺陷（2026-08-04 用户实测报的）
 *
 * 用户在网页上登录完，界面却提示「该账号在此平台下没有可用分组」，非要再点一次
 * 「刷新」/「获取密钥」才补上。根因在这一条路少了一步：
 *
 * `probeSite` 只建**站点行**（`creds::save_site`），`operator_login` 只写**凭据** ——
 * 两条都不碰分组。而拉分组只有 `operator_provision` 这一条路（真打
 * `/groups/available` 再逐组建 sk）。少了它，宿主随后那次 refresh 读的是本地 DB，
 * 里头一个档位都没有 ⇒ 那一行落到 `loggedIn && tiers.length === 0` 分支，
 * 显示的正是那句「没有可用分组」。
 *
 * 而那句话此刻是**不属实的**：账号在运营商那边有分组，只是本地还没去拉。
 *
 * ## 为什么是渲染测试而不是读源码断言
 *
 * 要验的是「点确认之后真的发出了那条命令」这个**行为**，而不是源码里出现过
 * 某个标识符。源码断言在这里会漏掉真实的失败形态：把 provision 写在
 * `if (loggedIn)` 之外、或写在 `onClose()` 之后（跑得到，但已经晚了 ——
 * 宿主早被通知去 reload 了），
 * 字符串照样匹配得上。
 */
const { login, provision, probeSite, listSponsors, listSites } = vi.hoisted(
  () => ({
    login: vi.fn(),
    provision: vi.fn(),
    probeSite: vi.fn(),
    listSponsors: vi.fn(),
    listSites: vi.fn(),
  }),
);

vi.mock("@/lib/api", () => ({
  operatorApi: { login, provision, probeSite, listSponsors, listSites },
}));

// 全局 setup 里的 i18n 资源是**空的** ⇒ `t()` 只回 key、把插值丢掉，
// 于是「点名说出是哪个分组」这件事在断言里看不见。把参数拼进返回值，
// 断言才管得到「分组名与原因真的进了那条 toast」（与
// `ModelsDevAutoSyncPanel.test.tsx` 同一写法）。
vi.mock("react-i18next", () => ({
  useTranslation: () => ({
    t: (key: string, options?: Record<string, unknown>) =>
      options ? `${key} ${JSON.stringify(options)}` : key,
    i18n: { resolvedLanguage: "zh" },
  }),
}));

const toastError = vi.fn();
const toastWarning = vi.fn();
vi.mock("sonner", () => ({
  toast: {
    // `success` 不留 spy：这些测试断言的是「分组拉了没有 / 失败说了没有」，
    // 成功提示的措辞归 i18n 的闸管（`tests/config/loongportLocales.test.ts`）。
    success: vi.fn(),
    error: (...args: unknown[]) => toastError(...args),
    warning: (...args: unknown[]) => toastWarning(...args),
  },
}));

// Radix 的弹窗在 jsdom 里要 portal + 焦点管理，塌成裸 div 就够验行为
// （与 `AddProviderDialog.test.tsx` 同一写法）。
vi.mock("@/components/ui/dialog", () => ({
  Dialog: ({ children }: { children: React.ReactNode }) => (
    <div>{children}</div>
  ),
  DialogContent: ({ children }: { children: React.ReactNode }) => (
    <div>{children}</div>
  ),
  DialogHeader: ({ children }: { children: React.ReactNode }) => (
    <div>{children}</div>
  ),
  DialogTitle: ({ children }: { children: React.ReactNode }) => (
    <h1>{children}</h1>
  ),
  DialogDescription: ({ children }: { children: React.ReactNode }) => (
    <p>{children}</p>
  ),
  DialogFooter: ({ children }: { children: React.ReactNode }) => (
    <div>{children}</div>
  ),
}));

const { AddSiteDialog } = await import("@/components/operator/AddSiteDialog");

/** 一份空的 provision 结果 —— 这些测试关心的是「有没有调它」，不是它返回什么。 */
const emptySummary = { tiers: [], keysCreated: 0, failures: [] };

function renderDialog(overrides: { onAdded?: () => void } = {}) {
  return render(
    <AddSiteDialog
      open
      onClose={() => {}}
      onAdded={overrides.onAdded ?? (() => {})}
      defaultSite="790053500.com"
      appId="codex"
    />,
  );
}

/** 点「确认」（`common.confirm`，i18n 资源为空 ⇒ 渲染成 key 本身）。 */
function clickConfirm() {
  fireEvent.click(screen.getByText("common.confirm"));
}

describe("「添加中转站」登录后就地备好密钥", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    probeSite.mockResolvedValue({
      operatorId: 7,
      siteOrigin: "https://790053500.com",
      siteName: "鑫旺",
    });
    listSponsors.mockResolvedValue([]);
    listSites.mockResolvedValue([]);
    login.mockResolvedValue(true);
    provision.mockResolvedValue(emptySummary);
  });

  it("登录成功后拉一次分组，用户不必再点刷新", async () => {
    renderDialog();
    clickConfirm();

    // 少了这一步，那一行就停在「该账号在此平台下没有可用分组」——
    // 而那句话不属实（分组在运营商那边有，只是本地没去拉）。
    //
    // 断言**带上 id**：`provision` 的 operatorId 是必填的（「当前站」那条回落已随
    // `is_current` 一起删掉），而它必须是 `probeSite` 返回的那一行 ——
    // 传错行就是给别的账号建 sk。
    await waitFor(() => expect(provision).toHaveBeenCalledWith(7));
  });

  it("等分组拉完才通知宿主刷新（否则宿主读到的还是零档位）", async () => {
    // 卡住 provision 不 resolve —— 这是唯一能区分「await 了」与「fire-and-forget」的形状。
    let releaseProvision!: (r: unknown) => void;
    provision.mockReturnValue(
      new Promise((resolve) => {
        releaseProvision = resolve;
      }),
    );
    const onAdded = vi.fn();
    renderDialog({ onAdded });
    clickConfirm();

    await waitFor(() => expect(provision).toHaveBeenCalled());
    // ⚠️ **这条闸守的是顺序，不是「有没有调」**（codex review 2026-08-04 抓出）：
    // 写成 `void provision().then(...)` 的话 `onAdded()` 会立刻跑 ⇒ 宿主的 refresh
    // 抢在 `/groups/available` 与逐组建 sk 的网络往返**之前**读本地 DB
    // ⇒ 那一行照样显示「没有可用分组」，而且之后没有任何东西再刷它
    // —— 一字不差地复现这个 bug，而另四条闸全都照样绿。
    expect(onAdded).not.toHaveBeenCalled();

    releaseProvision(emptySummary);
    await waitFor(() => expect(onAdded).toHaveBeenCalled());
  });

  it("用户自己关掉登录窗时不拉分组（没有凭据，后端在发请求前就报错）", async () => {
    login.mockResolvedValue(false);
    renderDialog();
    clickConfirm();

    // 等 handleProbe 整条链跑完再断言「没调」，否则这条测试对任何实现都绿。
    await waitFor(() => expect(login).toHaveBeenCalled());
    expect(provision).not.toHaveBeenCalled();
  });

  it("部分分组建密钥失败时点名说出来（行内那两条路径就是这么做的）", async () => {
    provision.mockResolvedValue({
      tiers: [{ providerId: "p1" }],
      keysCreated: 1,
      failures: [{ groupName: "pro池", reason: "已达 Key 上限" }],
    });
    renderDialog();
    clickConfirm();

    // 静默吞掉 failures 的后果：用户以为全部分组都备好了，而 `pro池` 那条
    // 压根没生成 —— 他要到点它、拿到一个看不懂的 401 时才发现。
    // `OperatorSection` 的 `reportProvision`（行内登录 / 获取密钥两条路共用它）
    // 都逐条 warning，这条路不该更沉默。
    await waitFor(() =>
      expect(toastWarning).toHaveBeenCalledWith(
        expect.stringContaining("pro池"),
      ),
    );
    expect(toastWarning).toHaveBeenCalledWith(
      expect.stringContaining("已达 Key 上限"),
    );
  });

  it("拉分组失败不卡住加站：照样通知宿主刷新，并把原因说出来", async () => {
    provision.mockRejectedValue(new Error("网络不通"));
    const onAdded = vi.fn();
    renderDialog({ onAdded });
    clickConfirm();

    // 站点行与登录态都已落库 ⇒ 必须让宿主刷出那一行。它会显示「获取密钥」，
    // 那是用户可点的出口；把弹窗卡住反而让他既看不到新行、也没法重试。
    await waitFor(() => expect(onAdded).toHaveBeenCalled());
    expect(toastError).toHaveBeenCalledWith(
      expect.stringContaining("网络不通"),
    );
  });
});
