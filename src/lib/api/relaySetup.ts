import type {
  ApplicationRoutingPreparedRequest,
  ApplicationRoutingChange,
  ApplicationRoutingCommandResult,
} from "./applicationRouting";
import type { SwitchTierCommandResult } from "./relay";
import type { AppId } from "./types";

/** Native setup DTOs: local existence, remote-resource facts and connection
 * publication are distinct; none of these types contains credentials. */
export interface RelayTierSelection {
  app: AppId;
  groupId: string;
}
export interface RelayTierChoice extends RelayTierSelection {
  groupName: string;
  providerId: string;
  localConfiguration: "absent" | "present";
  compatibility: "unknown" | "configured";
  keyStatus: "unknown";
}
export interface RelayDiscovery {
  schemaVersion: 1;
  relayId: number;
  account: { identity: string; siteOrigin: string; accountLabel: string };
  revision: string;
  choices: RelayTierChoice[];
}
export type RelayPrepareRequest =
  | {
      kind: "start";
      operationId: string;
      relayId: number;
      discoveryRevision: string;
      selections: RelayTierSelection[];
    }
  | {
      kind: "resume";
      operationId: string;
      expectedReceiptRevision: number;
      apps: AppId[];
    };
export interface RelayKeyFact {
  groupId: string;
  groupName: string;
  state: "notStarted" | "requesting" | "ready" | "rejected" | "unknown";
  origin?: "created" | "reused" | "observed";
  remoteKeyId?: string;
  reasonCode?: string;
}
export interface RelayTargetFact extends RelayTierSelection {
  providerId: string;
  configuration: "notStarted" | "existing" | "prepared" | "failed" | "unknown";
  compatibility: "unknown" | "supported" | "unsupported";
  reasonCode?: string;
  connection: {
    state:
      | "notRequested"
      | "confirmationRequired"
      | "applying"
      | "applied"
      | "failedBeforeWrite"
      | "unknown"
      | "changed";
    requestId?: string;
    intent?: {
      preparedRequest: ApplicationRoutingPreparedRequest;
      change: ApplicationRoutingChange;
      beforeRevision: string;
      providerRevision?: string;
    };
    quitChatgpt?: boolean;
    observedCurrent?: boolean;
    writeAcknowledged?: boolean;
    observedRevision?: string;
    appliedRevision?: string;
    reasonCode?: string;
  };
}
export interface RelaySetupReceipt {
  schemaVersion: 1;
  operationId: string;
  receiptRevision: number;
  relayId: number;
  accountIdentity: string;
  selectionDigest: string;
  selections: RelayTierSelection[];
  status: "running" | "settled" | "needsReconcile";
  groups: RelayKeyFact[];
  targets: RelayTargetFact[];
  createdKeyCount: number;
  createdAt: number;
  updatedAt: number;
}

export interface RelayPreparationRejection {
  schemaVersion: 1;
  outcome: "rejectedBeforeOperation";
  operationId: string;
  reasonCode: string;
}
/** Only the native owner can issue this proof. A transport error or missing
 * receipt is never converted into this result by the client. */
export function isRelayPreparationRejection(
  value: unknown,
  operationId: string,
): value is RelayPreparationRejection {
  if (!value || typeof value !== "object") return false;
  const error = value as Partial<RelayPreparationRejection>;
  return (
    error.schemaVersion === 1 &&
    error.outcome === "rejectedBeforeOperation" &&
    error.operationId === operationId &&
    typeof error.reasonCode === "string"
  );
}

export interface RelayConnectionReview {
  receipt: RelaySetupReceipt;
  confirmation: Extract<
    SwitchTierCommandResult,
    { status: "confirmationRequired" }
  > | null;
}
export interface RelayConnectionOutcome {
  receipt: RelaySetupReceipt;
  result: ApplicationRoutingCommandResult | null;
}
