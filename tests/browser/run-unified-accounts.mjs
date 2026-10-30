import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
import { fileURLToPath } from "node:url";
import { assertScreenshotSource } from "./screenshot-source.mjs";
import {
  platformAccountFilterName,
  overviewAccountDetailsName,
  nativeWorkspaceAccountName,
  nativeAccountKeyboardSteps,
  nativeAccountOverviewCounts,
} from "./unified-account-locators.mjs";
import { mkdir, readFile, writeFile } from "node:fs/promises";
import path from "node:path";

// Kept here so local regression can exercise this exact browser evidence reader.
function readFocusEvidence() {
  const e = document.activeElement,
    b = e.getBoundingClientRect();
  const labelledBy = e.getAttribute("aria-labelledby");
  const tab = labelledBy ? document.getElementById(labelledBy) : null;
  const activeTabPanel =
    e.matches('[role="tabpanel"][tabindex="0"][data-state="active"]') &&
    !e.closest("[hidden],[inert]") &&
    getComputedStyle(e).display !== "none" &&
    getComputedStyle(e).visibility !== "hidden" &&
    Boolean(
      e.id &&
      tab?.getAttribute("role") === "tab" &&
      tab.getAttribute("aria-selected") === "true" &&
      tab.getAttribute("aria-controls") === e.id,
    );

  return {
    label: e.getAttribute("aria-label") || e.textContent.trim(),
    role: e.getAttribute("role"),
    id: e.id,
    labelledBy,
    activeTabPanel,
    inMain: document.querySelector("main").contains(e),
    interactive: e.matches(
      "button:not(:disabled),input:not(:disabled),select:not(:disabled),textarea:not(:disabled),a[href],[role=tab],[role=combobox]",
    ),
    enabled:
      !e.hasAttribute("disabled") && e.getAttribute("aria-disabled") !== "true",
    tag: e.tagName,
    top: b.top,
    bottom: b.bottom,
    hit: e.contains(
      document.elementFromPoint(b.left + b.width / 2, b.top + b.height / 2),
    ),
  };
}
function isReachableFocus(evidence, height) {
  return (
    evidence.inMain &&
    (evidence.interactive || evidence.activeTabPanel) &&
    evidence.enabled &&
    evidence.tag !== "BODY" &&
    evidence.top >= 0 &&
    evidence.bottom <= height &&
    evidence.hit
  );
}

const { chromium } = await import(
  process.env.PLAYWRIGHT_MODULE ?? "playwright"
);
const output =
  process.env.UNIFIED_LAYOUT_OUTPUT ?? "artifacts/unified-accounts";
const source = process.env.UNIFIED_LAYOUT_SOURCE_SHA;
// Local scoped runs do not replace the default full CI suite.
const qoderOnly = process.env.UNIFIED_LAYOUT_QODER_ONLY === "1";
assert.match(
  source ?? "",
  /^[0-9a-f]{40}$/,
  "Exact current-checkout source SHA is required",
);
assert.equal(
  execFileSync("git", ["rev-parse", "HEAD"], { encoding: "utf8" }).trim(),
  source,
  "Screenshot source must be the current checkout",
);
const response = await fetch(
  "http://127.0.0.1:4314/unified-source-evidence.json",
);
if (!response.ok)
  throw new Error("Screenshot build source evidence is unavailable");
