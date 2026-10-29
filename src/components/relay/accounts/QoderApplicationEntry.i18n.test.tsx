import { render, screen, cleanup } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { createInstance } from "i18next";
import { I18nextProvider, initReactI18next } from "react-i18next";
import { invoke } from "@tauri-apps/api/core";
import { afterEach, expect, it, vi } from "vitest";
import { QoderApplicationEntry } from "./QoderApplicationEntry";
import zh from "@/i18n/locales/zh.json";
import zhTW from "@/i18n/locales/zh-TW.json";
import en from "@/i18n/locales/en.json";
import ja from "@/i18n/locales/ja.json";
vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
const resources = {
  zh: { translation: zh },
  "zh-TW": { translation: zhTW },
  en: { translation: en },
  ja: { translation: ja },
};
const expected = {
  zh: [
    "当前配置用于哪些应用",
    "正在读取应用绑定…",
    "应用绑定读取失败，请在原应用中查看配置。",
    "当前配置暂无后端确认的应用绑定，请在 Codex 原配置中查看。",
    "当前页面无法打开原应用，请返回账号管理页面查看。",
  ],
  "zh-TW": [
    "目前設定用於哪些應用程式",
    "正在讀取應用程式綁定…",
    "讀取應用程式綁定失敗，請在原應用程式中查看設定。",
    "目前設定暫無後端確認的應用程式綁定，請在 Codex 原設定中查看。",
    "目前頁面無法開啟原應用程式，請返回帳號管理頁面查看。",
  ],
  en: [
    "Applications using this configuration",
    "Loading application bindings…",
    "Could not load application bindings. Check the configuration in the original application.",
    "No backend-confirmed application bindings for this configuration. Check the original Codex configuration.",
    "This page cannot open the original application. Return to account management to view it.",
  ],
  ja: [
    "この設定を使用するアプリ",
    "アプリの紐付けを読み込んでいます…",
    "アプリの紐付けを読み込めませんでした。元のアプリで設定を確認してください。",
    "この設定にはバックエンドで確認されたアプリの紐付けがありません。Codex の元の設定を確認してください。",
    "このページから元のアプリを開けません。アカウント管理ページに戻って確認してください。",
  ],
} as const;
const provider = {
  id: "a",
  name: "a",
  settingsConfig: {},
  meta: { providerType: "qoder_sdk" },
} as const;
async function mount(
  language: keyof typeof expected,
  withNavigation = true,
  withDefault = false,
) {
  const i18n = createInstance();
  await i18n.use(initReactI18next).init({
    lng: language,
    fallbackLng: false,
    resources,
    interpolation: { escapeValue: false },
  });
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  return render(
    <I18nextProvider i18n={i18n}>
      <QueryClientProvider client={client}>
        <QoderApplicationEntry
          provider={provider}
          onOpenApp={withNavigation ? vi.fn() : undefined}
          defaultApp="codex"
          onOpenDefault={withDefault ? vi.fn() : undefined}
        />
      </QueryClientProvider>
    </I18nextProvider>,
  );
}
afterEach(() => {
  cleanup();
  vi.mocked(invoke).mockReset();
});
it.each(["zh", "zh-TW", "en", "ja"] as const)(
  "renders all states in %s without fallback keys",
  async (language) => {
    const [title, loading, error, empty, unavailable] = expected[language];
    vi.mocked(invoke).mockReturnValueOnce(new Promise(() => {}));
    let view = await mount(language);
    expect(screen.getByRole("region", { name: title })).toBeInTheDocument();
    expect(screen.getByRole("status")).toHaveTextContent(loading);
    expect(
      screen.getByText(
        resources[language].translation.qoderApplicationEntry.description,
      ),
    ).toBeInTheDocument();
    view.unmount();
    vi.mocked(invoke).mockRejectedValueOnce(new Error("synthetic"));
    view = await mount(language);
    expect(await screen.findByRole("alert")).toHaveTextContent(error);
    view.unmount();
    vi.mocked(invoke).mockResolvedValueOnce([]);
    view = await mount(language, false);
    expect(await screen.findByText(empty)).toBeInTheDocument();
    expect(screen.getByText(unavailable)).toBeInTheDocument();
    view.unmount();
    vi.mocked(invoke).mockResolvedValueOnce([
      {
        sourceId: "s",
        account: {
          kind: "nativeAccount",
          platform: "qoder_sdk",
          id: "synthetic",
        },
        name: "Fixture",
        configurationName: null,
        bindings: [
          {
            appId: "codex",
            providerId: "a",
            model: "fixture-model",
            usageState: "inactive",
            canSelect: false,
          },
        ],
      },
    ]);
    view = await mount(language, false);
    expect(
      await screen.findByRole("button", { name: /fixture-model/ }),
    ).toBeDisabled();
    expect(screen.getByText(unavailable)).toBeInTheDocument();
    view.unmount();
  },
);

it.each([
  ["zh", "到 Codex 中查看与配置"],
  ["zh-TW", "前往 Codex 查看與設定"],
  ["en", "View and configure in Codex"],
  ["ja", "Codex で表示・設定"],
] as const)(
  "preserves localized original navigation for an empty %s result",
  async (language, label) => {
    vi.mocked(invoke).mockResolvedValueOnce([]);
    await mount(language, true, true);
    expect(await screen.findByRole("button", { name: label })).toBeEnabled();
  },
);
