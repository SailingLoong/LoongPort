import { pathToFileURL } from "node:url";
import { mkdir, writeFile } from "node:fs/promises";
import path from "node:path";
import { createHash } from "node:crypto";

const { chromium } = await import(
  pathToFileURL(process.env.PLAYWRIGHT_MODULE).href
);
const output = process.env.DOCS_SCREENSHOTS ?? "docs-ui-artifact";
await mkdir(output, { recursive: true });
const browser = await chromium.launch({
  headless: true,
  executablePath: process.env.DOCS_UI_CHROME,
  chromiumSandbox: true,
});
const context = await browser.newContext({
  viewport: { width: 1440, height: 1000 },
  locale: "zh-CN",
  colorScheme: "light",
  deviceScaleFactor: 1,
});
await context.route("**/*", (route) => {
  const url = new URL(route.request().url());
  return url.hostname === "127.0.0.1" && url.port === "4198"
    ? route.continue()
    : route.abort();
});
const page = await context.newPage();
const messages = [];
const errors = [];
page.on("console", (message) =>
  messages.push({ type: message.type(), text: message.text() }),
);
page.on("pageerror", (error) => errors.push(error.message));
const captures = [];
async function capture(name) {
  await page.evaluate(() => document.fonts.ready);
  const bytes = await page.screenshot({
    path: path.join(output, `${name}.png`),
    fullPage: false,
    animations: "disabled",
  });
  captures.push({
    name,
    sha256: createHash("sha256").update(bytes).digest("hex"),
    source: process.env.LOONGPORT_SOURCE_SHA,
    version: "6.26.2",
    data: "synthetic",
    runtime: "production React components with mock Tauri IPC",
  });
}
try {
  await page.goto("http://127.0.0.1:4198/", { waitUntil: "load" });
  await page.getByRole("heading", { name: /可用档位/ }).waitFor();
  await page
    .getByText("演示服务 · demo@example.com", { exact: true })
    .waitFor();
  await capture("01-application-workspace");
  await page.getByRole("button", { name: "扩展资源", exact: true }).click();
  await page
    .getByText("管理应用使用的工具、指令和工作区。", { exact: true })
    .waitFor();
  await capture("02-resources");
  await page.getByRole("button", { name: "设置", exact: true }).click();
  await page.getByRole("tab", { name: "连接设置", exact: true }).waitFor();
  await capture("03-settings-general");
  await page.getByRole("tab", { name: "连接设置", exact: true }).click();
  await capture("04-connection-settings");
  await page.getByRole("tab", { name: "高级", exact: true }).click();
  await capture("05-settings-advanced");
  await page.getByRole("button", { name: "应用", exact: true }).click();
  await page
    .getByRole("button", { name: "添加服务", exact: true })
    .first()
    .click();
  await page.getByRole("tab", { name: "手动添加", exact: true }).waitFor();
  await page.getByPlaceholder("bestapi.store").fill("https://example.com");
  await capture("06-add-service");
  await page.getByRole("tab", { name: "手动添加", exact: true }).click();
  await capture("07-manual-provider");
  if (errors.length) throw new Error(`Renderer errors: ${errors.join("; ")}`);
} catch (error) {
  await capture("failure-state");
  throw error;
} finally {
  await writeFile(
    path.join(output, "capture-record.json"),
    JSON.stringify(
      {
        source: process.env.LOONGPORT_SOURCE_SHA,
        captures,
        errors,
        console: messages,
      },
      null,
      2,
    ),
  );
  await browser.close();
}
