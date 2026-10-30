import { useLayoutEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";
import { vendorApi, VENDOR_CATALOG } from "@/lib/api/vendor";
import { extractErrorMessage } from "@/utils/errorUtils";
import type { OfficialApiPageProps } from "./OfficialApiPage";
export function useOfficialApiPageViewModel({
  sourceAppId,
  vendorId,
  onBack,
  onConnected,
}: OfficialApiPageProps) {
  const { t } = useTranslation();
  const [loginningVendor, setLoginningVendor] = useState<string | null>(null);
  const [showOpenCodeRecovery, setShowOpenCodeRecovery] = useState(false);
  const alive = useRef(true);
  const generation = useRef(0);
  const pending = useRef(false);
  const [, publishOwner] = useState(0);
  useLayoutEffect(() => {
    alive.current = true;
    publishOwner((value) => value + 1);
    if (!pending.current) setLoginningVendor(null);
    setShowOpenCodeRecovery(false);
    return () => {
      alive.current = false;
      generation.current += 1;
    };
  }, [sourceAppId, vendorId]);
  const ownerGeneration = generation.current;
  const handlePick = async (vendorId: string) => {
    if (
      !alive.current ||
      ownerGeneration !== generation.current ||
      pending.current
    )
      return;
    pending.current = true;
    const request = generation.current;
    const current = () => alive.current && request === generation.current;
    setLoginningVendor(vendorId);
    setShowOpenCodeRecovery(false);
    try {
      const result = await vendorApi.openLogin(vendorId, sourceAppId);
      // null = 用户自己关了窗或超时，不出提示（他知道自己干了什么）。
      if (!current() || result === null) return;
      alive.current = false;
      generation.current += 1;
      setLoginningVendor(null);
      toast.success(t("loongport.session.connected"));
      if (onConnected)
        onConnected({
          kind: "vendor",
          rowId: result.rowId,
          name:
            VENDOR_CATALOG.find((vendor) => vendor.id === vendorId)
              ?.displayName ?? vendorId,
        });
      else onBack();
    } catch (e) {
      if (current()) {
        toast.error(extractErrorMessage(e));
        // Offer existing forms, not a diagnosis or an automatic credential import.
        if (vendorId === "opencode") setShowOpenCodeRecovery(true);
      }
    } finally {
      pending.current = false;
      if (alive.current) setLoginningVendor(null);
    }
  };
  const back = () => {
    if (!alive.current || ownerGeneration !== generation.current) return;
    alive.current = false;
    generation.current += 1;
    onBack();
  };
  const visibleVendors = VENDOR_CATALOG.filter(
    (vendor) => !vendorId || vendor.id === vendorId,
  );
  const showOpenCodeEntry = !vendorId || vendorId === "opencode";
  return {
    loginningVendor,
    visibleVendors,
    showOpenCodeEntry,
    recoveryVisible: showOpenCodeRecovery && showOpenCodeEntry,
    handlePick,
    back,
  };
}
