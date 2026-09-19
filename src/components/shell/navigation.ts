import { useCallback, useReducer } from "react";
import type { AppId } from "@/lib/api";

export type ClientView =
  | "providers"
  | "services"
  | "image"
  | "records"
  | "usage"
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
  "usage",
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
// Routes without a parent are primary destinations. Only subordinate flows
// retain their caller; selecting a primary destination starts a new branch.
const parentViews: Partial<Record<ClientView, ClientView>> = {
  addHub: "services",
  sessions: "records",
  skills: "resources",
  skillsDiscovery: "skills",
  mcp: "resources",
  prompts: "resources",
  agents: "resources",
  universal: "resources",
  workspace: "resources",
  openclawEnv: "resources",
  openclawTools: "resources",
  openclawAgents: "resources",
  hermesMemory: "resources",
};

export function getNavigationSection(view: ClientView): ClientView {
  const parent = parentViews[view];
  return parent ? getNavigationSection(parent) : view;
}

function getParentView(route: ClientRoute): ClientView | undefined {
  return route.account ? "services" : parentViews[route.view];
}

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
  if (action.type === "back") {
    const parent = getParentView(state.current);
    if (!parent) return state;
    return {
      current: state.history.at(-1) ?? { view: parent, app: state.current.app },
      history: state.history.slice(0, -1),
    };
  }
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
  const sameAccount =
    current.account !== undefined &&
    current.account.kind === state.current.account?.kind &&
    current.account.id === state.current.account?.id;
  return {
    current,
    history: !getParentView(current)
      ? []
      : action.type === "navigate" && !sameAccount
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
    canGoBack: getParentView(state.current) !== undefined,
  };
}