const builtSource = await response.json();
const sourceEvidence = assertScreenshotSource(
  fileURLToPath(new URL("../..", import.meta.url)),
  builtSource,
  { strict: process.env.UNIFIED_LAYOUT_STRICT === "1" },
);
await mkdir(output, { recursive: true });
await writeFile(path.join(output, "source-sha.txt"), `${source}\n`);
await writeFile(
  path.join(output, "source-evidence.json"),
  JSON.stringify(
    {
      ...sourceEvidence,
      coverage: qoderOnly ? "qoder-only" : "full",
      builtDirtyPaths: builtSource.dirtyPaths,
    },
    null,
    2,
  ),
);
const coverage = qoderOnly ? "qoder-only" : "full";
const verificationPath = path.join(output, "verification-status.json");
await writeFile(
  verificationPath,
  JSON.stringify({ status: "running", sourceEvidence, coverage }, null, 2),
);
const browser = await chromium.launch({
  headless: true,
  chromiumSandbox: true,
  ...(process.env.BROWSER_EXECUTABLE
    ? { executablePath: process.env.BROWSER_EXECUTABLE }
    : {}),
});
const results = [];
let completed = false;
try {
  for (const variant of [
    { width: 1440, height: 800, language: "en", dark: false },
    { width: 959, height: 650, language: "zh", dark: true },
  ]) {
    const translations = JSON.parse(
      await readFile(
        new URL(
          `../../src/i18n/locales/${variant.language}.json`,
          import.meta.url,
        ),
        "utf8",
      ),
    );
    const t = (key) =>
      key.split(".").reduce((value, part) => value[part], translations);
    for (const scenario of qoderOnly
      ? []
      : ["empty", "populated", "add", "native"]) {
      const context = await browser.newContext({
        viewport: { width: variant.width, height: variant.height },
      });
      const errors = [],
        unexpectedRequests = [];
      await context.route("**/*", (route) => {
        const url = new URL(route.request().url());
        if (url.hostname === "127.0.0.1" && url.port === "4314")
          return route.continue();
        unexpectedRequests.push(url.origin);
        return route.abort();
      });
      const page = await context.newPage();
      page.on("pageerror", (error) => errors.push(error.message));
      page.on("console", (message) => {
        if (message.type() === "error") errors.push(message.text());
      });
      let geometry;
      const focus = [];
      const controls = [];
      try {
        await page.goto(
          `http://127.0.0.1:4314/tests/browser/unified-accounts.html?scenario=${scenario}&lang=${variant.language}&dark=${variant.dark}`,
        );
        const main = page.locator("main");
        if (scenario === "native") {
          const detail = main.getByRole("region", {
            name: "账号详情",
            exact: true,
          });
          await main
            .getByRole("button", {
              name: overviewAccountDetailsName(t, "Synthetic account B"),
              exact: true,
            })
            .waitFor();
          assert.equal(
            await detail.count(),
            0,
            "All must show summaries without visible native workspaces",
          );
          const overviewRow = main.getByRole("article", {
            name: "Synthetic account B",
            exact: true,
          });
          assert.equal(
            await overviewRow
              .getByRole("button", {
                name: overviewAccountDetailsName(t, "Synthetic account B"),
                exact: true,
              })
              .count(),
            1,
            "The independent Details entry belongs to the exact account row",
          );
          assert.equal(
            await overviewRow.locator("button button").count(),
            0,
            "Row controls must not nest buttons",
          );
          assert.equal(
            await overviewRow
              .getByRole("button", {
                name: t("accountsOverview.switch"),
                exact: true,
              })
              .isDisabled(),
            true,
            "Unavailable client switching must stay disabled",
          );

          await page.waitForFunction(() => window.unifiedLayout.settled());
          await page.evaluate(() => document.fonts.ready);
          await page.screenshot({
            path: path.join(
              output,
              `native-overview-${variant.width}-${variant.language}-${variant.dark ? "dark" : "light"}.png`,
            ),
            fullPage: true,
          });
          await main
            .getByRole("button", {
              name: overviewAccountDetailsName(t, "Synthetic account B"),
              exact: true,
            })
            .click();
          await detail
            .getByRole("heading", { name: "Synthetic account B", exact: true })
            .waitFor();
          assert.equal(
            await main
              .getByRole("button", {
                name: nativeWorkspaceAccountName("Synthetic account B"),
                exact: true,
              })
              .getAttribute("aria-pressed"),
            "true",
          );
          assert.equal(
            await detail
              .getByRole("heading", {
                name: "Synthetic account A",
                exact: true,
              })
              .count(),
            0,
          );
        } else {
          await main
            .getByRole(scenario === "add" ? "button" : "tab", {
              name: "MiniMax Code 国内版",
              exact: true,
            })
            .waitFor();
        }
        await page.waitForFunction(() => window.unifiedLayout.settled());
        await page.evaluate(() => document.fonts.ready);
        if (scenario === "empty" || scenario === "populated") {
          // Relay records live in their own tab in the approved account layout.
          await main
            .getByRole("tab", {
              name: t("loongport.accounts.relayTab") ?? "中转站",
              exact: true,
            })
            .click();
        }
        if (scenario === "populated") {
          await main
            .getByText(
              "Synthetic relay with a deliberately long display name",
              {
                exact: true,
              },
            )
            .waitFor();
          await main
            .getByRole("tab", {
              name: t("loongport.accounts.platformAccounts"),
              exact: true,
            })
            .click();
          await main
            .getByText("Synthetic workspace account", {
              exact: true,
            })
            .waitFor();
        }
        if (scenario === "empty") {
          await main
            .getByText(t("loongport.accounts.empty"), { exact: true })
            .waitFor();
          assert.equal(
            await main
              .getByText(
                "Synthetic relay with a deliberately long display name",
                { exact: true },
              )
              .count(),
            0,
          );
          assert.equal(
            await main
              .getByText("Synthetic workspace account", {
                exact: true,
              })
              .count(),
            0,
          );
        }
        if (scenario === "empty") {
          await main
            .getByRole("tab", {
              name: t("loongport.accounts.platformAccounts"),
              exact: true,
            })
            .click();
        }
        geometry = await page.evaluate(() => {
          const bounds = (selector) => {
            const b = document.querySelector(selector).getBoundingClientRect();
            return { x: b.x, y: b.y, width: b.width, height: b.height };
          };
          return {
            sidebar: bounds("aside"),
            header: bounds("[data-layout-header]"),
            main: bounds("main"),
            scrollWidth: document.documentElement.scrollWidth,
            viewport: innerWidth,
            contentWidths: [
              ...document.querySelectorAll("main, main .page-content"),
            ].map((e) => ({ scroll: e.scrollWidth, client: e.clientWidth })),
          };
        });
        assert.equal(geometry.sidebar.width, variant.width < 960 ? 72 : 200);
        assert.equal(geometry.main.x, geometry.sidebar.width);
        assert.equal(
          await page.locator("[data-layout-header]").isHidden(),
          scenario === "add",
        );
        assert.ok(
          geometry.main.y >= geometry.header.y + geometry.header.height,
        );
        for (const width of geometry.contentWidths)
          assert.ok(
            width.scroll <= width.client + 1,
            "Main content must not be horizontally clipped",
          );
        assert.ok(
          geometry.scrollWidth <= geometry.viewport,
          "Page must not overflow horizontally",
        );
        // Real Tab navigation must keep enabled controls hit-visible while scrolling.
        const first = main.getByRole("button").first();
        await first.focus();
        for (
          let i = 0;
          i < 12 && focus.filter((item) => item.interactive).length < 5;
          i++
        ) {
          const evidence = await page.evaluate(readFocusEvidence);
          assert.ok(
            isReachableFocus(evidence, variant.height),
            `Unreachable focus: ${JSON.stringify(evidence)}`,
          );
          focus.push(evidence);
          await page.keyboard.press("Tab");
        }
        assert.ok(
          new Set(
            focus.filter((item) => item.interactive).map((item) => item.label),
          ).size >= 3,
          "Keyboard must visit distinct real controls",
        );
        assert.equal(
          focus.filter((item) => item.interactive).length,
          5,
          "Tab must reach five actual controls; panel stops do not count",
        );
        if (scenario === "add") {
          for (const name of ["GitHub Copilot", "ChatGPT"])
            assert.ok(
              focus.some((item) => item.interactive && item.label === name),
              `Tab must reach platform button ${name} after the panel`,
            );
        }
        const critical = main.locator(
          "button, .page-content h2, .page-content h3, .page-content h4",
        );
        for (const control of await critical.all()) {
          if (!(await control.isVisible())) continue;
          await control.scrollIntoViewIfNeeded();
          const box = await control.boundingBox();
          assert.ok(
            box.y >= geometry.main.y &&
              box.y + box.height <= variant.height + 1,
            "Scrolled control/title must fit visible content",
          );
          const state = await control.evaluate((e) => {
            const r = e.getBoundingClientRect();
            return {
              label: e.getAttribute("aria-label") || e.textContent.trim(),
              disabled:
                e.matches(":disabled") ||
                e.getAttribute("aria-disabled") === "true",
              pointerEvents: getComputedStyle(e).pointerEvents,
              hit: e.contains(
                document.elementFromPoint(
                  r.left + r.width / 2,
                  r.top + r.height / 2,
                ),
              ),
            };
          });
          controls.push({ ...state, ...box });
          // Disabled actions intentionally have pointer-events:none in the real Button.
          // Their layout still matters, but they must not be required to accept clicks.
          if (!state.disabled) {
            assert.notEqual(
              state.pointerEvents,
              "none",
              "Enabled control/title must accept pointer hits",
            );
            assert.equal(
              state.hit,
              true,
              "Enabled control/title must have a real hit target",
            );
          }
          assert.ok(
            box.x >= geometry.main.x && box.x + box.width <= variant.width + 1,
            "Action must not be horizontally clipped",
          );
        }
        await main.evaluate((node) => node.scrollTo(0, 0));
        if (scenario === "populated")
          await main
            .getByText("Synthetic workspace account", {
              exact: true,
            })
            .scrollIntoViewIfNeeded();
        const filename = `${scenario}-${variant.width}-${variant.language}-${variant.dark ? "dark" : "light"}.png`;
        await page.screenshot({
          path: path.join(output, filename),
          fullPage: false,
        });
        if (scenario === "add") {
          // Verify actual typed destinations, not obsolete generic-tab redirects.
          for (const name of ["Qoder", "OpenRouter · API Key", "OpenCode"]) {
            await main.getByRole("button", { name, exact: true }).click();
            const destination = main.getByRole("region", {
              name: "服务接入",
              exact: true,
            });
            await destination.waitFor();
            if (name === "Qoder") {
              const qoderAccounts = destination.getByRole("region", {
                name: "Qoder 账号管理",
                exact: true,
              });
              await qoderAccounts
                .getByRole("heading", {
                  name: "Qoder 账号（国际版 / CN）",
                  exact: true,
                })
                .waitFor();
              assert.equal(
                await qoderAccounts
                  .getByText(/不会以自定义 API 或 SDK\s*配置代替账号接入/)
                  .count(),
                1,
                "Qoder account entry must preserve the native-account boundary",
              );
              assert.equal(
                await destination.getByRole("textbox").count(),
                0,
                "Qoder account entry must not masquerade as a custom API form",
              );
            } else if (name === "OpenRouter · API Key") {
              const providerName = destination.locator('input[name="name"]');
              await providerName.waitFor();
              await page.waitForFunction(
                () =>
                  document.querySelector('input[name="name"]')?.value ===
                    "OpenRouter" &&
                  document.querySelector("#codexBaseUrl")?.value ===
                    "https://openrouter.ai/api/v1",
              );
              assert.equal(
                await providerName.inputValue(),
                "OpenRouter",
                "Original OpenRouter preset must be selected",
              );
              assert.equal(
                await destination.locator("#codexBaseUrl").inputValue(),
                "https://openrouter.ai/api/v1",
              );
            } else {
              await destination
                .getByRole("heading", {
                  name: t("loongport.officialApi.title"),
                  exact: true,
                })
                .waitFor();
              await destination
                .getByRole("button", {
                  name: /官网账户登录（不会自动获取密钥）/,
                })
                .waitFor();
              const personalKey = destination.getByRole("region", {
                name: "OpenCode 个人密钥接入",
                exact: true,
              });
              assert.equal(
                await personalKey
                  .getByRole("button", {
                    name: "同意加密保存并添加 OpenCode",
                    exact: true,
                  })
                  .isDisabled(),
                true,
                "An empty personal key cannot be imported without explicit input",
              );
              const localImport = destination.getByRole("region", {
                name: "从本机 OpenCode 导入",
                exact: true,
              });
              // Enumerate the actual actions, including the explicit local-read
              // consent. A raw historical count missed that shipped entry.
              const actions = [
                destination.getByRole("button", {
                  name: "返回平台列表",
                  exact: true,
                }),
                localImport.getByRole("button", {
                  name: "同意只读预览本机 OpenCode 凭据",
                  exact: true,
                }),
                personalKey.getByRole("button", {
                  name: "同意加密保存并添加 OpenCode",
                  exact: true,
                }),
                destination.getByRole("button", {
                  name: /官网账户登录（不会自动获取密钥）/,
                }),
              ];
              for (const action of actions) {
                assert.equal(await action.count(), 1, "Each action is unique");
                assert.equal(await action.isVisible(), true);
              }
              assert.equal(await actions[1].isEnabled(), true);
              assert.deepEqual(
                await destination.getByRole("button").allTextContents(),
                await Promise.all(
                  actions.map((action) => action.textContent()),
                ),
                "Only Back, local-read consent, personal-key consent and the original vendor action belong to OpenCode",
              );
              assert.equal(
                await destination
                  .getByRole("region", {
                    name: "Qoder 账号管理",
                    exact: true,
                  })
                  .count(),
                0,
                "The previous Qoder destination must not leak into OpenCode",
              );
            }
            await destination
              .getByRole("button", { name: "返回平台列表", exact: true })
              .click();
            await main
              .getByRole("button", { name: "Qoder", exact: true })
              .waitFor();
            assert.equal(
              await main
                .getByRole("region", { name: "服务接入", exact: true })
                .count(),
              0,
              "Returning to the platform list must retire the source destination",
            );
          }
          await page
            .getByRole("tab", {
              name: t("loongport.sections.relay"),
              exact: true,
            })
            .click();
          assert.equal(
            await page
              .getByRole("tab", {
                name: t("loongport.sections.relay"),
                exact: true,
              })
              .getAttribute("aria-selected"),
            "true",
          );
          await page.waitForFunction(() => window.unifiedLayout.settled());
          assert.ok(
            (await main.getByRole("textbox").count()) > 0,
            "Real relay directory must render its input",
          );
          await page
            .getByRole("button", { name: t("common.back"), exact: true })
            .click();
          await page.locator('main[data-layout-view="services"]').waitFor();
          await main
            .getByRole("tab", { name: "MiniMax Code 国内版", exact: true })
            .waitFor();
        } else if (scenario === "native") {
          const platformTabs = main.getByRole("tablist", {
            name: platformAccountFilterName(t),
            exact: true,
          });
          // Exercise the real status hook for both providers. The fixture exposes
          // only local synthetic snapshots/events; any unexpected IPC still fails below.
          for (const [provider, name] of [
            ["minimaxCode", "MiniMax Code 国内版"],
            ["workbuddy", "WorkBuddy"],
          ]) {
            await platformTabs.getByRole("tab", { name, exact: true }).click();
            await main
              .getByRole("button", {
                name:
                  provider === "minimaxCode"
                    ? overviewAccountDetailsName(t, "Synthetic account A")
                    : overviewAccountDetailsName(
                        t,
                        "Synthetic WorkBuddy account",
                      ),
                exact: true,
              })
              .click();
            const status = main.getByRole("region", {
              name: t("automaticStatus.title"),
              exact: true,
            });
            await status
              .getByText(t("automaticStatus.waiting"), { exact: true })
              .waitFor();
            await page.waitForFunction(
              (provider) =>
                window.unifiedLayout.automaticStatusReads[provider] >= 2,
              provider,
            );
            const before = await page.evaluate(
              (provider) => window.unifiedLayout.automaticStatusReads[provider],
              provider,
            );
            await page.evaluate(
              (provider) =>
                window.unifiedLayout.completeAutomaticCheck(provider),
              provider,
            );
            await status
              .getByText(t("automaticStatus.reason.disabled"), { exact: true })
              .waitFor();
            assert.ok(
              await page.evaluate(
                ({ provider, before }) =>
                  window.unifiedLayout.automaticStatusReads[provider] > before,
                { provider, before },
              ),
              `${provider} completion must re-read the native status DTO`,
            );
          }
          await platformTabs
            .getByRole("tab", { name: "MiniMax Code 国内版", exact: true })
            .click();
          await platformTabs
            .getByRole("tab", { name: "MiniMax Code 国内版", exact: true })
            .focus();
          for (const [key, name] of nativeAccountKeyboardSteps(t)) {
            await page.keyboard.press(key);
            const selected = platformTabs.getByRole("tab", {
              name,
              exact: true,
            });
            await page.waitForFunction(
              (name) =>
                document.activeElement?.getAttribute("aria-label") === name &&
                document.activeElement?.getAttribute("aria-selected") ===
                  "true",
              name,
            );
            assert.equal(await selected.getAttribute("aria-selected"), "true");
            assert.equal(
              await selected.evaluate(
                (node) => document.activeElement === node,
              ),
              true,
            );
            assert.ok(
              isReachableFocus(
                await page.evaluate(readFocusEvidence),
                variant.height,
              ),
              `Platform ${name} must remain keyboard reachable and hit-visible`,
            );
          }
          await platformTabs
            .getByRole("tab", {
              name: t("loongport.accounts.allPlatforms"),
              exact: true,
            })
            .click();
          await main
            .getByRole("tab", { name: "MiniMax Code 国内版", exact: true })
            .waitFor();
          assert.equal(
            await platformTabs
              .getByRole("tab", {
                name: t("loongport.accounts.allPlatforms"),
                exact: true,
              })
              .getAttribute("aria-selected"),
            "true",
          );
          for (const name of [
            overviewAccountDetailsName(t, "Synthetic account A"),
            overviewAccountDetailsName(t, "Synthetic WorkBuddy account"),
          ])
            await main.getByRole("button", { name, exact: true }).waitFor();
          assert.equal(
            await main
              .getByRole("region", { name: "账号详情", exact: true })
              .count(),
            nativeAccountOverviewCounts.details,
          );
          for (const name of [
            overviewAccountDetailsName(t, "Synthetic account A"),
            overviewAccountDetailsName(t, "Synthetic account B"),
          ])
            assert.equal(
              await main.getByRole("button", { name, exact: true }).count(),
              nativeAccountOverviewCounts.accountButtons,
            );
        }
        await page.waitForFunction(() => window.unifiedLayout.settled());
        assert.deepEqual(
          await page.evaluate(() => window.unifiedLayout.rejectedCommands),
          [],
        );
        assert.deepEqual(errors, []);
        assert.deepEqual(unexpectedRequests, []);
        results.push({
          source,
          sourceEvidence,
          scenario,
          ...variant,
          filename,
          geometry,
          focus,
          controls,
          errors,
          unexpectedRequests,
        });
        await writeFile(
          path.join(output, "results.json"),
          JSON.stringify(results, null, 2),
        );
      } catch (error) {
        const failureName = `failure-${scenario}-${variant.width}-${variant.language}`;
        await page
          .screenshot({ path: path.join(output, `${failureName}.png`) })
          .catch(() => {});
        const rejectedCommands = await page
          .evaluate(() => window.unifiedLayout?.rejectedCommands ?? [])
          .catch(() => []);
        await writeFile(
          path.join(output, `${failureName}.json`),
          JSON.stringify(
            {
              source,
              scenario,
              ...variant,
              status: "failed",
              message: String(error),
              geometry,
              focus,
              controls,
              errors,
              unexpectedRequests,
              rejectedCommands,
            },
            null,
            2,
          ),
        );
        throw error;
      } finally {
        await context.close();
      }
    }
  }
  // Capture the real Qoder account detail in both existing owners. Fixture IPC
  // is synthetic and the navigation callback only records the requested app.
  for (const variant of [
    { width: 1440, height: 800, language: "en", dark: false },
    { width: 959, height: 650, language: "zh", dark: true },
  ]) {
    const translations = JSON.parse(
      await readFile(
        new URL(
          `../../src/i18n/locales/${variant.language}.json`,
          import.meta.url,
        ),
        "utf8",
      ),
    );
    const labels = translations.qoderApplicationEntry;
    for (const entry of ["qoder-services", "qoder-add"]) {
      for (const state of ["bound", "unknown", "error"]) {
        const context = await browser.newContext({
          viewport: { width: variant.width, height: variant.height },
        });
        const errors = [],
          unexpectedRequests = [];
        await context.route("**/*", (route) => {
          const url = new URL(route.request().url());
          if (
            url.hostname === "127.0.0.1" &&
            url.port === "4314" &&
            route.request().method() === "GET"
          )
            return route.continue();
          unexpectedRequests.push(url.origin);
          return route.abort();
        });
        const page = await context.newPage();
        page.on("pageerror", (error) => errors.push(error.message));
        page.on("console", (message) => {
          if (message.type() === "error") errors.push(message.text());
        });
        try {
          await page.goto(
            `http://127.0.0.1:4314/tests/browser/unified-accounts.html?scenario=${entry}&qoderState=${state}&lang=${variant.language}&dark=${variant.dark}`,
          );
          await page.waitForFunction(() => window.unifiedLayout?.checkout);
          assert.equal(
            await page.evaluate(() => window.unifiedLayout.checkout),
            builtSource.head,
            "Served build must match its source evidence",
          );
          const panel = page.getByRole("region", {
            name: labels.title,
            exact: true,
          });
          await panel.waitFor();
          const content =
            state === "bound"
              ? "synthetic-qoder-model"
              : state === "unknown"
                ? labels.empty
                : labels.error;
          await panel.getByText(content, { exact: true }).waitFor();
          assert.equal(
            await page
              .getByRole("region", { name: "用于哪些应用", exact: true })
              .count(),
            0,
            "Qoder must not duplicate the generic section",
          );
          assert.equal(
            await panel
              .getByText("synthetic-other-model", { exact: true })
              .count(),
            0,
            "Another Provider binding must not leak",
          );
          const binding = panel.getByRole("button", {
            name: /synthetic-qoder-model/,
          });
          const fallback = panel.getByRole("button", {
            name: labels.defaultNavigationNamed.replace("{{app}}", "Codex"),
            exact: true,
          });
          assert.equal(await binding.count(), state === "bound" ? 1 : 0);
          assert.equal(await fallback.count(), state === "bound" ? 0 : 1);
          await page.evaluate(() => document.fonts.ready);
          await panel.scrollIntoViewIfNeeded();
          const geometry = await panel.evaluate((el) => {
            const rect = el.getBoundingClientRect(),
              clipped = [];
            const walker = document.createTreeWalker(el, NodeFilter.SHOW_TEXT);
            for (let node = walker.nextNode(); node; node = walker.nextNode()) {
              if (!node.textContent.trim()) continue;
              const range = document.createRange();
              range.selectNodeContents(node);
              for (const box of range.getClientRects()) {
                if (!box.width) continue;
                const bounds = {
                  left: 0,
                  top: 0,
                  right: innerWidth,
                  bottom: innerHeight,
                };
                for (
                  let parent = node.parentElement;
                  parent;
                  parent = parent.parentElement
                ) {
                  const style = getComputedStyle(parent),
                    r = parent.getBoundingClientRect();
                  if (/hidden|clip|auto|scroll/.test(style.overflowX)) {
                    bounds.left = Math.max(bounds.left, r.left);
                    bounds.right = Math.min(bounds.right, r.right);
                  }
                  if (/hidden|clip|auto|scroll/.test(style.overflowY)) {
                    bounds.top = Math.max(bounds.top, r.top);
                    bounds.bottom = Math.min(bounds.bottom, r.bottom);
                  }
                }
                if (
                  box.left < bounds.left - 0.5 ||
                  box.right > bounds.right + 0.5 ||
                  box.top < bounds.top - 0.5 ||
                  box.bottom > bounds.bottom + 0.5
                )
                  clipped.push(node.textContent.trim());
              }
            }
            return {
              x: rect.x,
              y: rect.y,
              width: rect.width,
              height: rect.height,
              clipped,
            };
          });
          assert.deepEqual(
            geometry.clipped,
            [],
            "Qoder entry text must not be clipped",
          );
          const filename = `${entry}-${state}-${variant.width}-${variant.language}-${variant.dark ? "dark" : "light"}.png`;
          await page.screenshot({
            path: path.join(output, filename),
            fullPage: false,
          });
          await (state === "bound" ? binding : fallback).click();
          if (entry === "qoder-services")
            await page
              .getByRole("button", { name: "管理原有 Qoder 配置", exact: true })
              .click();
          assert.deepEqual(
            await page.evaluate(() => window.unifiedLayout.navigationCalls),
            entry === "qoder-services" ? ["codex", "codex"] : ["codex"],
          );
          assert.deepEqual(
            await page.evaluate(() => window.unifiedLayout.rejectedCommands),
            [],
          );
          assert.deepEqual(errors, []);
          assert.deepEqual(unexpectedRequests, []);
          const screenshotSha256 = createHash("sha256")
            .update(await readFile(path.join(output, filename)))
            .digest("hex");
          results.push({
            source,
            sourceEvidence,
            scenario: entry,
            state,
            ...variant,
            filename,
            screenshotSha256,
            geometry,
            components: [
              "src/components/relay/accounts/QoderAccountPanel.tsx",
              "src/components/relay/accounts/QoderApplicationEntry.tsx",
              entry === "qoder-services"
                ? "src/components/relay/accounts/ServicesPage.tsx"
                : "src/components/relay/AddHubPage.tsx",
            ],
            errors,
            unexpectedRequests,
          });
          await writeFile(
            path.join(output, "results.json"),
            JSON.stringify(results, null, 2),
          );
        } catch (error) {
          const failureName = `failure-${entry}-${state}-${variant.width}-${variant.language}`;
          await page
            .screenshot({ path: path.join(output, `${failureName}.png`) })
            .catch(() => {});
          await writeFile(
            path.join(output, `${failureName}.json`),
            JSON.stringify(
              {
                source,
                entry,
                state,
                ...variant,
                error: String(error),
                errors,
                unexpectedRequests,
              },
              null,
              2,
            ),
          );
          throw error;
        } finally {
          await context.close();
        }
      }
    }
  }
  assertScreenshotSource(
    fileURLToPath(new URL("../..", import.meta.url)),
    builtSource,
    { strict: process.env.UNIFIED_LAYOUT_STRICT === "1" },
  );
  completed = true;
  await writeFile(
    verificationPath,
    JSON.stringify(
      {
        status: "passed",
        sourceEvidence,
        coverage,
        screenshots: results.length,
      },
      null,
      2,
    ),
  );
  console.log(
    JSON.stringify({
      source,
      sourceEvidence,
      coverage,
      screenshots: results.length,
      status: "passed",
    }),
  );
} finally {
  if (!completed)
    await writeFile(
      verificationPath,
      JSON.stringify(
        {
          status: "failed",
          sourceEvidence,
          coverage,
          screenshots: results.length,
        },
        null,
        2,
      ),
    );
  await browser.close();
}
