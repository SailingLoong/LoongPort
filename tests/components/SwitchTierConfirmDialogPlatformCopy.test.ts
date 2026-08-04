import { describe, expect, it } from "vitest";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";

/**
 * 守「切换确认弹窗按平台给不同警告」这件事。
 *
 * ## 为什么这条值得单独立一道闸
 *
 * 这个弹窗是**强制关闭 ChatGPT 的知情同意书** —— 用户点「退出并切换」之后，
 * Windows 上会 `taskkill /F` 直接终止它（那边没有任何优雅手段：`WM_CLOSE` 被
 * minimize-to-tray 吃掉、官方无 reload 接口、当不了父进程 —— 逐条实测见
 * `src-tauri/src/operator/chatgpt_app.rs` 的模块文档）。
 *
 * 而 macOS 那句 `declineNote` 承诺的是「有进行中的对话会弹确认框，在那里点取消
 * 就会中止」。**把那句话摆给 Windows 用户看就是骗他签字**：那边不会弹任何确认框，
 * 点下去就直接杀。所以两个平台的文案必须分开，且 Windows 那句必须说清
 * 「强制关闭、不会提示保存」。
 *
 * 用读源码断言的形式（与 `ProviderCardLayout.test.ts` 同一套路）而不是渲染组件：
 * 要守的是**分支存在**这件事，而 jsdom 里 `navigator.userAgent` 是固定的，
 * 渲染只能覆盖其中一个平台分支。
 */
const dialogSource = readFileSync(
  resolve(
    __dirname,
    "../../src/components/operator/SwitchTierConfirmDialog.tsx",
  ),
  "utf-8",
);

const zh = JSON.parse(
  readFileSync(resolve(__dirname, "../../src/i18n/locales/zh.json"), "utf-8"),
);

describe("切换确认弹窗的平台文案", () => {
  it("按平台在 forceKillWarning 与 declineNote 之间二选一", () => {
    // 三样都得在：判断、Windows 那句、非 Windows 那句。
    expect(dialogSource).toMatch(/isWindows/);
    expect(dialogSource).toContain("loongport.quitConfirm.forceKillWarning");
    expect(dialogSource).toContain("loongport.quitConfirm.declineNote");

    // 而且必须是**同一个三元表达式的两个分支** —— 两句都无条件渲染的话，
    // Windows 用户会同时看到「会弹确认框可以取消」和「强制关闭不提示保存」，
    // 自相矛盾。
    expect(dialogSource).toMatch(
      /\?\s*t\("loongport\.quitConfirm\.forceKillWarning"\)\s*:\s*t\("loongport\.quitConfirm\.declineNote"\)/,
    );
  });

  it("条件的极性必须对：Windows 走 forceKillWarning，不是反过来", () => {
    // ⚠️ 上一条只匹配 `? … : …` 那一段，**匹配不到条件本身** ——
    // 变异测试实证：把条件改成 `!onWindows` 或 `isMac()`，上一条照样绿。
    // 而极性反了正是这条弹窗最不能出的错：Windows 用户会看到
    // 「会弹确认框、可以取消」的承诺，然后点下去被直接强杀。
    expect(dialogSource).toMatch(
      /\{onWindows\s*\n?\s*\?\s*t\("loongport\.quitConfirm\.forceKillWarning"\)/,
    );
    // 且那个变量必须真的来自 isWindows()，不能是 isMac() 之类。
    expect(dialogSource).toMatch(/const\s+onWindows\s*=\s*isWindows\(\)/);
  });

  it("Windows 那句必须点明「强制」且提醒自己保存", () => {
    const msg = zh.loongport.quitConfirm.forceKillWarning as string;
    expect(msg).toContain("强制");
    // 不提醒保存，这条弹窗就不成立为知情同意 —— 强杀不给 app 保存的机会。
    expect(msg).toContain("保存");
  });

  it("declineNote 仍描述 macOS 的确认框语义，别被改成通用文案", () => {
    // 它是 macOS 专属的承诺（AppleScript quit 会触发 app 自己的确认框）。
    // 若有人把它改成「可能会…」这类模糊说法以图两边通用，这条会红 ——
    // 那样等于两个平台都说不准。
    const msg = zh.loongport.quitConfirm.declineNote as string;
    expect(msg).toContain("确认框");
    expect(msg).toContain("取消");
  });

  it("必须有一个「取消」按钮，且它不执行切换", () => {
    // ⚠️ 原来只有两个按钮（「只切换」/「退出并切换」），**两个都会切换** ——
    // 用户改主意了只剩 Esc 或点遮罩。对「会强制关闭 ChatGPT」这种不可逆动作，
    // 退路必须是看得见的按钮。
    expect(dialogSource).toContain('t("common.cancel")');
    // 它必须接 onCancel，不能接 onSwitch —— 接错就是「点取消反而切了」。
    expect(dialogSource).toMatch(
      /onClick=\{onCancel\}[\s\S]{0,80}t\("common\.cancel"\)/,
    );
    // 三个按钮都在。
    for (const key of [
      "common.cancel",
      "loongport.quitConfirm.switchOnly",
      "loongport.quitConfirm.quitAndSwitch",
    ]) {
      expect(dialogSource).toContain(`t("${key}")`);
    }
  });

  it("platformNote 不再声称「部分系统无法代为退出」", () => {
    // 那句写于 Windows 还没实现自动退出时。现在两个平台都能关，
    // 它该退回成「万一没关掉」的兜底说明；留着旧话会让 Windows 用户
    // 以为点了也不会关，从而对强制关闭毫无防备。
    const msg = zh.loongport.quitConfirm.platformNote as string;
    expect(msg).not.toContain("部分系统");
    expect(msg).toMatch(/万一|如果/);
  });
});
