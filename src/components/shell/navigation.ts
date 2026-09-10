import { useCallback, useReducer } from "react";
import type { AppId } from "@/lib/api";

export type ClientView =
  | "providers"
  | "services"
  | "image"
  | "records"
  | "resources"
  | "plaza"
  | "settings"
  | "prompts"
  | "skills"
  | "skillsDiscovery"
  | "mcp"
  | "agents"
  | "universal"
  | "sessions"
  | "workspace"
  | "openclawEnv"
  | "openclawTools"
  | "openclawAgents"
  | "hermesMemory"
  | "addHub";

export const CLIENT_VIEWS: ClientView[] = [
  "providers",
  "services",
  "image",
  "records",
  "resources",
  "plaza",
  "settings",
  "prompts",
  "skills",
  "skillsDiscovery",
  "mcp",
  "agents",
  "universal",
  "sessions",
  "workspace",
  "openclawEnv",
  "openclawTools",
  "openclawAgents",
  "hermesMemory",
  "addHub",
];
export interface AccountRoute {
  kind: "relay" | "vendor";
  id: number;
}

export interface ClientRoute {
  view: ClientView;
  app: AppId;
  account?: AccountRoute;
}
export interface NavigationState {
  current: ClientRoute;
  history: ClientRoute[];
}
type NavigationAction =
  | {
      type: "navigate" | "replace";
      view: ClientView;
      app?: AppId;
      account?: AccountRoute;
    }
  | { type: "app"; app: AppId }
  | { type: "back" };
export function navigationReducer(
  state: NavigationState,
  action: NavigationAction,
): NavigationState {
  if (action.type === "back")
    return {
      current: state.history.at(-1) ?? {
        view: "providers",
        app: state.current.app === "codex-image" ? "codex" : state.current.app,
      },
      history: state.history.slice(0, -1),
    };
  if (action.type === "app")
    return { ...state, current: { ...state.current, app: action.app } };
  const current: ClientRoute = {
    view: action.view,
    app: action.app ?? state.current.app,
    ...(action.view === "services" && action.account
      ? { account: action.account }
      : {}),
  };
  if (
    current.view === state.current.view &&
    current.app === state.current.app &&
    current.account?.kind === state.current.account?.kind &&
    current.account?.id === state.current.account?.id
  )
    return state;
  return {
    current,
    history:
      action.type === "navigate"
        ? [...state.history, state.current]
        : state.history,
  };
}
export function useClientNavigation(
  initial: () => ClientView,
  initialApp: () => AppId,
) {
  const [state, dispatch] = useReducer(navigationReducer, undefined, () => {
    const savedView = initial();
    const app = initialApp();
    const view =
      savedView === "providers" && app === "codex-image" ? "image" : savedView;
    return {
      current: { view, app: view === "image" ? "codex-image" : app },
      history: [],
    };
  });
  const navigate = useCallback(
    (view: ClientView, app?: AppId, account?: AccountRoute) =>
      dispatch({ type: "navigate", view, app, account }),
    [],
  );
  const replace = useCallback(
    (view: ClientView) => dispatch({ type: "replace", view }),
    [],
  );
  const setApp = useCallback(
    (app: AppId) => dispatch({ type: "app", app }),
    [],
  );
  const back = useCallback(() => dispatch({ type: "back" }), []);
  return {
    view: state.current.view,
    app: state.current.app,
    account: state.current.account,
    navigate,
    replace,
    setApp,
    back,
    canGoBack: state.history.length > 0 || state.current.view !== "providers",
  };
}
