import { invoke } from "@tauri-apps/api/core";

export interface ClaimPlan {
  id: string;
  name: string | null;
  description: string | null;
  priority: number;
  units: number | null;
  grants: { name: string; units: number | null; period: string }[];
}
export interface ClaimRecord {
  status:
    | "unknown"
    | "claimable"
    | "claimed"
    | "noClaim"
    | "verificationRequired"
    | "loginExpired"
    | "resultPending"
    | "cancelled";
  plans: ClaimPlan[];
  planId: string | null;
  planName: string | null;
  startsAt: number | null;
  endsAt: number | null;
  checkedAt: number | null;
  reason: string | null;
  canClaim: boolean;
}
export interface ClaimState {
  enabled: boolean;
  participants: string[];
  records: Record<string, ClaimRecord>;
  busy: boolean;
}
export const zcodeClaimApi = {
  state: (dataRoot?: string) =>
    invoke<ClaimState>("get_zcode_claim_state", { dataRoot }),
  setAuto: (
    dataRoot: string | undefined,
    enabled: boolean,
    participants: string[],
  ) =>
    invoke<ClaimState>("set_zcode_claim_auto", {
      dataRoot,
      enabled,
      participants,
    }),
  start: (dataRoot: string | undefined, ids: string[], previewOnly: boolean) =>
    invoke<ClaimState>("start_zcode_claim", { dataRoot, ids, previewOnly }),
  cancel: (dataRoot?: string) =>
    invoke<ClaimState>("cancel_zcode_claim", { dataRoot }),
};
