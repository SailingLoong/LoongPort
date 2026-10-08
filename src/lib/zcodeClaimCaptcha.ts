// Adapted from pjpv/zcode-switch f34225686dfef05d84c256a56f868719248f15ff/src/captcha.js.
// Copyright (c) 2026 zcode-switch contributors; MIT, licenses/zcode-switch-MIT.txt.
import { invoke } from "@tauri-apps/api/core";
type Config = {
  enabled: boolean;
  region: string;
  prefix: string;
  sceneId: string;
};
type Captcha = { startTracelessVerification?: () => void };
declare global {
  interface Window {
    initAliyunCaptcha?: (options: {
      SceneId: string;
      mode: string;
      language: string;
      showErrorTip: boolean;
      element: string;
      button: string;
      getInstance: (instance: Captcha) => void;
      success: (param: string | { captchaVerifyParam?: string }) => void;
      fail: () => void;
      onError: () => void;
    }) => void;
    AliyunCaptchaConfig?: { region: string; prefix: string };
  }
}
const SDK_URL =
  "https://o.alicdn.com/captcha-frontend/aliyunCaptcha/AliyunCaptcha.js";
const text = document.getElementById("cap-text")!;
const button = document.getElementById("cap-btn") as HTMLButtonElement;
const zh = navigator.language.toLowerCase().startsWith("zh");
function status(en: string, cn: string) {
  text.textContent = zh ? cn : en;
}
function loadSdk() {
  return new Promise<void>((resolve, reject) => {
    if (typeof window.initAliyunCaptcha === "function") return resolve();
    const script = document.createElement("script");
    script.src = SDK_URL;
    script.onload = () => resolve();
    script.onerror = () => reject(new Error("sdk"));
    document.head.appendChild(script);
  });
}
let submitted = false;
let timer: ReturnType<typeof setTimeout> | undefined;
async function interactive() {
  if (timer) clearTimeout(timer);
  button.hidden = false;
  button.textContent = zh ? "完成验证" : "Verify";
  status("Complete verification to claim this plan.", "完成验证后领取套餐。");
  await invoke("show_zcode_claim_captcha").catch(() => {});
}
async function run() {
  try {
    const { nonce, config } = await invoke<{ nonce: string; config: Config }>(
      "get_zcode_claim_captcha",
    );
    if (!config.enabled || !config.sceneId) {
      await interactive();
      return;
    }
    status("Preparing verification…", "正在准备验证…");
    await loadSdk();
    window.AliyunCaptchaConfig = {
      region: config.region,
      prefix: config.prefix,
    };
    const submit = (param: string | undefined) => {
      if (submitted || !param?.trim()) return;
      submitted = true;
      if (timer) clearTimeout(timer);
      status(
        "Verification complete. Checking claim result…",
        "验证完成，正在核实领取结果…",
      );
      void invoke("submit_zcode_claim_captcha", { nonce, param }).catch(() => {
        status(
          "Please return to the account page to check the result.",
          "请返回账号页核实结果。",
        );
      });
    };
    window.initAliyunCaptcha!({
      SceneId: config.sceneId,
      mode: "popup",
      language: zh ? "zh-CN" : "en",
      showErrorTip: false,
      element: "#cap-holder",
      button: "#cap-btn",
      getInstance: (instance) => {
        if (typeof instance.startTracelessVerification === "function") {
          instance.startTracelessVerification();
          timer = setTimeout(() => void interactive(), 8000);
        } else {
          void interactive();
        }
      },
      success: (param) =>
        submit(typeof param === "string" ? param : param?.captchaVerifyParam),
      fail: () => void interactive(),
      onError: () => void interactive(),
    });
  } catch {
    await interactive();
    status(
      "Verification is unavailable. Close this window and try again later.",
      "验证暂不可用，请关闭窗口后稍后再试。",
    );
  }
}
void run();
