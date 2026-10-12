import { readFileSync } from "node:fs";
import { createRequire } from "node:module";
import postcss from "postcss";
import tailwind from "tailwindcss";

const css = readFileSync("src/index.css", "utf8");
const config = createRequire(import.meta.url)("../../tailwind.config.cjs");

function variables(selector: string) {
  const values = new Map<string, string>();
  postcss.parse(css).walkRules(selector, (rule) => {
    rule.walkDecls((decl) => {
      values.set(decl.prop, decl.value);
    });
  });
  return values;
}

it.each([":root", ".dark"])(
  "keeps %s action fill and text centrally branded blue and white",
  (selector) => {
    const values = variables(selector);
    expect(values.get("--action-bg")).toBe("#2563eb");
    expect(values.get("--action-fg")).toBe("#ffffff");
    expect(values.get("--primary")).toBe("221.2 83.2% 53.3%");
    expect(values.get("--primary-foreground")).toBe("0 0% 100%");
    expect(values.get("--action-text")).toBeTruthy();
    expect(values.get("--ring")).not.toBe("24.6 95% 53.1%");
  },
);

it("generates the semantic control utilities and preserves the 1180px page container", async () => {
  const { css: output } = await postcss([
    tailwind({
      ...config,
      content: [
        {
          raw: "bg-action text-action-fg bg-surface text-fg-1 bg-warning-soft rounded-control rounded-panel rounded-s-dialog shadow-v7-lg text-body text-caption opacity-45 scroll-stable",
        },
      ],
    }),
  ]).process(css, { from: "src/index.css" });
  expect(output).toContain("background-color: var(--action-bg)");
  expect(output).toContain("color: var(--action-fg)");
  expect(output).toContain("background-color: var(--bg-card)");
  expect(output).toContain("background-color: var(--warning-soft)");
  expect(output).toContain("border-radius: 6px");
  expect(output).toContain("border-radius: 10px");
  expect(output).toContain("border-start-start-radius: 14px");
  expect(output).toContain("font-size: 13px");
  expect(output).toContain("opacity: 0.45");
  expect(output).toContain("scrollbar-gutter: stable;");
  expect(css).toContain("max-width: 1180px");
  expect(config.plugins).toEqual([]);
});

it("uses the shared semantic ring for the global keyboard focus outline", () => {
  const declarations = variables("*:focus-visible");
  expect(declarations.get("outline-color")).toBe("hsl(var(--ring))");
});

function contrastWithWhite(hex: string) {
  const channels = hex
    .slice(1)
    .match(/../g)!
    .map((value) => parseInt(value, 16) / 255);
  const linear = channels.map((value) =>
    value <= 0.04045 ? value / 12.92 : ((value + 0.055) / 1.055) ** 2.4,
  );
  return (
    1.05 / (0.2126 * linear[0] + 0.7152 * linear[1] + 0.0722 * linear[2] + 0.05)
  );
}

it.each([":root", ".dark"])(
  "keeps destructive white labels at 4.5:1 in %s, including hover",
  async (selector) => {
    const { buttonVariants } = await import("@/components/ui/button");
    const classes = buttonVariants({ variant: "destructive" }).split(" ");
    const values = variables(selector);
    function color(prefix: string) {
      const utility = classes.find((value) => value.startsWith(prefix));
      if (!utility) return undefined;
      const [role, variant = "DEFAULT"] = utility
        .slice(prefix.length)
        .split("-");
      const token = config.theme.extend.colors[role][variant] as string;
      return values.get(token.slice(4, -1))!;
    }
    const background = color("bg-")!;
    let hover = color("hover:bg-") ?? background;
    if (classes.includes("hover:brightness-110")) {
      hover =
        "#" +
        hover
          .slice(1)
          .match(/../g)!
          .map((value) =>
            Math.min(255, Math.round(parseInt(value, 16) * 1.1))
              .toString(16)
              .padStart(2, "0"),
          )
          .join("");
    }
    expect(classes).toContain("text-danger-on");
    expect(values.get("--danger-on")).toBe("#ffffff");
    expect(contrastWithWhite(background)).toBeGreaterThanOrEqual(4.5);
    expect(contrastWithWhite(hover)).toBeGreaterThanOrEqual(4.5);
  },
);
