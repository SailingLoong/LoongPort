// Use an existing Playwright runtime and browser. No install or product API calls.
import assert from "node:assert/strict";
import { mkdir, writeFile } from "node:fs/promises";
import path from "node:path";

const { chromium } = await import(
  process.env.PLAYWRIGHT_MODULE ?? "playwright"
);
const output = process.env.UI_FOUNDATION_OUTPUT ?? "artifacts/ui-foundation";
await mkdir(output, { recursive: true });
const browser = await chromium.launch({
  headless: true,
  chromiumSandbox: true,
  ...(process.env.BROWSER_EXECUTABLE
    ? { executablePath: process.env.BROWSER_EXECUTABLE }
    : {}),
});
const results = [];
try {
  for (const options of [
    { width: 1440, height: 800, dark: false },
    { width: 1000, height: 650, dark: false },
    { width: 959, height: 650, dark: true },
  ]) {
    const context = await browser.newContext({
      viewport: options,
      reducedMotion: "reduce",
    });
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
      `http://127.0.0.1:4314/tests/browser/ui-foundation.html?dark=${options.dark}`,
    );
    await page.getByRole("heading", { name: "UI foundation" }).waitFor();
    const heading = await page
      .getByRole("heading", { name: "UI foundation" })
      .evaluate((node) => ({
        fontSize: getComputedStyle(node).fontSize,
        y: node.getBoundingClientRect().y,
      }));
    assert.equal(heading.fontSize, "18px");
    assert.ok(
      heading.y >= 48,
      "The heading must clear the 28px native chrome.",
    );
    const primary = page.getByRole("button", {
      name: "Apply example",
      exact: true,
    });
    await primary.waitFor();
    const styles = await primary.evaluate((node) => {
      const css = getComputedStyle(node);
      return {
        background: css.backgroundColor,
        color: css.color,
        height: node.getBoundingClientRect().height,
      };
    });
    assert.deepEqual(styles, {
      background: "rgb(37, 99, 235)",
      color: "rgb(255, 255, 255)",
      height: 28,
    });
    assert.equal(
      await page.getByRole("button", { name: "Disabled example" }).isDisabled(),
      true,
    );
    assert.equal(
      await page
        .getByRole("button", { name: "Loading example" })
        .getAttribute("aria-busy"),
      "true",
    );
    const destructive = page.getByRole("button", { name: "Delete example" });
    assert.deepEqual(
      await destructive.evaluate((node) => ({
        background: getComputedStyle(node).backgroundColor,
        color: getComputedStyle(node).color,
      })),
      { background: "rgb(220, 38, 38)", color: "rgb(255, 255, 255)" },
    );
    await destructive.hover();
    await page.waitForFunction(() =>
      Array.from(document.querySelectorAll("button")).some(
        (node) =>
          node.textContent === "Delete example" &&
          getComputedStyle(node).backgroundColor === "rgb(185, 28, 28)",
      ),
    );
    await page.mouse.move(0, 0);
    await page.screenshot({
      path: path.join(
        output,
        `controls-${options.width}-${options.dark ? "dark" : "light"}.png`,
      ),
    });
    const tabs = page.getByRole("tab");
    await tabs.first().focus();
    await page.keyboard.press("ArrowRight");
    assert.equal(
      await page
        .getByRole("tab", { name: "Files" })
        .getAttribute("aria-selected"),
      "true",
    );
    await page.keyboard.press("Home");
    assert.equal(
      await tabs.first().evaluate((node) => node === document.activeElement),
      true,
    );
    await primary.focus();
    assert.notEqual(
      await primary.evaluate((node) => getComputedStyle(node).boxShadow),
      "none",
    );
    await page.screenshot({
      path: path.join(
        output,
        `focus-${options.width}-${options.dark ? "dark" : "light"}.png`,
      ),
    });
    const blocked = page.getByRole("button", { name: "Blocked example" });
    await blocked.focus();
    await page.getByRole("tooltip").waitFor();
    await page.keyboard.press("Enter");
    await page.keyboard.press("Space");
    assert.equal(
      await page
        .locator("[data-action-count]")
        .getAttribute("data-action-count"),
      "0",
    );
    await page.keyboard.press("Escape");
    const input = page.getByRole("textbox", { name: "Search tiers" });
    await input.fill("sample");
    await page.keyboard.press("Escape");
    assert.equal(await input.inputValue(), "");
    await page.getByRole("button", { name: "Dismiss notice" }).click();
    assert.equal(
      await page.getByRole("status", { includeHidden: true }).textContent(),
      "",
    );
    const trigger = page.getByRole("button", { name: "Open draft drawer" });
    await trigger.click();
    const dialog = page.getByRole("dialog", { name: "Draft options" });
    await dialog.waitFor();
    assert.equal(
      await dialog.evaluate((node) => getComputedStyle(node).zIndex),
      "80",
    );
    const drawerSearch = page.getByRole("textbox", {
      name: "Search in drawer",
    });
    await drawerSearch.focus();
    await page.keyboard.press("Escape");
    assert.equal(await drawerSearch.inputValue(), "");
    assert.equal(await dialog.isVisible(), true);
    await page.keyboard.press("Escape");
    await dialog.waitFor({ state: "hidden" });
    await page.waitForFunction(
      () => document.activeElement?.textContent === "Open draft drawer",
    );
    await trigger.click();
    await dialog.waitFor();
    assert.equal(
      await page
        .getByRole("textbox", { name: "Read-only search" })
        .inputValue(),
      "protected value",
    );
    assert.equal(
      await page.getByRole("textbox", { name: "Disabled search" }).isDisabled(),
      true,
    );
    assert.equal(
      await page
        .getByRole("button", { name: "Clear read-only search" })
        .count(),
      0,
    );
    assert.equal(
      await page.getByRole("button", { name: "Clear disabled search" }).count(),
      0,
    );
    const close = page.getByRole("button", { name: "Close draft drawer" });
    // A visible drawer can precede Radix's passive layer registration on reopen.
    // Wait for real click actionability without clicking, then check its center.
    await close.click({ trial: true });
    assert.equal(
      await close.evaluate((node) => {
        const rect = node.getBoundingClientRect();
        return node.contains(
          document.elementFromPoint(
            rect.x + rect.width / 2,
            rect.y + rect.height / 2,
          ),
        );
      }),
      true,
    );
    await page
      .getByRole("textbox", { name: "Draft value" })
      .fill("unsaved synthetic draft");
    await page.getByRole("button", { name: "Drawer explanation" }).click();
    const tip = page.getByRole("tooltip");
    await tip.waitFor();
    assert.equal(
      await tip.evaluate((node) => getComputedStyle(node).zIndex),
      "120",
    );
    await page.screenshot({
      path: path.join(
        output,
        `drawer-${options.width}-${options.dark ? "dark" : "light"}.png`,
      ),
    });
    await page.keyboard.press("Escape");
    assert.equal(await dialog.isVisible(), true);
    // A pointer down on the overlay must not discard the draft.
    await page.mouse.click(10, 100);
    assert.equal(await dialog.isVisible(), true);
    await page.getByRole("button", { name: "Visit another page" }).click();
    await dialog.waitFor({ state: "hidden" });
    assert.notEqual(
      await page.evaluate(() => document.body.style.pointerEvents),
      "none",
    );
    await page.getByRole("button", { name: "Return to draft page" }).click();
    await dialog.waitFor();
    assert.equal(
      await page.getByRole("textbox", { name: "Draft value" }).inputValue(),
      "unsaved synthetic draft",
    );
    await page.getByRole("button", { name: "Cancel drawer" }).click();
    await dialog.waitFor({ state: "hidden" });
    await page.waitForFunction(
      () => document.activeElement?.textContent === "Open draft drawer",
    );
    await trigger.click();
    await page.keyboard.press("Escape");
    await dialog.waitFor({ state: "hidden" });
    await page.waitForFunction(
      () => document.activeElement?.textContent === "Open draft drawer",
    );
    assert.deepEqual(errors, []);
    results.push({ ...options, styles, status: "passed" });
    await context.close();
  }
  await writeFile(
    path.join(output, "ui-foundation-results.json"),
    JSON.stringify(results, null, 2),
  );
  console.log(JSON.stringify(results, null, 2));
} finally {
  await browser.close();
}
