/**
 * 档位行的「模型验证」入口按钮。挂在 hover 操作组里、连通检测之后。
 * 模块下线（`MODEL_VERIFICATION_ENABLED = false`）或档位不可验证
 * （`canVerifyModels = false`）时不渲染。
 */
import { useTranslation } from "react-i18next";
import { Fingerprint, Loader2 } from "lucide-react";

import { Button } from "@/components/ui/button";

import { MODEL_VERIFICATION_ENABLED } from "./availability";
import {
  useTierVerification,
  type VerificationTierRef,
} from "./TierVerificationProvider";

export function TierVerifyButton({
  tier,
  canVerify,
}: {
  tier: VerificationTierRef;
  canVerify: boolean;
}) {
  const { openVerification, isVerifying } = useTierVerification();
  const { t } = useTranslation();

  if (!MODEL_VERIFICATION_ENABLED || !canVerify) {
    return null;
  }

  return (
    <Button
      type="button"
      variant="ghost"
      size="icon"
      className="h-7 w-7 shrink-0 p-1 text-muted-foreground hover:text-foreground"
      onClick={() => openVerification(tier)}
      title={t("loongport.modelVerification.title")}
    >
      {isVerifying(tier.providerId) ? (
        <Loader2 className="h-3.5 w-3.5 animate-spin" />
      ) : (
        <Fingerprint className="h-3.5 w-3.5" />
      )}
    </Button>
  );
}
