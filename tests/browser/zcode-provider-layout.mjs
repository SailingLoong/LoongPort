import assert from "node:assert/strict";

// Run with an existing Playwright installation; no native configuration is read.
// Pass a Playwright Page and the URL served by Vite with the repository as root.
export async function verifyProviderLayout(
  page,
  url,
  {
    height = 650,
    models = 80,
    resolution = "Keep my input",
    tabKey = "Tab",
  } = {},
) {
  await page.setViewportSize({ width: 1000, height });
  await page.goto(`${url}?models=${models}`);
  await page.getByRole("button", { name: "Edit", exact: true }).click();
  const dialog = page.getByRole("dialog");
  const key = dialog.getByLabel("API Key", { exact: true });
  await key.fill("synthetic-private-input");
  assert.equal(await key.getAttribute("type"), "password");
  await dialog.getByRole("button", { name: "Save", exact: true }).click();
  const reload = dialog.getByRole("button", {
    name: "Read latest configuration",
  });
  await reload.focus();
  await page.keyboard.press("Enter");
  await dialog.getByRole("button", { name: "Keep my input" }).waitFor();
  assert.ok(!(await dialog.innerText()).includes("synthetic-private-input"));
  const bounds = await dialog.boundingBox();
  assert.ok(
    bounds.y >= 0 && bounds.y + bounds.height <= height,
    "Dialog must fit the viewport",
  );
  const evidence = { height, models, bounds, focus: [] };
  // Real Tab navigation must scroll each decision into view inside the dialog.
  await dialog.getByLabel("Name", { exact: true }).focus();
  const required = new Set(["Keep my input", "Use external values", "Cancel"]);
  for (let index = 0; index < 20 && required.size; index++) {
    await page.keyboard.press(tabKey);
    const focused = await page.evaluate(() => {
      const element = document.activeElement;
      const box = element.getBoundingClientRect();
      const dialog = element.closest('[role="dialog"]');
      const outer = dialog.getBoundingClientRect();
      const x = box.left + box.width / 2,
        y = box.top + box.height / 2;
      return {
        text: element.textContent.trim(),
        top: box.top,
        bottom: box.bottom,
        inside: box.top >= outer.top && box.bottom <= outer.bottom,
        hit: element.contains(document.elementFromPoint(x, y)),
      };
    });
    if (required.has(focused.text)) {
      evidence.focus.push(focused);
      assert.ok(
        focused.top >= 0 &&
          focused.bottom <= height &&
          focused.inside &&
          focused.hit,
        `${focused.text} is focused but unreachable: ${JSON.stringify(focused)}`,
      );
      required.delete(focused.text);
    }
  }
  assert.equal(required.size, 0, "Tab must reach all enabled decisions");
  await dialog.getByRole("button", { name: resolution, exact: true }).focus();
  await page.keyboard.press("Enter");
  assert.equal(
    await key.inputValue(),
    resolution === "Keep my input" ? "synthetic-private-input" : "",
  );
  const save = dialog.getByRole("button", { name: "Save", exact: true });
  assert.equal(await save.isEnabled(), true);
  await save.focus();
  const saveBounds = await save.boundingBox();
  assert.ok(
    saveBounds.y >= 0 && saveBounds.y + saveBounds.height <= height,
    "Save must scroll into view",
  );
  await page.keyboard.press("Enter");
  await dialog.waitFor({ state: "hidden" });
  return evidence;
}
