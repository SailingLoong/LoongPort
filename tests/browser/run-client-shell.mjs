// Start: node node_modules/vite/bin/vite.js --config tests/browser/vite.config.mjs
// Run with PLAYWRIGHT_MODULE pointing to an installed Playwright runtime.
// Isolated renderer verification, not native GUI or full-workspace acceptance.
import assert from "node:assert/strict";
import { mkdir } from "node:fs/promises";
import path from "node:path";

const { chromium } = await import(
  process.env.PLAYWRIGHT_MODULE ?? "playwright"
);
const output = process.env.SHELL_LAYOUT_OUTPUT ?? "artifacts/client-shell";
await mkdir(output, { recursive: true });
const browser = await chromium.launch({
  headless: true,
  chromiumSandbox: true,
  ...(process.env.BROWSER_EXECUTABLE
    ? { executablePath: process.env.BROWSER_EXECUTABLE }
    : {}),
});

try {
  const results = [];
  for (const options of [
    { width: 1440, height: 800, language: "en", dark: false },
    { width: 1000, height: 650, language: "en", dark: false },
    { width: 959, height: 650, language: "en", dark: false },
    { width: 959, height: 650, language: "zh", dark: true },
  ]) {
    const context = await browser.newContext({ viewport: options });
    await context.route("**/*", (route) => {
      const url = new URL(route.request().url());
      return url.hostname === "127.0.0.1" && url.port === "4314"
        ? route.continue()
        : route.abort();
    });
    const page = await context.newPage();
    const errors = [];
    page.on("pageerror", (error) => errors.push(error.message));
    await page.goto(
      `http://127.0.0.1:4314/tests/browser/client-shell.html?lang=${options.language}`,
    );
    const nav = page.getByRole("navigation");
    await nav.waitFor();
    if (options.dark)
      await page.evaluate(() => document.documentElement.classList.add("dark"));
    const expectedWidth = options.width < 960 ? 72 : 200;
    const geometry = async () =>
      page.evaluate(() => {
        const box = (selector) => {
          const bounds = document
            .querySelector(selector)
            .getBoundingClientRect();
          return {
            x: bounds.x,
            y: bounds.y,
            width: bounds.width,
            height: bounds.height,
          };
        };
        return {
          sidebar: box("aside"),
          header: box("header"),
          main: box("main"),
          content: box(".page-content"),
        };
      });
    let bounds = await geometry();
    assert.equal(bounds.sidebar.width, expectedWidth);
    assert.equal(bounds.sidebar.y, 28);
    assert.equal(bounds.header.x, expectedWidth);
    assert.equal(bounds.header.height, 52);
    assert.equal(bounds.main.x, expectedWidth);
    assert.ok(bounds.content.width <= 1180);
    assert.equal(await nav.getByRole("button").count(), 7);
    const labels =
      options.language === "zh"
        ? [
            "应用",
            "服务与账号",
            "生图",
            "使用记录",
            "用量",
            "扩展资源",
            "中转站广场",
          ]
        : [
            "Applications",
            "Services & accounts",
            "Images",
            "Activity",
            "Usage",
            "Resources",
            "Relay directory",
          ];
    for (const label of labels)
      assert.equal(
        await nav.getByRole("button", { name: label, exact: true }).count(),
        1,
      );
    await page.screenshot({
      path: path.join(
        output,
        `initial-shell-${options.width}-${options.language}-${options.dark ? "dark" : "light"}.png`,
      ),
    });
    const toggle = page.getByRole("button", {
      name:
        options.language === "zh"
          ? expectedWidth === 72
            ? "展开侧栏"
            : "收起侧栏"
          : expectedWidth === 72
            ? "Expand sidebar"
            : "Collapse sidebar",
    });
    await toggle.click();
    bounds = await geometry();
    assert.equal(bounds.sidebar.width, expectedWidth === 72 ? 200 : 72);
    assert.equal(bounds.header.x, bounds.sidebar.width);
    assert.equal(bounds.main.x, bounds.sidebar.width);
    assert.equal(
      await page
        .locator("[data-navigation-count]")
        .getAttribute("data-navigation-count"),
      "0",
    );
    if (bounds.sidebar.width !== 72) {
      await page.keyboard.down("Control");
      await page.keyboard.press("\\");
      await page.keyboard.up("Control");
    }
    assert.equal((await geometry()).sidebar.width, 72);
    for (const name of ["Import fixture", "Update fixture", "GitHub fixture"]) {
      const button = page.getByRole("button", { name });
      const box = await button.boundingBox();
      assert.ok(box && box.x >= 0 && box.x + box.width <= 72);
      assert.ok(
        box.y >= 28 && box.y + box.height <= options.height,
        `${name} must not be clipped`,
      );
    }
    const hitVisible = async (selector) =>
      page.locator(selector).evaluate((element) => {
        const box = element.getBoundingClientRect();
        const hit = document.elementFromPoint(
          box.x + box.width / 2,
          box.y + box.height / 2,
        );
        return !!hit && (element === hit || element.contains(hit));
      });
    // The preceding click can leave this button focused without opening its tooltip.
    await page.getByRole("main").focus();
    await page
      .getByRole("button", {
        name: options.language === "zh" ? "展开侧栏" : "Expand sidebar",
      })
      .focus();
    await page.getByRole("tooltip").waitFor();
    assert.ok(
      await hitVisible('[role="tooltip"]'),
      "collapse tooltip must paint above the page header",
    );
    await page.keyboard.press("Escape");
    await nav.getByRole("button", { name: labels[1], exact: true }).focus();
    await page.getByRole("tooltip").waitFor();
    assert.ok(
      (await page.getByRole("tooltip").textContent()).includes(labels[1]),
    );
    await page.keyboard.press("Escape");
    const skip = page.getByRole("link", {
      name: options.language === "zh" ? "跳到内容" : "Skip to content",
    });
    await skip.focus();
    assert.ok(
      await hitVisible('a[href="#main-content"]'),
      "focused skip link must paint above the page header",
    );
    await page.keyboard.press("Enter");
    assert.equal(
      await page.evaluate(() => document.activeElement?.id),
      "main-content",
    );
    if (options.width < 1120) {
      assert.ok(
        await page
          .locator("[data-table-scroll]")
          .evaluate((element) => element.scrollWidth > element.clientWidth),
      );
    }
    await page.screenshot({
      path: path.join(
        output,
        `shell-${options.width}-${options.language}-${options.dark ? "dark" : "light"}.png`,
      ),
    });
    assert.deepEqual(errors, []);
    results.push({
      ...options,
      rail: 72,
      header: 52,
      destinations: 7,
      footerVisible: true,
      skipFocus: true,
      noNavigationFromToggle: true,
    });
    await context.close();
  }
  console.log(
    JSON.stringify(
      { scope: "synthetic shell renderer only", results },
      null,
      2,
    ),
  );
} finally {
  await browser.close();
}
