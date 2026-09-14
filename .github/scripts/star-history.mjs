#!/usr/bin/env node
/**
 * Star History 快照生成器。
 *
 * GitHub 自 2026-06 起 stargazers 时间线（starred_at）仅仓库 admin/collaborator
 * 可读，star-history.com 的公共嵌入图因此对所有公开仓失效。本脚本用仓库
 * owner 身份的 token 拉取完整时间线，聚合成逐日累计星数并渲染成 SVG，
 * 与 history.json 一起写入目标目录。目标目录即数据分支 `star-history`
 * 的工作树（见 .github/workflows/star-history.yml），产物被 README 引用。
 *
 * 用法：node .github/scripts/star-history.mjs snapshot <target-dir>
 * 环境：GITHUB_REPOSITORY=owner/repo  GITHUB_TOKEN=<对该仓库有 admin 权限的 token>
 */

import { mkdir, writeFile } from "node:fs/promises";
import path from "node:path";
import { pathToFileURL } from "node:url";

const DAY_MS = 86_400_000;
const WIDTH = 960;
const HEIGHT = 480;
const MARGIN = { top: 72, right: 32, bottom: 44, left: 64 };

export async function fetchStargazers(repo, token) {
  const stars = [];
  for (let page = 1; ; page += 1) {
    const res = await fetch(
      `https://api.github.com/repos/${repo}/stargazers?per_page=100&page=${page}`,
      {
        headers: {
          Authorization: `Bearer ${token}`,
          Accept: "application/vnd.github.star+json",
          "X-GitHub-Api-Version": "2022-11-28",
          "User-Agent": "loongport-star-history",
        },
      },
    );
    if (!res.ok) {
      throw new Error(
        `GitHub API ${res.status} on stargazers page ${page}: ${await res.text()}`,
      );
    }
    const batch = await res.json();
    stars.push(...batch);
    if (batch.length < 100) return stars;
  }
}

export function cumulativeByDay(stars) {
  const gained = new Map();
  let seenStarredAt = 0;
  for (const star of stars) {
    if (!star.starred_at) continue;
    seenStarredAt += 1;
    const day = star.starred_at.slice(0, 10);
    gained.set(day, (gained.get(day) ?? 0) + 1);
  }
  if (stars.length > 0 && seenStarredAt === 0) {
    throw new Error(
      "stargazers 响应缺少 starred_at —— token 对该仓库没有星标时间线读取权限（需要 owner/collaborator 身份）",
    );
  }
  let total = 0;
  return [...gained.entries()]
    .sort(([a], [b]) => a.localeCompare(b))
    .map(([date, count]) => ({ date, stars: (total += count) }));
}

