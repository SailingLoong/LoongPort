import { CopilotAuthSection } from "@/components/providers/forms/CopilotAuthSection";
import { CodexOAuthSection } from "@/components/providers/forms/CodexOAuthSection";
import { XaiOAuthSection } from "@/components/providers/forms/XaiOAuthSection";
import { MiniMaxCodeSection } from "@/components/providers/forms/MiniMaxCodeSection";
import { RestoreOfficialLoginButton } from "@/components/relay/RestoreOfficialLoginButton";
import { WorkBuddyAccounts } from "@/components/workbuddy/WorkBuddyAccounts";
import { TraeAccounts } from "@/components/trae/TraeAccounts";
import { ZCodeAccountPanel } from "@/components/zcode/ZCodeAccountPanel";
import type { AccountPlatform } from "@/components/relay/accounts/accountPlatforms";

/** A single selected manager. Never mount a second copy in settings or provider dialogs. */
export function AuthCenterPanel({
  platform,
  onNavigationBlockedChange,
}: {
  platform: AccountPlatform;
  onNavigationBlockedChange?: (blocked: boolean) => void;
}) {
  switch (platform) {
    case "github_copilot":
      return <CopilotAuthSection />;
    case "codex_oauth":
      return (
        <>
          <CodexOAuthSection showAccountQuota />
          <RestoreOfficialLoginButton />
        </>
      );
    case "minimax_code":
      return <MiniMaxCodeSection />;
    case "xai_oauth":
      return <XaiOAuthSection />;
    case "workbuddy":
      return <WorkBuddyAccounts />;
    case "trae":
      return <TraeAccounts />;
    case "zcode":
      return <ZCodeAccountPanel onBusyChange={onNavigationBlockedChange} />;
  }
}
