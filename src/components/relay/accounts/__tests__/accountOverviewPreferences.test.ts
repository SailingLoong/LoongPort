import { expect, it, vi } from "vitest";
import { accountPlatforms } from "../accountPlatforms";
import {
  ACCOUNT_OVERVIEW_PREFERENCES_KEY,
  createAccountOverviewPreferencesRepository,
} from "../accountOverviewPreferences";

const ids = accountPlatforms.map((group) => group.id);
const storage = (value: string | null) => ({
  getItem: vi.fn(() => value),
  setItem: vi.fn(),
});
it.each([null, "{bad", "null", '{"version":99,"hideExhausted":false}'])(
  "defaults invalid/missing preferences without writing %#",
  (value) => {
    const source = storage(value);
    const repo = createAccountOverviewPreferencesRepository(source);
    const prefs = repo.read();
    expect(prefs.hideExhausted).toBe(true);
    expect(prefs.groupOrder).toEqual(ids);
    expect(prefs.collapsedGroups).toEqual([]);
    expect(source.setItem).not.toHaveBeenCalled();
  },
);
it("validates unknown and duplicate groups and appends new platforms without deleting temporarily absent ones", () => {
  const source = storage(
    JSON.stringify({
      version: 1,
      groupOrder: ["workbuddy", "workbuddy", "not-a-platform"],
      collapsedGroups: ["qoder", "qoder", "unknown"],
      sortByGroup: {
        workbuddy: "expiryAsc",
        unknown: "balanceDesc",
        qoder: "bad",
      },
      hideExhausted: false,
    }),
  );
  const prefs = createAccountOverviewPreferencesRepository(source).read();
  expect(prefs.groupOrder).toEqual([
    "workbuddy",
    ...ids.filter((id) => id !== "workbuddy"),
  ]);
  expect(prefs.collapsedGroups).toEqual(["qoder"]);
  expect(prefs.sortByGroup).toEqual({ workbuddy: "expiryAsc" });
  expect(prefs.hideExhausted).toBe(false);
  expect(prefs.groupOrder).toContain("github_copilot");
});
it("writes one UI-only snapshot per explicit save, stripping account/native/consent fields", () => {
  const source = storage(null);
  const repo = createAccountOverviewPreferencesRepository(source);
  const prefs = repo.read();
  const result = repo.save({
    ...prefs,
    hideExhausted: false,
    selection: { id: "private", incarnation: "private" },
    token: "secret",
    autoClaimEnabled: true,
  } as any);
  expect(result.persisted).toBe(true);
  expect(source.setItem).toHaveBeenCalledTimes(1);
  const [key, text] = source.setItem.mock.calls[0] as unknown as [
    string,
    string,
  ];
  expect(key).toBe(ACCOUNT_OVERVIEW_PREFERENCES_KEY);
  expect(JSON.parse(text)).toEqual({ ...prefs, hideExhausted: false });
  expect(text).not.toMatch(/private|secret|selection|autoClaim/);
});
it("read errors default and write errors keep a normalized session result without claiming persistence", () => {
  const source = {
    getItem: vi.fn(() => {
      throw new Error("unavailable");
    }),
    setItem: vi.fn(() => {
      throw new Error("quota");
    }),
  };
  const repo = createAccountOverviewPreferencesRepository(source);
  const prefs = repo.read();
  const result = repo.save({ ...prefs, hideExhausted: false });
  expect(result.persisted).toBe(false);
  expect(result.preferences.hideExhausted).toBe(false);
  expect(repo.read().hideExhausted).toBe(true);
});
it("unavailable storage remains an explicit session-only result", () => {
  const repo = createAccountOverviewPreferencesRepository(null);
  expect(repo.save(repo.read()).persisted).toBe(false);
});
