// Start: node node_modules/vite/bin/vite.js --config tests/browser/vite.config.mjs
// Run: PLAYWRIGHT_MODULE=/path/to/playwright/index.mjs node tests/browser/run-zcode-layout.mjs
// Uses an existing browser test runtime; this is not native GUI acceptance.
import { verifyProviderLayout } from "./zcode-provider-layout.mjs";

const { chromium, webkit } = await import(
  process.env.PLAYWRIGHT_MODULE ?? "playwright"
);
const engine = process.env.ENGINE ?? "chromium";
if (!["chromium", "webkit"].includes(engine))
  throw new Error("Unknown browser engine");
const browser = await { chromium, webkit }[engine].launch({
  headless: true,
  ...(process.env.BROWSER_EXECUTABLE
    ? { executablePath: process.env.BROWSER_EXECUTABLE }
    : {}),
});
try {
  const page = await browser.newPage();
  for (const options of [
    { height: 650, models: 80 },
    { height: 800, models: 80, resolution: "Use external values" },
    { height: 800, models: 1 },
  ]) {
    console.log(
      JSON.stringify({
        engine,
        ...(await verifyProviderLayout(
          page,
          "http://127.0.0.1:4314/tests/browser/zcode-provider.html",
          { ...options, tabKey: engine === "webkit" ? "Alt+Tab" : "Tab" },
        )),
      }),
    );
  }
} finally {
  await browser.close();
}
