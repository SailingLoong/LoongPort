import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { mkdir, readFile, writeFile } from "node:fs/promises";
import path from "node:path";

const origin = "http://127.0.0.1:4314";
const approvedImageSha256 =
  "05d8b77c3a164e69eeb03ab7f8ea03691d90bda80988a1143c81b2d6f5b41544";
const source = process.env.APPLICATIONS_C2_SOURCE_SHA;
assert.match(
  source ?? "",
  /^[0-9a-f]{40}$/,
  "Exact current-checkout SHA is required",
);
const output =
  process.env.APPLICATIONS_C2_OUTPUT ?? "artifacts/applications-c2";
await mkdir(output, { recursive: true });
const provenance = {
  source,
  approvedImageSha256,
  fixture:
    "Current-checkout real App/CSS with official Tauri mockIPC/mockWindows; all native data and writes are synthetic browser memory. No real credentials or accounts.",
  proves: "Browser rendering and interaction only",
  doesNotProve: [
    "native/WebView behavior",
    "real account availability",
    "group member eligibility or skip reasons",
    "actual forwarding/model requests",
    "pixel-equivalence or visual acceptance against the approved image",
  ],
  hashes: {},
};
for (const file of [
  "src/App.tsx",
  "src/index.css",
  "src/components/AppSwitcher.tsx",
  "src/components/applications/ApplicationRouteWorkspace.tsx",
  "src/components/applications/SharedRouteReview.tsx",
  "src/components/applications/useSharedRouteWorkspace.ts",
  "tests/browser/applications-c2-fixture.ts",
  "tests/browser/applications-c2-app.tsx",
  "tests/browser/applications-c2.tsx",
  "tests/browser/run-applications-c2.mjs",
]) {
  provenance.hashes[file] = createHash("sha256")
    .update(await readFile(new URL(`../../${file}`, import.meta.url)))
    .digest("hex");
}
await writeFile(
  path.join(output, "provenance.json"),
  JSON.stringify(provenance, null, 2),
);
const { chromium } = await import(
  process.env.PLAYWRIGHT_MODULE ?? "playwright"
);
const browser = await chromium.launch({
  headless: true,
  chromiumSandbox: true,
  ...(process.env.BROWSER_EXECUTABLE
    ? { executablePath: process.env.BROWSER_EXECUTABLE }
    : {}),
});
const results = [];
const readFixture = (page) =>
  page.evaluate(() => ({
    calls: window.applicationsC2.calls,
    rejectedCommands: window.applicationsC2.rejectedCommands,
  }));
const settle = async (page) => {
  await page.waitForFunction(() => window.applicationsC2?.settled());
  await page.evaluate(() => document.fonts.ready);
  // Playwright's ordinary visible state permits opacity:0. Inspect the actual
  // workspace/modal ancestor chain so captures wait for App's entry to finish,
  // without disabling production animations or adding a fixed-time sleep.
  await page.waitForFunction(() => {
    const workspace = document.querySelector(
      'section[aria-label="应用路由工作台"]',
    );
    if (!workspace) return false;
    const dialog = document.querySelector('[role="dialog"]');
    return [workspace, ...(dialog ? [dialog] : [])].every((entry) => {
      const bounds = entry.getBoundingClientRect();
      if (bounds.width <= 0 || bounds.height <= 0) return false;
      for (let node = entry; node; node = node.parentElement) {
        const style = getComputedStyle(node);
        if (
          style.display === "none" ||
          style.visibility === "hidden" ||
          style.visibility === "collapse" ||
          Number(style.opacity) !== 1
        )
          return false;
      }
      return true;
    });
  });
};
const commands = (evidence, command) =>
  evidence.calls.filter((call) => call.command === command);
