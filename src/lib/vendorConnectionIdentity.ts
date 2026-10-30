import type {
  VendorPrepareConnectionResult,
  VendorQueryConnectionResult,
  VendorReviewedReceipt,
} from "@/lib/api/vendor";
import type { AppId } from "@/lib/api";

type VendorReceipt =
  | VendorPrepareConnectionResult
  | VendorQueryConnectionResult
  | VendorReviewedReceipt;
type IdentifiedReceipt = VendorReceipt & {
  app: AppId;
  rowId: number;
  plan: string;
  operationId: string;
  providerId: string;
};
export interface VendorConnectionTarget {
  app: AppId;
  rowId: number;
  plan: string;
  operationId?: string;
  providerId?: string;
}

/** Compare native receipt identity only; status and revisions remain the caller's responsibility. */
export function matchesVendorConnectionTarget(
  receipt: VendorReceipt,
  target: VendorConnectionTarget,
): receipt is IdentifiedReceipt {
  if (
    !("rowId" in receipt) ||
    !("plan" in receipt) ||
    !("providerId" in receipt)
  )
    return false;
  return (
    typeof receipt.operationId === "string" &&
    receipt.operationId.length > 0 &&
    typeof receipt.providerId === "string" &&
    receipt.providerId.length > 0 &&
    typeof receipt.plan === "string" &&
    receipt.plan.length > 0 &&
    typeof receipt.rowId === "number" &&
    Number.isSafeInteger(receipt.rowId) &&
    receipt.rowId >= 0 &&
    receipt.app === target.app &&
    receipt.rowId === target.rowId &&
    receipt.plan === target.plan &&
    (target.operationId === undefined ||
      receipt.operationId === target.operationId) &&
    (target.providerId === undefined ||
      receipt.providerId === target.providerId)
  );
}
