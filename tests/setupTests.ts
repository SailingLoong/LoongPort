import "@testing-library/jest-dom/vitest";
import { afterAll, afterEach, beforeAll, vi } from "vitest";
import { cleanup } from "@testing-library/react";
import i18n from "i18next";
import { initReactI18next } from "react-i18next";
import { server } from "./msw/server";
import { resetProviderState } from "./msw/state";
import "./msw/tauriMocks";

// jsdom 未实现 scrollIntoView；AppSwitcher 切换 tab 时会把活动 tab 滚进条带视野。
Element.prototype.scrollIntoView = vi.fn();

beforeAll(async () => {
  // MSW 3 把 onUnhandledRequest 改名为 onUnhandledFrame（请求与 WebSocket 帧统一），
  // 默认行为仍是 warn，这里保持显式 warn 不变。
  server.listen({ onUnhandledFrame: "warn" });
  await i18n.use(initReactI18next).init({
    lng: "zh",
    fallbackLng: "zh",
    resources: {
      zh: { translation: {} },
      en: { translation: {} },
    },
    interpolation: {
      escapeValue: false,
    },
  });
});

afterEach(() => {
  cleanup();
  resetProviderState();
  server.resetHandlers();
  vi.clearAllMocks();
});

afterAll(() => {
  server.close();
});
