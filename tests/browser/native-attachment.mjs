import assert from "node:assert/strict";
import path from "node:path";

/** Real component, mocked receipts only; never native or real-account acceptance. */
export async function checkNativeAttachment({
  browser,
  variant,
  t,
  output,
  source,
}) {
  const context = await browser.newContext({
    viewport: { width: variant.width, height: variant.height },
  });
  const errors = [],
    unexpected = [];
  await context.route("**/*", (route) => {
    if (new URL(route.request().url()).origin === "http://127.0.0.1:4314")
      return route.continue();
    unexpected.push(route.request().url());
    return route.abort();
  });
  const page = await context.newPage();
  page.on("pageerror", (error) => errors.push(String(error)));
  page.on("console", (message) => {
    if (message.type() === "error") errors.push(message.text());
  });
  const button = (key) =>
    page.getByRole("button", {
      name: t(`workbuddy.attachment.${key}`),
      exact: true,
    });
  const status = (key) =>
    page.getByText(t(`workbuddy.attachment.${key}`), { exact: true });
  try {
    await page.goto(
      `http://127.0.0.1:4314/tests/browser/unified-accounts.html?scenario=native-attachment&lang=${variant.language}&dark=${variant.dark}`,
    );
    assert.equal(
      await page.evaluate(() => window.unifiedLayout.checkout),
      source,
    );
    await button("prepare").click();
    await status("ready").waitFor();
    await page.getByText("Synthetic attachment A", { exact: true }).waitFor();
    await button("confirm").dblclick();
    await status("saved").waitFor();
    await page.waitForFunction(
      () =>
        document.querySelector('[data-testid="attachment-refreshed"]')
          .textContent === "1",
    );
    assert.equal(
      await page.evaluate(() => window.unifiedLayout.attachment.confirms),
      1,
    );
    await page.evaluate(() => {
      window.unifiedLayout.attachment.defer = true;
    });
    await page
      .getByRole("button", {
        name: "Select synthetic attachment B",
        exact: true,
      })
      .click();
    await button("prepare").click();
    await status("ready").waitFor();
    await button("confirm").click();
    await page.evaluate(() => window.unifiedLayout.attachment.loseResponse());
    await status("unknown").waitFor();
    await button("inspect").click();
    await page
      .getByRole("button", {
        name: "Select synthetic attachment A",
        exact: true,
      })
      .click();
    await page.evaluate(() =>
      window.unifiedLayout.attachment.releaseLateSaved(),
    );
    await page.evaluate(
      () =>
        new Promise((resolve) =>
          requestAnimationFrame(() => requestAnimationFrame(resolve)),
        ),
    );
    assert.equal(await status("saved").count(), 0);
    assert.equal(
      await page.getByTestId("attachment-refreshed").textContent(),
      "1",
    );
    assert.equal(
      await page.evaluate(() => window.unifiedLayout.attachment.confirms),
      2,
    );
    assert.deepEqual(
      await page.evaluate(() => window.unifiedLayout.rejectedCommands),
      [],
    );
    const nativeWrites = await page.evaluate(() =>
      window.unifiedLayout.requestedCommands.filter((command) =>
        [
          "capture_workbuddy_native_account",
          "activate_workbuddy_native_account",
          "activate_workbuddy_native_with_restart",
          "recover_workbuddy_native_switch",
        ].includes(command),
      ),
    );
    assert.deepEqual(nativeWrites, []);
    assert.deepEqual(errors, []);
    assert.deepEqual(unexpected, []);
    const screenshot = `native-attachment-${variant.width}-${variant.language}.png`;
    await page.screenshot({
      path: path.join(output, screenshot),
      fullPage: true,
    });
    return {
      scenario: "native-attachment",
      ...variant,
      screenshot,
      source,
      coverage: "mock-receipt-component-only",
      status: "passed",
    };
  } finally {
    await context.close();
  }
}
