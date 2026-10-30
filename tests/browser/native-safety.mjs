import assert from "node:assert/strict";
import path from "node:path";

/** No native write handler exists. Any accidental submit fails this check.
 * Vendor assertions cover presentation callbacks, not the consumer transaction. */
export async function checkNativeSafety({
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
    unexpectedRequests = [];
  await context.route("**/*", (route) => {
    const url = new URL(route.request().url());
    if (url.origin === "http://127.0.0.1:4314") return route.continue();
    unexpectedRequests.push(url.origin);
    return route.abort();
  });
  const page = await context.newPage();
  page.on("pageerror", (error) => errors.push(String(error)));
  page.on("console", (message) => {
    if (message.type() === "error") errors.push(message.text());
  });
  try {
    await page.goto(
      `http://127.0.0.1:4314/tests/browser/unified-accounts.html?scenario=native-safety&lang=${variant.language}&dark=${variant.dark}`,
    );
    assert.equal(
      await page.evaluate(() => window.unifiedLayout.checkout),
      source,
    );
    const preview = page.getByRole("region", {
      name: t("workbuddy.nativePreview.title"),
      exact: true,
    });
    const capture = page.getByRole("group", {
      name: t("workbuddy.nativeCapture.title"),
      exact: true,
    });
    for (let repeat = 0; repeat < 2; repeat += 1) {
      await preview
        .getByRole("button", {
          name: t("workbuddy.nativeCapture.open"),
          exact: true,
        })
        .click();
      await capture.getByRole("textbox").fill(`Synthetic label ${repeat}`);
      assert.equal(
        await capture
          .getByRole("button", {
            name: t("workbuddy.nativeCapture.confirm"),
            exact: true,
          })
          .isEnabled(),
        true,
      );
      await capture
        .getByRole("button", {
          name: t("workbuddy.nativeCapture.cancel"),
          exact: true,
        })
        .click();
      await capture.waitFor({ state: "hidden" });
    }
    await preview
      .getByRole("button", {
        name: t("workbuddy.nativeCapture.open"),
        exact: true,
      })
      .click();
    await capture.getByRole("textbox").fill("Synthetic interrupted capture");
    await page
      .getByRole("button", { name: "Leave safety view", exact: true })
      .click();
    await preview.waitFor({ state: "hidden" });
    await page
      .getByRole("button", { name: "Return to safety view", exact: true })
      .click();
    await preview.waitFor({ state: "visible" });
    await capture
      .getByRole("button", {
        name: t("workbuddy.nativeCapture.cancel"),
        exact: true,
      })
      .click();
    await capture.waitFor({ state: "hidden" });
    const recovery = page.getByRole("region", {
      name: t("workbuddy.nativeRecovery.title"),
      exact: true,
    });
    assert.equal(
      await recovery
        .getByRole("button", {
          name: t("workbuddy.nativeRecovery.open"),
          exact: true,
        })
        .isEnabled(),
      false,
    );
    await recovery
      .getByRole("button", {
        name: t("workbuddy.nativeRecovery.check"),
        exact: true,
      })
      .click();
    const recoverOpen = recovery.getByRole("button", {
      name: t("workbuddy.nativeRecovery.open"),
      exact: true,
    });
    await recoverOpen.waitFor();
    // Waiting for enabled via retrying locator action also covers the asynchronous read.
    for (const dismissal of ["cancel", "escape"]) {
      await recoverOpen.click();
      const dialog = page.getByRole("dialog", {
        name: t("workbuddy.nativeRecovery.confirmTitle"),
        exact: true,
      });
      await dialog.waitFor();
      assert.equal(
        await dialog
          .getByRole("button", {
            name: t("workbuddy.nativeRecovery.confirm"),
            exact: true,
          })
          .isEnabled(),
        true,
      );
      if (dismissal === "cancel")
        await dialog
          .getByRole("button", { name: t("common.cancel"), exact: true })
          .click();
      else await page.keyboard.press("Escape");
      await dialog.waitFor({ state: "hidden" });
    }
    const openVendor = page.getByRole("button", {
      name: "Review synthetic vendor",
      exact: true,
    });
    for (const dismissal of ["cancel", "escape"]) {
      await openVendor.click();
      const dialog = page.getByRole("dialog", {
        name: t("applications.vendorRecovery.title"),
        exact: true,
      });
      await dialog.waitFor();
      if (dismissal === "cancel")
        await dialog
          .getByRole("button", { name: t("common.cancel"), exact: true })
          .click();
      else await page.keyboard.press("Escape");
      await dialog.waitFor({ state: "hidden" });
    }
    assert.deepEqual(
      await page.evaluate(() => window.unifiedLayout.safetyEvidence),
      { vendorConfirms: 0, vendorCancels: 2 },
    );
    await page
      .getByRole("checkbox", { name: "Disable synthetic vendor confirmation" })
      .check();
    await openVendor.click();
    let vendorDialog = page.getByRole("dialog", {
      name: t("applications.vendorRecovery.title"),
      exact: true,
    });
    assert.equal(
      await vendorDialog
        .getByRole("button", {
          name: t("applications.vendorRecovery.confirm"),
          exact: true,
        })
        .isEnabled(),
      false,
    );
    await page.keyboard.press("Escape");
    await vendorDialog.waitFor({ state: "hidden" });
    await page
      .getByRole("checkbox", { name: "Disable synthetic vendor confirmation" })
      .uncheck();
    for (let repeat = 0; repeat < 2; repeat += 1) {
      await openVendor.click();
      vendorDialog = page.getByRole("dialog", {
        name: t("applications.vendorRecovery.title"),
        exact: true,
      });
      await vendorDialog
        .getByRole("button", {
          name: t("applications.vendorRecovery.confirm"),
          exact: true,
        })
        .click();
      await vendorDialog.waitFor({ state: "hidden" });
      assert.equal(
        await page.evaluate(
          () => window.unifiedLayout.safetyEvidence.vendorConfirms,
        ),
        repeat + 1,
      );
    }
    const evidence = await page.evaluate(() => ({
      requested: window.unifiedLayout.requestedCommands,
      rejected: window.unifiedLayout.rejectedCommands,
      callbacks: window.unifiedLayout.safetyEvidence,
    }));
    assert.deepEqual(evidence.rejected, []);
    assert.ok(evidence.requested.includes("inspect_workbuddy_native_recovery"));
    assert.deepEqual(
      evidence.requested.filter(
        (name) =>
          ![
            "list_workbuddy_native_captures",
            "inspect_workbuddy_native_recovery",
          ].includes(name),
      ),
      [],
    );
    assert.deepEqual(errors, []);
    assert.deepEqual(unexpectedRequests, []);
    const filename = `native-safety-${variant.width}-${variant.language}.png`;
    await page.screenshot({
      path: path.join(output, filename),
      fullPage: true,
    });
    return {
      source,
      scenario: "native-safety",
      ...variant,
      filename,
      evidence,
      errors,
      unexpectedRequests,
      boundary:
        "Cancellation/read-only UI and vendor presentation callbacks only; no native write or vendor consumer transaction tested",
    };
  } catch (error) {
    await page
      .screenshot({
        path: path.join(
          output,
          `failure-native-safety-${variant.language}.png`,
        ),
      })
      .catch(() => {});
    throw error;
  } finally {
    await context.close();
  }
}