async function checkGeometry(page, viewport, dialog = false) {
  const main = page.locator("main");
  const geometry = await page.evaluate(() => {
    const main = document.querySelector("main"),
      header = document.querySelector("header");
    return {
      viewport: innerWidth,
      pageScrollWidth: document.documentElement.scrollWidth,
      main: {
        x: main.getBoundingClientRect().x,
        width: main.clientWidth,
        scrollWidth: main.scrollWidth,
        scrollHeight: main.scrollHeight,
        height: main.clientHeight,
      },
      header: header.getBoundingClientRect().toJSON(),
    };
  });
  assert.ok(
    geometry.pageScrollWidth <= viewport.width + 1,
    "Document must not overflow horizontally",
  );
  assert.ok(
    geometry.main.scrollWidth <= geometry.main.width + 1,
    "Actual App main content must not overflow horizontally",
  );
  const scope = dialog
    ? page.getByRole("dialog")
    : page.getByRole("region", { name: "应用路由工作台", exact: true });
  const evidence = [];
  if (viewport.width === 1440 && !dialog) {
    // Approved compact desktop treatment for these short synthetic identities;
    // this is not a height limit for arbitrary long real-world names.
    for (const member of await page
      .getByTestId("shared-member-list")
      .locator("li")
      .all()) {
      const box = await member.boundingBox();
      assert.ok(
        box && box.height <= 60,
        `Compact desktop member exceeds 60 px: ${JSON.stringify(box)}`,
      );
    }
  }
  // Inspect actual content, not just the outside bounding box. Scrollable vertical
  // content is allowed; clipped text/controls or unreachable targets are not.
  for (const item of await scope
    .locator("h1,h2,h3,p,select,button,summary,li,input,label")
    .all()) {
    if (!(await item.isVisible())) continue;
    await item.scrollIntoViewIfNeeded();
    const state = await item.evaluate((element) => {
      const box = element.getBoundingClientRect();
      const style = getComputedStyle(element);
      const clippedText = [];
      const walker = document.createTreeWalker(element, NodeFilter.SHOW_TEXT);
      while (walker.nextNode()) {
        const node = walker.currentNode;
        const parent = node.parentElement;
        if (
          !node.textContent.trim() ||
          !parent ||
          parent.closest(".sr-only,[hidden]")
        )
          continue;
        const textStyle = getComputedStyle(parent);
        if (textStyle.display === "none" || textStyle.visibility === "hidden")
          continue;
        const range = document.createRange();
        range.selectNodeContents(node);
        for (const textRect of range.getClientRects()) {
          if (!textRect.width || !textRect.height) continue;
          for (
            let ancestor = parent;
            ancestor;
            ancestor = ancestor.parentElement
          ) {
            const ancestorStyle = getComputedStyle(ancestor);
            const bounds = ancestor.getBoundingClientRect();
            const left = bounds.left + ancestor.clientLeft;
            const top = bounds.top + ancestor.clientTop;
            const clipsX = ["hidden", "clip", "auto", "scroll"].includes(
              ancestorStyle.overflowX,
            );
            const clipsY = ["hidden", "clip", "auto", "scroll"].includes(
              ancestorStyle.overflowY,
            );
            if (
              (clipsX &&
                (textRect.left < left - 1 ||
                  textRect.right > left + ancestor.clientWidth + 1)) ||
              (clipsY &&
                (textRect.top < top - 1 ||
                  textRect.bottom > top + ancestor.clientHeight + 1))
            ) {
              clippedText.push({
                text: node.textContent.trim(),
                ancestor: ancestor.tagName,
                className: ancestor.className,
                overflowX: ancestorStyle.overflowX,
                overflowY: ancestorStyle.overflowY,
                textRect: textRect.toJSON(),
                clipRect: {
                  left,
                  top,
                  width: ancestor.clientWidth,
                  height: ancestor.clientHeight,
                },
              });
            }
          }
        }
      }
      return {
        tag: element.tagName,
        label: element.getAttribute("aria-label") || element.textContent.trim(),
        x: box.x,
        right: box.right,
        top: box.top,
        bottom: box.bottom,
        clientWidth: element.clientWidth,
        scrollWidth: element.scrollWidth,
        clientHeight: element.clientHeight,
        scrollHeight: element.scrollHeight,
        verticallyScrollable: ["auto", "scroll"].includes(style.overflowY),
        clippedText,
        enabled:
          !element.matches(":disabled") &&
          element.getAttribute("aria-disabled") !== "true",
        interactive: element.matches("button,select,summary,input"),
        hit: element.contains(
          document.elementFromPoint(
            box.x + box.width / 2,
            box.y + box.height / 2,
          ),
        ),
      };
    });
    assert.ok(
      state.x >= (dialog ? 0 : geometry.main.x) - 1 &&
        state.right <= viewport.width + 1,
      `Horizontal content escape: ${JSON.stringify(state)}`,
    );
    assert.ok(
      state.scrollWidth <= state.clientWidth + 1,
      `Clipped content: ${JSON.stringify(state)}`,
    );
    if (!state.verticallyScrollable) {
      assert.ok(
        state.scrollHeight <= state.clientHeight + 1,
        `Vertically clipped content: ${JSON.stringify(state)}`,
      );
    }
    assert.deepEqual(
      state.clippedText,
      [],
      `Text clipped by an overflow ancestor: ${state.label}`,
    );
    if (state.interactive && state.enabled) {
      assert.ok(
        state.top >= (dialog ? 0 : geometry.header.bottom) - 1 &&
          state.bottom <= viewport.height + 1,
        `Unreachable vertical control: ${JSON.stringify(state)}`,
      );
      assert.ok(
        state.hit,
        `Control covered by another element: ${JSON.stringify(state)}`,
      );
    }
    evidence.push(state);
  }
  if (!dialog) await main.evaluate((element) => element.scrollTo(0, 0));
  return { ...geometry, content: evidence };
}
async function capture(
  page,
  state,
  scenario,
  viewport,
  errors,
  requests,
  dialog = false,
) {
  await settle(page);
  const geometry = await checkGeometry(page, viewport, dialog);
  const fixture = await readFixture(page);
  assert.deepEqual(
    fixture.rejectedCommands,
    [],
    "No unimplemented native call may be hidden",
  );
  assert.deepEqual(errors, [], "No renderer error may be ignored");
  assert.deepEqual(
    requests,
    [],
    "No request outside the loopback renderer is allowed",
  );
  assert.match(
    await page.getByTestId("actual-request-fact").textContent(),
    /未验证/,
  );
  const screenshots = [];
  const filename = `${scenario}-${viewport.width}-${state}.png`;
  await page.screenshot({
    path: path.join(output, filename),
    fullPage: false,
    animations: "disabled",
  });
  screenshots.push(filename);
  if (!dialog && geometry.main.scrollHeight > geometry.main.height + 1) {
    await page
      .locator("main")
      .evaluate((element) => element.scrollTo(0, element.scrollHeight));
    const footer = `${scenario}-${viewport.width}-${state}-footer.png`;
    await page.screenshot({
      path: path.join(output, footer),
      fullPage: false,
      animations: "disabled",
    });
    screenshots.push(footer);
    await page.locator("main").evaluate((element) => element.scrollTo(0, 0));
  }
  const hashes = {};
  for (const name of screenshots)
    hashes[name] = createHash("sha256")
      .update(await readFile(path.join(output, name)))
      .digest("hex");
  results.push({
    source,
    approvedImageSha256,
    scenario,
    state,
    viewport,
    screenshots,
    hashes,
    geometry,
    fixture,
    errors: [...errors],
    unexpectedRequests: [...requests],
  });
  await writeFile(
    path.join(output, "results.json"),
    JSON.stringify(results, null, 2),
  );
}
try {
  for (const viewport of [
    { width: 1440, height: 1040 },
    { width: 760, height: 1040 },
  ]) {
    for (const scenario of ["normal", "unknown", "pending"]) {
      const context = await browser.newContext({
        viewport,
        locale: "zh-CN",
        colorScheme: "light",
        reducedMotion: "reduce",
        serviceWorkers: "block",
      });
      const errors = [],
        requests = [];
      await context.route("**/*", (route) => {
        const request = route.request();
        if (
          new URL(request.url()).origin === origin &&
          request.method() === "GET" &&
          [
            "document",
            "script",
            "stylesheet",
            "image",
            "font",
            "other",
          ].includes(request.resourceType())
        )
          return route.continue();
        requests.push({ url: request.url(), method: request.method() });
        return route.abort();
      });
      await context.routeWebSocket("**/*", (socket) => {
        requests.push({ url: socket.url(), method: "WEBSOCKET" });
        socket.close();
      });
      const page = await context.newPage();
      page.on("pageerror", (error) => errors.push(error.message));
      page.on("console", (message) => {
        if (message.type() === "error") errors.push(message.text());
      });
      try {
        await page.goto(
          `${origin}/tests/browser/applications-c2.html?scenario=${scenario}`,
        );
        await page
          .getByRole("heading", { name: "调整 Codex 的路由", exact: true })
          .waitFor();
        await page.waitForFunction(
          () =>
            document.querySelector("#route-source-codex")?.value ===
            "synthetic-minimax",
        );
        await settle(page);
        const codex = page.getByRole("button", { name: "Codex", exact: true });
        assert.equal(await codex.getAttribute("aria-pressed"), "true");
        const tabBox = await codex.boundingBox();
        assert.equal(tabBox.width, 48);
        assert.equal(tabBox.height, 44);
        const add = page.getByRole("button", {
          name: "添加应用到主页面",
          exact: true,
        });
        const addBox = await add.boundingBox();
        assert.equal(addBox.width, 40);
        assert.equal(addBox.height, 44);
        await capture(
          page,
          "saved-manual",
          scenario,
          viewport,
          errors,
          requests,
        );
        if (scenario === "pending") {
          assert.equal(
            await page
              .getByRole("radio", { name: "省心 · 按顺序" })
              .isDisabled(),
            true,
          );
          await page
            .getByRole("button", { name: "恢复原配置操作", exact: true })
            .click();
          await page
            .getByText("原操作未保存变更。草稿已保留，可检查后重新预览。", {
              exact: true,
            })
            .waitFor();
          const evidence = await readFixture(page);
          assert.equal(
            commands(evidence, "recover_application_route_target").length,
            1,
          );
          assert.equal(
            commands(evidence, "apply_application_route_target").length,
            0,
          );
          assert.equal(
            commands(evidence, "recover_application_route_target")[0].payload
              .request.id,
            "synthetic-original-pending",
          );
          await capture(
            page,
            "original-operation-recovered",
            scenario,
            viewport,
            errors,
            requests,
          );
          continue;
        }
        if (scenario === "normal") {
          // Original x/+ change app visibility only; showing Gemini selects it.
          const hide = page.getByRole("button", {
            name: "从主页面隐藏: Gemini",
            exact: true,
          });
          await page
            .getByRole("button", { name: "Gemini", exact: true })
            .hover();
          const hideBox = await hide.boundingBox();
          assert.equal(hideBox.width, 20);
          assert.equal(hideBox.height, 20);
          await hide.click();
          await page
            .getByRole("button", { name: "Gemini", exact: true })
            .waitFor({ state: "detached" });
          await capture(
            page,
            "app-hidden",
            scenario,
            viewport,
            errors,
            requests,
          );
          await add.click();
          await page
            .getByRole("button", { name: "Gemini", exact: true })
            .click();
          await page.waitForFunction(
            () =>
              document
                .querySelector('button[aria-label="Gemini"]')
                ?.getAttribute("aria-pressed") === "true",
          );
          await codex.click();
          await page.waitForFunction(
            () =>
              document.querySelector("#route-source-codex")?.value ===
              "synthetic-minimax",
          );
          const visibility = commands(
            await readFixture(page),
            "set_app_visibility",
          );
          assert.deepEqual(
            visibility.map((call) => call.payload),
            [
              { app: "gemini", visible: false },
              { app: "gemini", visible: true },
            ],
          );
        }
        await page
          .getByRole("radio", { name: "省心 · 按顺序", exact: true })
          .check();
        await page
          .getByRole("combobox", { name: "共享组", exact: true })
          .selectOption("synthetic-daily");
        const members = page.getByTestId("shared-member-list").locator("li");
        assert.equal(await members.count(), 3);
        for (const [index, identity] of [
          "Qoder C",
          "MiniMax A",
          "WorkBuddy B",
        ].entries()) {
          assert.ok(
            (await members.nth(index).textContent()).includes(identity),
          );
          assert.equal(
            await members
              .nth(index)
              .getByText("待核验", { exact: true })
              .count(),
            1,
          );
        }
        let evidence = await readFixture(page);
        assert.equal(
          commands(evidence, "prepare_application_route_target").length,
          0,
        );
        assert.equal(
          commands(evidence, "apply_application_route_target").length,
          0,
        );
        await capture(
          page,
          "ordered-local-draft",
          scenario,
          viewport,
          errors,
          requests,
        );
        await page
          .getByRole("button", { name: "预览变更", exact: true })
          .click();
        const dialog = page.getByRole("dialog", {
          name: "确认这次路由变更",
          exact: true,
        });
        await dialog.waitFor();
        evidence = await readFixture(page);
        assert.equal(
          commands(evidence, "prepare_application_route_target").length,
          1,
        );
        assert.equal(
          commands(evidence, "apply_application_route_target").length,
          0,
        );
        await capture(
          page,
          "native-prepared-explicit-review",
          scenario,
          viewport,
          errors,
          requests,
          true,
        );
        await dialog
          .getByRole("button", { name: "确认保存此应用选择", exact: true })
          .click();
        if (scenario === "unknown") {
          await page
            .getByText("原操作结果尚未确认。请检查原记录，不要重复提交。", {
              exact: true,
            })
            .waitFor();
          assert.equal(
            await page
              .getByText("此应用选择已保存；真实请求仍未验证。", {
                exact: true,
              })
              .count(),
            0,
          );
          assert.equal(
            await page
              .getByRole("button", { name: "恢复原配置操作", exact: true })
              .count(),
            0,
          );
          await capture(
            page,
            "unknown-not-saved",
            scenario,
            viewport,
            errors,
            requests,
          );
          await page
            .getByRole("button", { name: "检查原配置操作", exact: true })
            .click();
          await settle(page);
          evidence = await readFixture(page);
          assert.equal(
            commands(evidence, "get_application_route_target_result").length,
            1,
          );
          assert.equal(
            commands(evidence, "recover_application_route_target").length,
            0,
          );
          await capture(
            page,
            "unknown-inspected-no-replay",
            scenario,
            viewport,
            errors,
            requests,
          );
        } else {
          await page
            .getByText("此应用选择已保存；真实请求仍未验证。", { exact: true })
            .waitFor();
          await capture(
            page,
            "saved-configuration-only",
            scenario,
            viewport,
            errors,
            requests,
          );
        }
        evidence = await readFixture(page);
        assert.equal(
          commands(evidence, "apply_application_route_target").length,
          1,
        );
      } catch (error) {
        const prefix = `failure-${scenario}-${viewport.width}`;
        await page
          .screenshot({ path: path.join(output, `${prefix}.png`) })
          .catch(() => {});
        await writeFile(
          path.join(output, `${prefix}.json`),
          JSON.stringify(
            {
              ...provenance,
              scenario,
              viewport,
              error: String(error),
              errors,
              unexpectedRequests: requests,
              fixture: await readFixture(page).catch(() => null),
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
  await writeFile(
    path.join(output, "summary.json"),
    JSON.stringify(
      {
        source,
        approvedImageSha256,
        status: "passed",
        states: results.length,
        screenshots: results.reduce(
          (n, result) => n + result.screenshots.length,
          0,
        ),
        boundary: provenance.proves,
        acceptance:
          "Human screenshot comparison and native/live acceptance remain separate.",
      },
      null,
      2,
    ),
  );
  console.log(
    JSON.stringify({
      source,
      status: "passed",
      states: results.length,
      boundary: provenance.proves,
    }),
  );
} finally {
  await browser.close();
}
