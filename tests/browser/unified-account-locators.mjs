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