const dayIndex = (date) => Math.floor(Date.parse(`${date}T00:00:00Z`) / DAY_MS);
const r2 = (n) => Math.round(n * 100) / 100;
const fmt = (n) => n.toLocaleString("en-US");
const esc = (s) =>
  String(s).replace(/[&<>"']/g, (c) => `&#${c.codePointAt(0)};`);

// 1-2-5 阶梯取整，保证 y 轴刻度是整数且条数在 4~5 个左右。
function niceStep(raw) {
  const pow = 10 ** Math.floor(Math.log10(raw));
  return [1, 2, 5, 10].map((m) => m * pow).find((step) => step >= raw);
}

export function renderSvg(repo, points) {
  const firstDay = dayIndex(points[0].date);
  const lastDataDay = dayIndex(points[points.length - 1].date);
  // x 轴延伸到今天（今天没进星也留出空档），且至少一天宽，避免除零。
  const lastDay = Math.max(
    Math.floor(Date.now() / DAY_MS),
    lastDataDay,
    firstDay + 1,
  );
  // 数据末端与画布右缘留出几天空档，端点圆点和数值标签不被挤到边框上。
  const RIGHT_PAD_DAYS = 3;
  const span = lastDay - firstDay + RIGHT_PAD_DAYS;
  const useMonthYear = lastDay - firstDay > 130;
  const maxStars = points[points.length - 1].stars;
  const yStep = niceStep(Math.max(maxStars, 1) / 4);
  const yMax = Math.ceil(maxStars / yStep) * yStep;

  const plotW = WIDTH - MARGIN.left - MARGIN.right;
  const plotH = HEIGHT - MARGIN.top - MARGIN.bottom;
  const x = (day) => MARGIN.left + ((day - firstDay) / span) * plotW;
  const y = (stars) => MARGIN.top + (1 - stars / yMax) * plotH;

  const line = points
    .map((p) => `${r2(x(dayIndex(p.date)))},${r2(y(p.stars))}`)
    .join(" ");
  const area = `${MARGIN.left},${r2(y(0))} ${line} ${r2(x(lastDataDay))},${r2(y(0))}`;

  const yTicks = [];
  for (let i = 0; i * yStep <= yMax; i += 1) {
    const stars = i * yStep;
    yTicks.push(
      `<line x1="${MARGIN.left}" x2="${WIDTH - MARGIN.right}" y1="${r2(y(stars))}" y2="${r2(y(stars))}" stroke="#E5E7EB" stroke-width="1"/>` +
        `<text x="${MARGIN.left - 10}" y="${r2(y(stars) + 4)}" text-anchor="end" font-size="11" fill="#6B7280">${fmt(stars)}</text>`,
    );
  }

  const dataSpan = lastDay - firstDay;
  const tickCount = Math.min(6, dataSpan + 1);
  const seenLabels = new Set();
  const xTicks = [];
  for (let i = 0; i < tickCount; i += 1) {
    const day = Math.round(firstDay + (dataSpan * i) / (tickCount - 1));
    const d = new Date(day * DAY_MS);
    const label = useMonthYear
      ? `${d.getUTCFullYear()}-${String(d.getUTCMonth() + 1).padStart(2, "0")}`
      : `${d.getUTCMonth() + 1}/${d.getUTCDate()}`;
    if (seenLabels.has(label)) continue;
    seenLabels.add(label);
    xTicks.push(
      `<line x1="${r2(x(day))}" x2="${r2(x(day))}" y1="${MARGIN.top}" y2="${r2(y(0))}" stroke="#F3F4F6" stroke-width="1"/>` +
        `<text x="${r2(x(day))}" y="${HEIGHT - MARGIN.bottom + 20}" text-anchor="middle" font-size="11" fill="#6B7280">${label}</text>`,
    );
  }

  return `<svg xmlns="http://www.w3.org/2000/svg" width="${WIDTH}" height="${HEIGHT}" viewBox="0 0 ${WIDTH} ${HEIGHT}" role="img" aria-label="Star History chart for ${esc(repo)}" style="font-family:-apple-system,BlinkMacSystemFont,'Segoe UI',Helvetica,Arial,sans-serif">
<rect width="${WIDTH}" height="${HEIGHT}" fill="#ffffff"/>
<text x="${WIDTH / 2}" y="34" text-anchor="middle" font-size="17" font-weight="600" fill="#1F2937">Star History</text>
<text x="${WIDTH / 2}" y="54" text-anchor="middle" font-size="13" fill="#6B7280">${esc(repo)} · ★ ${fmt(maxStars)}</text>
<defs><linearGradient id="area" x1="0" y1="0" x2="0" y2="1"><stop offset="0" stop-color="#3B82F6" stop-opacity="0.22"/><stop offset="1" stop-color="#3B82F6" stop-opacity="0"/></linearGradient></defs>
${yTicks.join("\n")}
${xTicks.join("\n")}
<polygon points="${area}" fill="url(#area)"/>
<polyline points="${line}" fill="none" stroke="#3B82F6" stroke-width="2.25" stroke-linejoin="round" stroke-linecap="round"/>
<circle cx="${r2(x(lastDataDay))}" cy="${r2(y(maxStars))}" r="3.5" fill="#3B82F6" stroke="#ffffff" stroke-width="1.5"/>
<text x="${r2(x(lastDataDay) + 8)}" y="${r2(y(maxStars) - 8)}" font-size="12" font-weight="600" fill="#1D4ED8">${fmt(maxStars)}</text>
</svg>
`;
}

const BRANCH_README = `# Star History data

本分支由 main 分支的 \`.github/workflows/star-history.yml\` 每日自动更新，请勿手工修改。
README 通过 raw.githubusercontent.com 引用本分支的 \`star-history.svg\`。
`;

async function main() {
  const [command, targetDir] = process.argv.slice(2);
  if (command !== "snapshot" || !targetDir) {
    console.error("用法: node .github/scripts/star-history.mjs snapshot <target-dir>");
    process.exit(2);
  }
  const repo = process.env.GITHUB_REPOSITORY;
  const token = process.env.GITHUB_TOKEN;
  if (!repo || !token) {
    console.error("需要 GITHUB_REPOSITORY 与 GITHUB_TOKEN 环境变量");
    process.exit(2);
  }

  const stars = await fetchStargazers(repo, token);
  const points = cumulativeByDay(stars);
  if (points.length === 0) throw new Error("该仓库还没有星标，无法生成图表");

  const dir = path.resolve(targetDir);
  await mkdir(dir, { recursive: true });
  const totalStars = points[points.length - 1].stars;
  await writeFile(
    path.join(dir, "history.json"),
    JSON.stringify({ repository: repo, totalStars, points }, null, 2) + "\n",
  );
  await writeFile(path.join(dir, "star-history.svg"), renderSvg(repo, points));
  await writeFile(path.join(dir, "README.md"), BRANCH_README);
  console.log(`star history: ${totalStars} stars, ${points.length} days -> ${dir}`);
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  main().catch((err) => {
    console.error(err);
    process.exit(1);
  });
}
