import { mockIPC, mockWindows } from "@tauri-apps/api/mocks";
import type { ApplicationRoutingRequest } from "@/lib/api/applicationRouting";
import type {
  PreparedRouteTarget,
  RouteBinding,
  SharedGroup,
} from "@/lib/api/sharedRouting";
import type { ServiceSource } from "@/lib/api/serviceSources";
import { DEFAULT_VISIBLE_APPS } from "@/config/appConfig";

export type ApplicationsC2Scenario = "normal" | "unknown" | "pending";
export const APPROVED_C2_SHA256 =
  "05d8b77c3a164e69eeb03ab7f8ea03691d90bda80988a1143c81b2d6f5b41544";
export const APPLICATIONS_C2_DECLARATION =
  "Current-checkout real App/CSS; all native DTOs and writes are synthetic browser-memory fixtures. Rendering/interaction evidence only: not native, live-account, eligibility, forwarding, or successful-model-request acceptance.";
const copy = <T>(value: T): T => structuredClone(value);

/** Test-only native boundary. No real accounts, credentials, OS writes or requests. */
export function installApplicationsC2Fixture(
  scenario: ApplicationsC2Scenario = "normal",
) {
  for (const [key, value] of Object.entries({
    "loongport-last-app": "codex",
    "loongport-last-view": "providers",
    language: "zh",
    "loongport-theme": "light",
    "loongport-sidebar-collapsed": "false",
  }))
    localStorage.setItem(key, value);
  document.documentElement.lang = "zh";
  const calls: { command: string; payload: unknown }[] = [];
  const rejectedCommands: string[] = [];
  const visibleApps = Object.fromEntries(
    Object.keys(DEFAULT_VISIBLE_APPS).map((app) => [
      app,
      ["claude", "codex", "gemini"].includes(app),
    ]),
  );
  const groups: SharedGroup[] = [
    {
      id: "synthetic-daily",
      name: "日常编码",
      revision: 3,
      members: [
        { sourceId: "synthetic-qoder", model: "fixture-qoder-model" },
        { sourceId: "synthetic-minimax", model: "model-a" },
        { sourceId: "synthetic-workbuddy", model: "model-b" },
      ],
    },
  ];
  const sources: ServiceSource[] = [
    {
      sourceId: "synthetic-qoder",
      name: "Qoder C",
      account: {
        kind: "nativeAccount",
        platform: "qoder_sdk",
        id: "synthetic-qoder-account",
      },
      serviceName: null,
      accountLabel: null,
      configurationName: null,
      bindings: [
        {
          appId: "codex",
          providerId: "synthetic-qoder-provider",
          model: "fixture-qoder-model",
          usageState: "enabled",
          canSelect: true,
        },
      ],
    },
    {
      sourceId: "synthetic-minimax",
      name: "MiniMax A",
      account: {
        kind: "nativeAccount",
        platform: "minimax_code",
        region: "cn",
        id: "synthetic-minimax-account",
      },
      serviceName: null,
      accountLabel: null,
      configurationName: null,
      bindings: [
        {
          appId: "codex",
          providerId: "synthetic-minimax-provider",
          model: "model-a",
          usageState: "current",
          canSelect: true,
        },
      ],
    },
    {
      sourceId: "synthetic-workbuddy",
      name: "WorkBuddy B",
      account: {
        kind: "nativeAccount",
        platform: "workbuddy",
        id: "synthetic-workbuddy-account",
      },
      serviceName: null,
      accountLabel: null,
      configurationName: null,
      bindings: [
        {
          appId: "codex",
          providerId: "synthetic-workbuddy-provider",
          model: "model-b",
          usageState: "enabled",
          canSelect: true,
        },
      ],
    },
  ];
  const binding: RouteBinding = {
    revision: 2,
    target: { kind: "manual", sourceId: "synthetic-minimax", model: "model-a" },
  };
  const originalRequest: ApplicationRoutingRequest = {
    id: "synthetic-original-pending",
    scope: "applicationRouting",
    draftDigest: "synthetic-original-digest",
    revision: "synthetic-original-revision",
  };
  let pendingRequest = scenario === "pending" ? originalRequest : null;
  let prepared: PreparedRouteTarget | null = null;
  let lastResult: {
    request: ApplicationRoutingRequest;
    status: "completed" | "unknown" | "discarded";
  } | null = null;
  let prepareSequence = 0;
  const settings = {
    showInTray: true,
    minimizeToTrayOnClose: false,
    enableClaudePluginIntegration: false,
    language: "zh",
    visibleApps,
    enableLocalProxy: true,
    showProfileSwitcher: false,
    useAppWindowControls: false,
  };
  const reject = (command: string): never => {
    rejectedCommands.push(command);
    throw new Error(`Unexpected synthetic fixture command: ${command}`);
  };
  const same = (a: unknown, b: unknown) =>
    JSON.stringify(a) === JSON.stringify(b);
  const requireCodex = (args: Record<string, unknown>) => {
    if (args.appType !== "codex")
      throw new Error("Synthetic fixture only authorizes Codex route writes");
  };
  const handler = (command: string, payload: unknown) => {
    calls.push({ command, payload: copy(payload ?? {}) });
    const args = (payload ?? {}) as Record<string, unknown>;
    switch (command) {
      case "get_runtime_availability":
        return { credentialRuntimeAvailable: true };
      case "get_bootstrap_settings":
      case "get_settings":
        return copy(settings);
      case "set_app_visibility":
        if (
          typeof args.app !== "string" ||
          !(args.app in visibleApps) ||
          typeof args.visible !== "boolean"
        )
          throw new Error("Invalid synthetic app visibility");
        visibleApps[args.app] = args.visible;
        return copy(visibleApps);
      case "get_credential_protection_status":
        return {
          mode: "plaintext",
          status: "available",
          shouldPrompt: false,
          canConfigure: true,
          canUnlock: false,
          canSwitchToPlaintext: true,
          canCreateSeparateConfiguration: false,
          reviewToken: "synthetic-status",
          restartRequired: false,
          errorCode: null,
        };
      case "get_shared_routing_state":
        return copy({
          appId: args.appType,
          binding: args.appType === "codex" ? binding : null,
          groups,
          pendingRequest: args.appType === "codex" ? pendingRequest : null,
          canApply: !(args.appType === "codex" && pendingRequest),
          blockedReason:
            args.appType === "codex" && pendingRequest
              ? "routing_groups.recovery_required"
              : null,
        });
      case "get_service_sources":
        return copy(sources);
      case "get_application_routing":
        // Match commands/application_routing.rs: a non-writable mode is a
        // structured rejection, not a successful board with ready modeState.
        if (args.appType === "codex" && pendingRequest) {
          throw {
            code: "mode.verification_required",
            modeState: {
              status: "pending",
              writeProtocol: null,
              blockedReason: "mode.verification_required",
              mode: null,
              attached: null,
              currentProviderId: null,
              directProviderId: null,
              publicationStarted: true,
              canWrite: false,
              canRecheck: true,
              legacyCommonConfigWritable: false,
            },
          };
        }
        return {
          modeState: {
            status: "ready",
            mode: "proxy",
            attached: true,
            currentProviderId: "synthetic-minimax-provider",
            directProviderId: null,
            publicationStarted: false,
            canWrite: true,
            writeProtocol: "native",
            blockedReason: null,
            canRecheck: true,
          },
          routingActive: false,
          autoFailoverEnabled: false,
          model: "model-a",
          modelOptions: [],
          chainIds: [],
          tiers: [],
        };
      case "prepare_application_route_target": {
        requireCodex(args);
        if (
          pendingRequest ||
          !same(args.target, {
            kind: "orderedGroup",
            groupId: "synthetic-daily",
          })
        )
          throw new Error("Invalid synthetic target review");
        prepared = {
          target: copy(args.target) as PreparedRouteTarget["target"],
          request: {
            id: `synthetic-prepared-${++prepareSequence}`,
            scope: "applicationRouting",
            draftDigest: `synthetic-digest-${prepareSequence}`,
            revision: `synthetic-revision-${binding.revision}`,
          },
          expectedBindingRevision: binding.revision,
          groupRevision: groups[0].revision,
        };
        return copy(prepared);
      }
      case "apply_application_route_target": {
        requireCodex(args);
        if (
          !prepared ||
          !same(args.prepared, prepared) ||
          prepared.expectedBindingRevision !== binding.revision
        )
          throw new Error("Exact unused synthetic native receipt required");
        const review = prepared;
        prepared = null;
        if (scenario === "unknown")
          lastResult = { request: review.request, status: "unknown" };
        else {
          binding.target = copy(review.target);
          binding.revision++;
          lastResult = { request: review.request, status: "completed" };
        }
        return copy(lastResult);
      }
      case "get_application_route_target_result":
        requireCodex(args);
        if (pendingRequest && same(args.request, pendingRequest))
          return { request: copy(pendingRequest), status: "pending" };
        if (!lastResult || !same(args.request, lastResult.request))
          throw new Error("Unknown synthetic operation receipt");
        return copy(lastResult);
      case "recover_application_route_target":
        requireCodex(args);
        if (!pendingRequest || !same(args.request, pendingRequest))
          throw new Error("Original app-local pending receipt required");
        lastResult = { request: pendingRequest, status: "discarded" };
        pendingRequest = null;
        return copy(lastResult);
      case "get_startup_recovery":
        throw "secret.no_pending_operation";
      case "get_cc_switch_import_preview":
        return {
          sourceExists: false,
          canImport: false,
          providers: {
            willImport: 0,
            skipped: [],
            mergedToRelay: [],
            cannotFingerprint: 0,
          },
          mcpServers: 0,
          prompts: 0,
          skills: 0,
          notes: [],
        };
      case "get_zcode_account_library":
        return {
          revision: "synthetic-empty",
          profiles: [],
          current: null,
          pending: false,
          nativeUnconfirmed: false,
          actions: {
            canAdd: true,
            canImport: true,
            canBackup: false,
            canEditLabels: false,
            blockedReason: null,
          },
        };
      // Unrelated account-manager inventories are outside this routing fixture.
      // Empty adjunct DTOs do not qualify/exclude any shared-group member.
      case "list_minimax_code_accounts":
      case "list_workbuddy_accounts":
      case "list_workbuddy_inference_accounts":
      case "list_trae_accounts":
        return [];
      case "get_providers":
        return {};
      case "get_current_provider":
        return "";
      case "get_proxy_status":
        return {
          running: false,
          address: "127.0.0.1",
          port: 0,
          active_connections: 0,
          total_requests: 0,
          success_requests: 0,
          failed_requests: 0,
          success_rate: 0,
          uptime_seconds: 0,
          current_provider: null,
          current_provider_id: null,
          last_request_at: null,
          last_error: null,
          failover_count: 0,
          active_targets: [],
        };
      case "get_proxy_takeover_status":
        return { claude: false, codex: false, gemini: false, grokbuild: false };
      case "service_onboarding_status":
        return { shouldPrompt: false, completed: true, plazaVisible: false };
      case "get_migration_result":
      case "plugin:window|is_maximized":
        return false;
      case "get_skills_migration_result":
      case "get_dismissed_update_version":
      case "get_app_update_status":
        return null;
      case "check_env_conflicts":
      case "get_pending_announcements":
      case "get_active_model_mismatches":
      case "scan_unmanaged_skills":
      case "relay_check_session":
        return [];
      case "plugin:app|version":
        return "0.0.0-synthetic-c2";
      case "plugin:os|platform":
        return "linux";
      case "set_window_theme":
      case "plugin:window|set_decorations":
      case "update_tray_menu":
        return null;
      default:
        return reject(command);
    }
  };
  mockWindows("main");
  mockIPC(handler, { shouldMockEvents: true });
  // Official event mock ignores unrecognised event names; close that permissive
  // edge while retaining its actual listen/emit/unlisten implementation.
  const internals = (
    window as unknown as {
      __TAURI_INTERNALS__: {
        invoke: (
          command: string,
          args?: unknown,
          options?: unknown,
        ) => Promise<unknown>;
      };
    }
  ).__TAURI_INTERNALS__;
  const officialInvoke = internals.invoke;
  internals.invoke = async (command, args, options) => {
    if (
      command.startsWith("plugin:event|") &&
      ![
        "plugin:event|listen",
        "plugin:event|emit",
        "plugin:event|unlisten",
      ].includes(command)
    )
      return reject(command);
    return officialInvoke(command, args, options);
  };
  return {
    calls,
    rejectedCommands,
    scenario,
    declaration: APPLICATIONS_C2_DECLARATION,
    approvedImageSha256: APPROVED_C2_SHA256,
  };
}
