import { describe, expect, it } from "vitest";
import { navigationReducer, type NavigationState } from "../navigation";

const initial: NavigationState = {
  current: { view: "providers", app: "claude" },
  history: [],
};
describe("client navigation", () => {
  it("returns to the actual source page through a nested flow", () => {
    let state = navigationReducer(initial, {
      type: "navigate",
      view: "services",
    });
    state = navigationReducer(state, { type: "navigate", view: "addHub" });
    state = navigationReducer(state, { type: "back" });
    expect(state.current.view).toBe("services");
    state = navigationReducer(state, { type: "back" });
    expect(state.current.view).toBe("services");
    expect(state.history).toEqual([]);
  });
  it("does not duplicate a page when it is selected again", () => {
    const state = navigationReducer(initial, {
      type: "navigate",
      view: "providers",
    });
    expect(state).toEqual(initial);
  });
  it("replaces an invalid capability view without trapping Back in a loop", () => {
    let state = navigationReducer(initial, { type: "navigate", view: "mcp" });
    state = navigationReducer(state, { type: "replace", view: "providers" });
    expect(navigationReducer(state, { type: "back" })).toEqual(initial);
  });
});

it.each([
  "providers",
  "services",
  "image",
  "records",
  "resources",
  "plaza",
  "settings",
] as const)("clears account detail and history when selecting %s", (view) => {
  const detail = navigationReducer(initial, {
    type: "navigate",
    view: "services",
    account: { kind: "relay", id: 1 },
  });
  const state = navigationReducer(detail, { type: "navigate", view });
  expect(state.current.account).toBeUndefined();
  expect(state.history).toEqual([]);
  expect(navigationReducer(state, { type: "back" })).toEqual(state);
});

it("restores image context after an account detail flow", () => {
  const image: NavigationState = {
    current: { view: "image", app: "codex-image" },
    history: [],
  };
  const detail = navigationReducer(image, {
    type: "navigate",
    view: "services",
    app: "codex",
    account: { kind: "relay", id: 1 },
  });
  expect(navigationReducer(detail, { type: "back" })).toEqual(image);
});

it.each([
  "providers",
  "services",
  "image",
  "records",
  "resources",
  "plaza",
  "settings",
] as const)(
  "returns from add onboarding to %s without exposing old history",
  (view) => {
    const caller: NavigationState = {
      current: { view, app: view === "image" ? "codex-image" : "claude" },
      history: [],
    };
    const wizard = navigationReducer(caller, {
      type: "navigate",
      view: "addHub",
    });
    expect(navigationReducer(wizard, { type: "back" })).toEqual(caller);
  },
);

it("returns a restored subordinate page to its parent", () => {
  const restored: NavigationState = {
    current: { view: "skillsDiscovery", app: "claude" },
    history: [],
  };
  const skills = navigationReducer(restored, { type: "back" });
  expect(skills.current.view).toBe("skills");
  expect(navigationReducer(skills, { type: "back" }).current.view).toBe(
    "resources",
  );
});

it("restores exact account detail through navigation history", () => {
  const account = { kind: "vendor" as const, id: 3 };
  let state = navigationReducer(initial, {
    type: "navigate",
    view: "services",
    account,
  });
  expect(state.current.account).toEqual(account);
  state = navigationReducer(state, { type: "navigate", view: "addHub" });
  expect(state.current.account).toBeUndefined();
  state = navigationReducer(state, { type: "back" });
  expect(state.current.account).toEqual(account);
  state = navigationReducer(state, { type: "navigate", view: "services" });
  expect(state.current.account).toBeUndefined();
  expect(state.history).toEqual([]);
  expect(
    navigationReducer(state, { type: "back" }).current.account,
  ).toBeUndefined();
});

it("changes app context within the same account without adding a Back step", () => {
  const accounts: NavigationState = {
    current: { view: "services", app: "codex" },
    history: [],
  };
  const account = { kind: "relay" as const, id: 1 };
  let state = navigationReducer(accounts, {
    type: "navigate",
    view: "services",
    app: "codex",
    account,
  });
  state = navigationReducer(state, {
    type: "navigate",
    view: "services",
    app: "claude",
    account,
  });
  expect(state.current.app).toBe("claude");
  expect(navigationReducer(state, { type: "back" })).toEqual(accounts);
});
