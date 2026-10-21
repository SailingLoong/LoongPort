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
  editionScoped = false,
  onNavigationBlockedChange,
}: {
  platform: AccountPlatform;
  editionScoped?: boolean;
  onNavigationBlockedChange?: (blocked: boolean) => void;
}) {
  switch (platform) {
    case "qoder":
    case "openrouter":
    case "opencode":
      return (
        <p>
          请从服务与账号页面进入对应服务连接，不以 API 配置替代原生账号授权。
        </p>
      );
    case "github_copilot":
      return <CopilotAuthSection unified={editionScoped} />;
    case "codex_oauth":
      return (
        <>
          <CodexOAuthSection showAccountQuota unified={editionScoped} />
          <RestoreOfficialLoginButton />
        </>
      );
    case "minimax_code":
      return (
        <MiniMaxCodeSection
          unified={editionScoped}
          edition={editionScoped ? "cn" : undefined}
        />
      );
    case "minimax_code_global":
      return <MiniMaxCodeSection unified={editionScoped} edition="global" />;
    case "trae_cn":
      return (
        <TraeAccounts key="trae-cn" unified={editionScoped} edition="cn" />
      );
    case "xai_oauth":
      return <XaiOAuthSection unified={editionScoped} />;
    case "workbuddy":
      return <WorkBuddyAccounts unified={editionScoped} />;
    case "trae":
      return <TraeAccounts unified={editionScoped} />;
    case "zcode":
      return (
        <ZCodeAccountPanel
          unified={editionScoped}
          onBusyChange={onNavigationBlockedChange}
        />
      );
  }
}
