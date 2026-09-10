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
    expect(state).toEqual(initial);
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

it("restores application context when returning from services to image", () => {
  let state = navigationReducer(initial, {
    type: "navigate",
    view: "image",
    app: "codex-image",
  });
  state = navigationReducer(state, {
    type: "navigate",
    view: "services",
    app: "codex",
  });
  expect(navigationReducer(state, { type: "back" }).current).toEqual({
    view: "image",
    app: "codex-image",
  });
});
