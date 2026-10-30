/** Shared with the component regression so browser role names stay locale-correct. */
export function platformAccountFilterName(t) {
  return t("loongport.accounts.platformFilter");
}

export function nativeAccountKeyboardSteps(t) {
  return [
    ["Home", t("loongport.accounts.allPlatforms")],
    ["ArrowRight", "MiniMax Code 国内版"],
    ["End", "OpenCode"],
  ];
}

export const nativeAccountOverviewCounts = { details: 0, accountButtons: 1 };

/** The overview uses a localized, independent Details button inside its account row. */
export function overviewAccountDetailsName(t, name) {
  return t("accountsOverview.viewAccount").replaceAll("{{name}}", name);
}

/** Existing native workspace selection remains a separate, currently Chinese label. */
export function nativeWorkspaceAccountName(name) {
  return `查看 ${name}`;
}
