import type { ProviderCategory } from "@/types";
import type { PresetTheme } from "./claudeProviderPresets";
import {
  getPiModelCatalogReference,
  piModel,
  type PiCatalogModel,
} from "./piModelCatalog";
import {
  getPiThinkingProfile,
  resolvePiThinkingProfile,
  type PiThinkingLevelMap,
} from "./piThinkingProfiles";

export type PiApiFormat =
  | "openai-completions"
  | "openai-responses"
  | "anthropic-messages"
  | "google-generative-ai"
  | "bedrock-converse-stream";

export type PiPresetModel = PiCatalogModel & {
  thinkingLevelMap?: PiThinkingLevelMap;
  compat?: Record<string, unknown>;
};

export interface PiProviderPreset {
  name: string;
  nameKey?: string;
  providerKey: string;
  websiteUrl: string;
  apiKeyUrl?: string;
  settingsConfig: {
    name: string;
    baseUrl: string;
    api: PiApiFormat;
    apiKey: string;
    headers?: Record<string, string>;
    compat?: Record<string, unknown>;
    models: PiPresetModel[];
  };
  category?: ProviderCategory;
  theme?: PresetTheme;
  icon?: string;
  iconColor?: string;
}

const OPENAI_COMPLETIONS_COMPAT = {
  supportsStore: false,
  supportsDeveloperRole: false,
  maxTokensField: "max_tokens",
} as const;

const DEEPSEEK_THINKING_COMPAT = {
  ...OPENAI_COMPLETIONS_COMPAT,
  requiresReasoningContentOnAssistantMessages: true,
  thinkingFormat: "deepseek",
} as const;

const XIAOMI_THINKING_COMPAT = {
  requiresReasoningContentOnAssistantMessages: true,
  thinkingFormat: "deepseek",
} as const;

const KIMI_K3_COMPAT = {
  supportsStore: false,
  supportsDeveloperRole: false,
  supportsReasoningEffort: true,
  maxTokensField: "max_tokens",
  supportsStrictMode: false,
  thinkingFormat: "openai",
  requiresReasoningContentOnAssistantMessages: true,
  deferredToolsMode: "kimi",
} as const;

/**
 * Pi-native provider catalog.
 *
 * This list is independently maintained because provider protocol, endpoint
 * roots and model capabilities are application-specific. It was initially
 * aligned with the OpenCode catalog, but Pi does not import or derive from
 * another application's presets at runtime.
 */
const piProviderPresetDefinitions: PiProviderPreset[] = [
  {
    name: "Kimi",
    providerKey: "loongport-kimi",
    websiteUrl: "https://platform.kimi.com",
    apiKeyUrl: "https://platform.kimi.com/console/api-keys",
    settingsConfig: {
      name: "Kimi",
      baseUrl: "https://api.moonshot.cn/v1",
      api: "openai-completions",
      apiKey: "",
      models: [
        piModel("moonshotai/kimi-k2.7-code", {
          id: "kimi-k2.7-code",
          thinkingProfile: "offUnsupported",
        }),
        {
          ...piModel("moonshotai/kimi-k3", {
            id: "kimi-k3",
            thinkingProfile: "kimi3",
          }),
          compat: { ...KIMI_K3_COMPAT },
        },
      ],
    },
    category: "cn_official",
    icon: "kimi",
    iconColor: "#6366F1",
  },
  {
    name: "Kimi For Coding",
    providerKey: "loongport-kimi-for-coding",
    websiteUrl: "https://www.kimi.com/code/",
    apiKeyUrl: "https://platform.kimi.com/console/api-keys",
    settingsConfig: {
      name: "Kimi For Coding",
      baseUrl: "https://api.kimi.com/coding",
      api: "anthropic-messages",
      apiKey: "",
      models: [
        piModel("moonshotai/kimi-k2.7-code", {
          id: "kimi-for-coding",
          name: "Kimi For Coding",
          maxTokens: 32768,
        }),
      ],
    },
    category: "cn_official",
    icon: "kimi",
    iconColor: "#6366F1",
  },
  {
    name: "PackyCode",
    providerKey: "loongport-packy-code",
    websiteUrl: "https://www.packyapi.ai",
    apiKeyUrl: "https://www.packyapi.ai/register",
    settingsConfig: {
      name: "PackyCode",
      baseUrl: "https://www.packyapi.ai",
      api: "anthropic-messages",
      apiKey: "",
      models: [
        piModel("anthropic/claude-sonnet-5", {
          id: "claude-sonnet-5",
        }),
        piModel("anthropic/claude-opus-5", {
          id: "claude-opus-5",
        }),
      ],
    },
    category: "third_party",
    icon: "packycode",
  },
  {
    name: "ZetaAPI",
    providerKey: "loongport-zeta-api",
    websiteUrl: "https://zetaapi.ai",
    apiKeyUrl: "https://zetaapi.ai/go/u117",
    settingsConfig: {
      name: "ZetaAPI",
      baseUrl: "https://api.zetaapi.ai/v1",
      api: "openai-completions",
      apiKey: "",
      models: [
        piModel("openai/gpt-5.6-sol", {
          id: "gpt-5.6-sol",
        }),
      ],
    },
    category: "aggregator",
    icon: "zetaapi",
  },
  {
    name: "APINebula",
    providerKey: "loongport-apinebula",
    websiteUrl: "https://apinebula.ai",
    apiKeyUrl: "https://apinebula.ai/VjM74M",
    settingsConfig: {
      name: "APINebula",
      baseUrl: "https://apinebula.ai/v1",
      api: "openai-completions",
      apiKey: "",
      models: [
        piModel("openai/gpt-5.6-sol", {
          id: "gpt-5.6-sol",
        }),
      ],
    },
    category: "third_party",
    icon: "apinebula",
  },
  {
    name: "AICodeMirror",
    providerKey: "loongport-aicode-mirror",
    websiteUrl: "https://www.aicodemirror.ai",
    apiKeyUrl: "https://www.aicodemirror.ai/register",
    settingsConfig: {
      name: "AICodeMirror",
      baseUrl: "https://api.aicodemirror.ai/api/claudecode",
      api: "anthropic-messages",
      apiKey: "",
      models: [
        piModel("anthropic/claude-sonnet-5", {
          id: "claude-sonnet-5",
        }),
        piModel("anthropic/claude-opus-5", {
          id: "claude-opus-5",
        }),
      ],
    },
    category: "third_party",
    icon: "aicodemirror",
    iconColor: "#000000",
  },
  {
    name: "FennoAI",
    providerKey: "loongport-fenno-ai",
    websiteUrl: "https://api.fenno.ai",
    apiKeyUrl:
      "https://api.fenno.ai/register?redirect=/purchase?tab=subscription%26group=16",
    settingsConfig: {
      name: "FennoAI",
      baseUrl: "https://api.fenno.ai/v1",
      api: "openai-completions",
      apiKey: "",
      models: [
        piModel("openai/gpt-5.6-sol", {
          id: "gpt-5.6-sol",
        }),
      ],
    },
    category: "aggregator",
    icon: "fenno",
  },
  {
    name: "RunAPI",
    providerKey: "loongport-run-api",
    websiteUrl: "https://runapi.co",
    apiKeyUrl: "https://runapi.co/register",
    settingsConfig: {
      name: "RunAPI",
      baseUrl: "https://runapi.co",
      api: "anthropic-messages",
      apiKey: "",
      models: [
        piModel("anthropic/claude-sonnet-5", {
          id: "claude-sonnet-5",
        }),
        piModel("anthropic/claude-opus-5", {
          id: "claude-opus-5",
        }),
        piModel("anthropic/claude-haiku-4.5", {
          id: "claude-haiku-4-5",
        }),
      ],
    },
    category: "aggregator",
    icon: "runapi",
  },
  {
    name: "Shengsuanyun",
    nameKey: "providerForm.presets.shengsuanyun",
    providerKey: "loongport-shengsuanyun",
    websiteUrl: "https://www.shengsuanyun.com/",
    apiKeyUrl: "https://www.shengsuanyun.com/",
    settingsConfig: {
      name: "Shengsuanyun",
      baseUrl: "https://router.shengsuanyun.com/api",
      api: "anthropic-messages",
      apiKey: "",
      models: [
        piModel("anthropic/claude-opus-5", {
          id: "anthropic/claude-opus-5",
        }),
        piModel("anthropic/claude-sonnet-5", {
          id: "anthropic/claude-sonnet-5",
        }),
      ],
    },
    category: "aggregator",
    icon: "shengsuanyun",
  },
  {
    name: "AIGoCode",
    providerKey: "loongport-aigo-code",
    websiteUrl: "https://aigocode.app",
    apiKeyUrl: "https://aigocode.app",
    settingsConfig: {
      name: "AIGoCode",
      baseUrl: "https://api.aigocode.app",
      api: "anthropic-messages",
      apiKey: "",
      models: [
        piModel("anthropic/claude-sonnet-5", {
          id: "claude-sonnet-5",
        }),
        piModel("anthropic/claude-opus-5", {
          id: "claude-opus-5",
        }),
      ],
    },
    category: "third_party",
    icon: "aigocode",
    iconColor: "#5B7FFF",
  },
  {
    name: "Qiniu",
    nameKey: "providerForm.presets.qiniu",
    providerKey: "loongport-qiniu",
    websiteUrl: "https://s.qiniu.com/nMvAvy",
    apiKeyUrl: "https://s.qiniu.com/nMvAvy",
    settingsConfig: {
      name: "Qiniu",
      baseUrl: "https://api.qnaigc.com/v1",
      api: "openai-completions",
      apiKey: "",
      models: [
        piModel("openai/gpt-5.6-sol", {
          id: "gpt-5.6-sol",
        }),
      ],
    },
    category: "aggregator",
    icon: "qiniu",
  },
  {
    name: "AICoding",
    providerKey: "loongport-aicoding",
    websiteUrl: "https://aicoding.inc",
    apiKeyUrl: "https://aicoding.inc",
    settingsConfig: {
      name: "AICoding",
      baseUrl: "https://api.aicoding.inc",
      api: "anthropic-messages",
      apiKey: "",
      models: [
        piModel("anthropic/claude-sonnet-5", {
          id: "claude-sonnet-5",
        }),
        piModel("anthropic/claude-opus-5", {
          id: "claude-opus-5",
        }),
      ],
    },
    category: "third_party",
    icon: "aicoding",
    iconColor: "#000000",
  },
  {
    name: "SubRouter",
    providerKey: "loongport-sub-router",
    websiteUrl: "https://subrouter.ai",
    apiKeyUrl: "https://subrouter.ai/register",
    settingsConfig: {
      name: "SubRouter",
      baseUrl: "https://subrouter.ai/v1",
      api: "openai-completions",
      apiKey: "",
      models: [
        piModel("openai/gpt-5.6-sol", {
          id: "gpt-5.6-sol",
        }),
      ],
    },
    category: "aggregator",
    icon: "subrouter",
  },
  {
    name: "APIKEY.FUN",
    providerKey: "loongport-apikey-fun",
    websiteUrl: "https://apikey.fan",
    apiKeyUrl: "https://apikey.fan/register",
    settingsConfig: {
      name: "APIKEY.FUN",
      baseUrl: "https://api.apikey.fan",
      api: "anthropic-messages",
      apiKey: "",
      models: [
        piModel("anthropic/claude-opus-5", {
          id: "claude-opus-5",
        }),
        piModel("anthropic/claude-sonnet-5", {
          id: "claude-sonnet-5",
        }),
        piModel("anthropic/claude-haiku-4.5", {
          id: "claude-haiku-4-5",
        }),
      ],
    },
    category: "third_party",
    icon: "apikeyfun",
  },
  {
    name: "Code0",
    providerKey: "loongport-code0",
    websiteUrl: "https://code0.ai",
    apiKeyUrl: "https://code0.ai/agent/register/B2XHxGjGmRvqgznY",
    settingsConfig: {
      name: "Code0",
      baseUrl: "https://code0.ai/v1",
      api: "openai-completions",
      apiKey: "",
      models: [
        piModel("openai/gpt-5.6-sol", {
          id: "gpt-5.6-sol",
        }),
      ],
    },
    category: "aggregator",
    icon: "code0",
  },
  {
    name: "TeamoRouter",
    providerKey: "loongport-teamo-router",
    websiteUrl: "https://teamorouter.com",
    apiKeyUrl: "https://teamorouter.com/",
    settingsConfig: {
      name: "TeamoRouter",
      baseUrl: "https://api.teamorouter.com/v1",
      api: "openai-completions",
      apiKey: "",
      models: [
        piModel("openai/gpt-5.6-sol", {
          id: "gpt-5.6-sol",
        }),
      ],
    },
    category: "aggregator",
    icon: "teamorouter",
  },
  {
    name: "ClaudeCN",
    providerKey: "loongport-claude-cn",
    websiteUrl: "https://claudecn.top",
    apiKeyUrl: "https://claudecn.ai/register",
    settingsConfig: {
      name: "ClaudeCN",
      baseUrl: "https://claudecn.top",
      api: "anthropic-messages",
      apiKey: "",
      models: [
        piModel("anthropic/claude-sonnet-5", {
          id: "claude-sonnet-5",
        }),
        piModel("anthropic/claude-opus-5", {
          id: "claude-opus-5",
        }),
        piModel("anthropic/claude-haiku-4.5", {
          id: "claude-haiku-4-5",
        }),
      ],
    },
    category: "third_party",
    icon: "claudecn",
  },
  {
    name: "火山Agentplan",
    providerKey: "loongport-agentplan",
    websiteUrl: "https://www.volcengine.com/activity/codingplan",
    apiKeyUrl: "https://www.volcengine.com/activity/codingplan",
    settingsConfig: {
      name: "火山Agentplan",
      baseUrl: "https://ark.cn-beijing.volces.com/api/coding/v3",
      api: "openai-completions",
      apiKey: "",
      models: [
        piModel("volcengine/ark-code-latest", {
          id: "ark-code-latest",
        }),
      ],
    },
    category: "cn_official",
    icon: "huoshan",
    iconColor: "#3370FF",
  },
  {
    name: "BytePlus",
    providerKey: "loongport-byte-plus",
    websiteUrl: "https://www.byteplus.com/en/product/modelark",
    apiKeyUrl: "https://www.byteplus.com/en/product/modelark",
    settingsConfig: {
      name: "BytePlus",
      baseUrl: "https://ark.ap-southeast.bytepluses.com/api/coding/v3",
      api: "openai-completions",
      apiKey: "",
      models: [
        piModel("volcengine/ark-code-latest", {
          id: "ark-code-latest",
        }),
      ],
    },
    category: "cn_official",
    icon: "byteplus",
    iconColor: "#3370FF",
  },
  {
    name: "DouBaoSeed",
    providerKey: "loongport-dou-bao-seed",
    websiteUrl:
      "https://console.volcengine.com/ark/region:ark+cn-beijing/apiKey?apikey=%7B%7D",
    apiKeyUrl:
      "https://console.volcengine.com/ark/region:ark+cn-beijing/apiKey?apikey=%7B%7D",
    settingsConfig: {
      name: "DouBaoSeed",
      baseUrl: "https://ark.cn-beijing.volces.com/api/v3",
      api: "openai-completions",
      apiKey: "",
      models: [
        piModel("volcengine/doubao-seed-2.1-pro", {
          id: "doubao-seed-2-1-pro-260628",
        }),
      ],
    },
    category: "cn_official",
    icon: "doubao",
    iconColor: "#3370FF",
  },
  {
    name: "A6API",
    providerKey: "loongport-a6-api",
    websiteUrl: "https://www.a6api.com",
    apiKeyUrl: "https://a6api.com/register",
    settingsConfig: {
      name: "A6API",
      baseUrl: "https://api.a6api.com/v1",
      api: "openai-completions",
      apiKey: "",
      models: [
        piModel("openai/gpt-5.6-sol", {
          id: "gpt-5.6-sol",
        }),
      ],
    },
    category: "aggregator",
    icon: "a6api",
  },
  {
    name: "AtlasCloud",
    providerKey: "loongport-atlas-cloud",
    websiteUrl: "https://www.atlascloud.ai/console/coding-plan",
    apiKeyUrl: "https://www.atlascloud.ai/console/coding-plan",
    settingsConfig: {
      name: "AtlasCloud",
      baseUrl: "https://api.atlascloud.ai/v1",
      api: "openai-completions",
      apiKey: "",
      models: [
        piModel("zai/glm-5.1", {
          id: "zai-org/glm-5.1",
          name: "GLM 5.1",
        }),
      ],
    },
    category: "aggregator",
    icon: "atlascloud",
  },
  {
    name: "CCSub",
    providerKey: "loongport-ccsub",
    websiteUrl: "https://www.ccsub.net",
    apiKeyUrl: "https://www.ccsub.net/register",
    settingsConfig: {
      name: "CCSub",
      baseUrl: "https://www.ccsub.net/v1",
      api: "openai-completions",
      apiKey: "",
      models: [
        piModel("openai/gpt-5.6-sol", {
          id: "gpt-5.6-sol",
        }),
      ],
    },
    category: "aggregator",
    icon: "ccsub",
  },
  {
    name: "SSSAiCode",
    providerKey: "loongport-sssai-code",
    websiteUrl: "https://sssaicodeapi.com",
    apiKeyUrl: "https://sssaicodeapi.com/register",
    settingsConfig: {
      name: "SSSAiCode",
      baseUrl: "https://node-hk.sssaicodeapi.com/api",
      api: "anthropic-messages",
      apiKey: "",
      models: [
        piModel("anthropic/claude-sonnet-5", {
          id: "claude-sonnet-5",
        }),
        piModel("anthropic/claude-opus-5", {
          id: "claude-opus-5",
        }),
      ],
    },
    category: "third_party",
    icon: "sssaicode",
    iconColor: "#000000",
  },
  {
    name: "Micu",
    providerKey: "loongport-micu",
    websiteUrl: "https://www.micuapi.ai",
    apiKeyUrl: "https://www.micuapi.ai/register",
    settingsConfig: {
      name: "Micu",
      baseUrl: "https://www.micuapi.ai",
      api: "anthropic-messages",
      apiKey: "",
      models: [
        piModel("anthropic/claude-opus-5", {
          id: "claude-opus-5",
        }),
        piModel("anthropic/claude-sonnet-5", {
          id: "claude-sonnet-5",
        }),
      ],
    },
    category: "third_party",
    icon: "micu",
    iconColor: "#000000",
  },
  {
    name: "RightCode",
    providerKey: "loongport-right-code",
    websiteUrl: "https://www.rightapi.ai",
    apiKeyUrl: "https://www.rightapi.ai/register",
    settingsConfig: {
      name: "RightCode",
      baseUrl: "https://www.rightapi.ai/codex/v1",
      api: "openai-responses",
      apiKey: "",
      models: [
        piModel("openai/gpt-5.6-sol", {
          id: "gpt-5.6-sol",
        }),
      ],
    },
    category: "third_party",
    icon: "rc",
    iconColor: "#E96B2C",
  },
  {
    name: "ETok.ai",
    providerKey: "loongport-etok-ai",
    websiteUrl: "https://etok.ai",
    apiKeyUrl: "https://etok.ai",
    settingsConfig: {
      name: "ETok",
      baseUrl: "https://api.etok.ai",
      api: "anthropic-messages",
      apiKey: "",
      models: [
        piModel("anthropic/claude-opus-5", {
          id: "claude-opus-5",
        }),
        piModel("anthropic/claude-sonnet-5", {
          id: "claude-sonnet-5",
        }),
      ],
    },
    category: "third_party",
    icon: "etok",
    iconColor: "#000000",
  },
  {
    name: "Cubence",
    providerKey: "loongport-cubence",
    websiteUrl: "https://cubence.com",
    apiKeyUrl: "https://cubence.com/signup",
    settingsConfig: {
      name: "Cubence",
      baseUrl: "https://api.cubence.com",
      api: "anthropic-messages",
      apiKey: "",
      models: [
        piModel("anthropic/claude-sonnet-5", {
          id: "claude-sonnet-5",
        }),
        piModel("anthropic/claude-opus-5", {
          id: "claude-opus-5",
        }),
      ],
    },
    category: "third_party",
    icon: "cubence",
    iconColor: "#000000",
  },
  {
    name: "CrazyRouter",
    providerKey: "loongport-crazy-router",
    websiteUrl: "https://www.crazyrouter.com",
    apiKeyUrl: "https://www.crazyrouter.com/register",
    settingsConfig: {
      name: "CrazyRouter",
      baseUrl: "https://cn.crazyrouter.com",
      api: "anthropic-messages",
      apiKey: "",
      models: [
        piModel("anthropic/claude-sonnet-5", {
          id: "claude-sonnet-5",
        }),
        piModel("anthropic/claude-opus-5", {
          id: "claude-opus-5",
        }),
      ],
    },
    category: "third_party",
    icon: "crazyrouter",
    iconColor: "#000000",
  },
  {
    name: "DMXAPI",
    providerKey: "loongport-dmxapi",
    websiteUrl: "https://www.dmxapi.cn",
    apiKeyUrl: "https://www.dmxapi.cn",
    settingsConfig: {
      name: "DMXAPI",
      baseUrl: "https://www.dmxapi.cn",
      api: "anthropic-messages",
      apiKey: "",
      models: [
        piModel("anthropic/claude-sonnet-5", {
          id: "claude-sonnet-5",
        }),
        piModel("anthropic/claude-opus-5", {
          id: "claude-opus-5",
        }),
      ],
    },
    category: "aggregator",
  },
  {
    name: "SudoCode.chat",
    providerKey: "loongport-sudo-code-chat",
    websiteUrl: "https://sudocode.chat",
    apiKeyUrl: "https://sudocode.chat/sign-up",
    settingsConfig: {
      name: "SudoCode.chat",
      baseUrl: "https://api.sudocode.chat/v1",
      api: "openai-responses",
      apiKey: "",
      models: [
        piModel("openai/gpt-5.6-sol", {
          id: "gpt-5.6-sol",
        }),
      ],
    },
    category: "third_party",
    icon: "sudocode",
  },
  {
    name: "SudoCode.us",
    providerKey: "loongport-sudo-code-us",
    websiteUrl: "https://sudocode.us",
    apiKeyUrl: "https://sudocode.us",
    settingsConfig: {
      name: "SudoCode.us",
      baseUrl: "https://sudocode.us/v1",
      api: "openai-responses",
      apiKey: "",
      models: [
        piModel("openai/gpt-5.6-sol", {
          id: "gpt-5.6-sol",
        }),
      ],
    },
    category: "third_party",
    icon: "sudocode-us",
  },
  {
    name: "Amux",
    providerKey: "loongport-amux",
    websiteUrl: "https://amux.ai",
    apiKeyUrl: "https://amux.ai",
    settingsConfig: {
      name: "Amux",
      baseUrl: "https://api.amux.ai/v1",
      api: "openai-completions",
      apiKey: "",
      models: [
        piModel("openai/gpt-5.6-sol", {
          id: "gpt-5.6-sol",
        }),
      ],
    },
    category: "aggregator",
    icon: "amux",
  },
  {
    name: "DeepSeek",
    providerKey: "loongport-deep-seek",
    websiteUrl: "https://platform.deepseek.com",
    apiKeyUrl: "https://platform.deepseek.com/api_keys",
    settingsConfig: {
      name: "DeepSeek",
      baseUrl: "https://api.deepseek.com/v1",
      api: "openai-completions",
      apiKey: "",
      models: [
        piModel("deepseek/deepseek-v4-pro", {
          id: "deepseek-v4-pro",
          thinkingProfile: "deepseekV4",
        }),
        piModel("deepseek/deepseek-v4-flash", {
          id: "deepseek-v4-flash",
          thinkingProfile: "deepseekV4",
        }),
      ],
    },
    category: "cn_official",
    icon: "deepseek",
    iconColor: "#1E88E5",
  },
  {
    name: "Zhipu GLM",
    providerKey: "loongport-zhipu-glm",
    websiteUrl: "https://open.bigmodel.cn",
    apiKeyUrl: "https://www.bigmodel.cn/claude-code",
    settingsConfig: {
      name: "Zhipu GLM",
      baseUrl: "https://open.bigmodel.cn/api/coding/paas/v4",
      api: "openai-completions",
      apiKey: "",
      models: [
        piModel("zai/glm-5.1", {
          id: "glm-5.1",
        }),
      ],
    },
    category: "cn_official",
    icon: "zhipu",
    iconColor: "#0F62FE",
  },
  {
    name: "Zhipu GLM en",
    providerKey: "loongport-zhipu-glm-en",
    websiteUrl: "https://z.ai",
    apiKeyUrl: "https://z.ai/subscribe",
    settingsConfig: {
      name: "Zhipu GLM en",
      baseUrl: "https://api.z.ai/api/coding/paas/v4",
      api: "openai-completions",
      apiKey: "",
      models: [
        piModel("zai/glm-5.1", {
          id: "glm-5.1",
        }),
      ],
    },
    category: "cn_official",
    icon: "zhipu",
    iconColor: "#0F62FE",
  },
  {
    name: "Bailian",
    providerKey: "loongport-bailian",
    websiteUrl: "https://bailian.console.aliyun.com",
    apiKeyUrl: "https://bailian.console.aliyun.com/#/api-key",
    settingsConfig: {
      name: "Bailian",
      baseUrl: "https://dashscope.aliyuncs.com/compatible-mode/v1",
      api: "openai-completions",
      apiKey: "",
      models: [
        piModel("qwen/qwen3-coder-plus", {
          id: "qwen3-coder-plus",
        }),
      ],
    },
    category: "cn_official",
    icon: "bailian",
    iconColor: "#624AFF",
  },
  {
    name: "StepFun",
    providerKey: "loongport-step-fun",
    websiteUrl: "https://platform.stepfun.com/step-plan",
    apiKeyUrl: "https://platform.stepfun.com/interface-key",
    settingsConfig: {
      name: "StepFun",
      baseUrl: "https://api.stepfun.com/step_plan/v1",
      api: "openai-completions",
      apiKey: "",
      models: [
        piModel("stepfun/step-3.5-flash", {
          id: "step-3.5-flash-2603",
          name: "Step 3.5 Flash 2603",
        }),
        piModel("stepfun/step-3.5-flash", {
          id: "step-3.5-flash",
        }),
      ],
    },
    category: "cn_official",
    icon: "stepfun",
    iconColor: "#16D6D2",
  },
  {
    name: "StepFun en",
    providerKey: "loongport-step-fun-en",
    websiteUrl: "https://platform.stepfun.ai/step-plan",
    apiKeyUrl: "https://platform.stepfun.ai/interface-key",
    settingsConfig: {
      name: "StepFun en",
      baseUrl: "https://api.stepfun.ai/step_plan/v1",
      api: "openai-completions",
      apiKey: "",
      models: [
        piModel("stepfun/step-3.5-flash", {
          id: "step-3.5-flash-2603",
          name: "Step 3.5 Flash 2603",
        }),
        piModel("stepfun/step-3.5-flash", {
          id: "step-3.5-flash",
        }),
      ],
    },
    category: "cn_official",
    icon: "stepfun",
    iconColor: "#16D6D2",
  },
  {
    name: "StepFun Step Plan",
    providerKey: "loongport-step-fun-step-plan",
    websiteUrl: "https://platform.stepfun.com/docs/zh/step-plan/overview",
    apiKeyUrl: "https://platform.stepfun.com/interface-key",
    settingsConfig: {
      name: "StepFun Step Plan",
      baseUrl: "https://api.stepfun.com/step_plan/v1",
      api: "openai-completions",
      apiKey: "",
      models: [
        piModel("stepfun/step-3.5-flash", {
          id: "step-3.5-flash",
        }),
      ],
    },
    category: "cn_official",
    icon: "stepfun",
    iconColor: "#005AFF",
  },
  {
    name: "ModelScope",
    providerKey: "loongport-model-scope",
    websiteUrl: "https://modelscope.cn",
    apiKeyUrl: "https://modelscope.cn/my/myaccesstoken",
    settingsConfig: {
      name: "ModelScope",
      baseUrl: "https://api-inference.modelscope.cn/v1",
      api: "openai-completions",
      apiKey: "",
      models: [
        piModel("zai/glm-5.2", {
          id: "ZhipuAI/GLM-5.2",
        }),
      ],
    },
    category: "aggregator",
    icon: "modelscope",
    iconColor: "#624AFF",
  },
  {
    name: "KAT-Coder",
    providerKey: "loongport-kat-coder",
    websiteUrl: "https://console.streamlake.ai",
    apiKeyUrl: "https://console.streamlake.ai/console/api-key",
    settingsConfig: {
      name: "KAT-Coder",
      baseUrl:
        "https://vanchin.streamlake.ai/api/gateway/v1/endpoints/${ENDPOINT_ID}/openai",
      api: "openai-completions",
      apiKey: "",
      models: [
        piModel("streamlake/kat-coder-pro", {
          id: "KAT-Coder-Pro",
        }),
      ],
    },
    category: "cn_official",
    icon: "catcoder",
  },
  {
    name: "Longcat",
    providerKey: "loongport-longcat",
    websiteUrl: "https://longcat.chat/platform",
    apiKeyUrl: "https://longcat.chat/platform/api_keys",
    settingsConfig: {
      name: "Longcat",
      baseUrl: "https://api.longcat.chat/openai/v1",
      api: "openai-completions",
      apiKey: "",
      models: [
        piModel("longcat/longcat-2.0", {
          id: "LongCat-2.0",
        }),
      ],
    },
    category: "cn_official",
    icon: "longcat",
    iconColor: "#29E154",
  },
  {
    name: "MiniMax",
    providerKey: "loongport-mini-max",
    websiteUrl: "https://platform.minimaxi.com",
    apiKeyUrl: "https://platform.minimaxi.com/subscribe/coding-plan",
    settingsConfig: {
      name: "MiniMax",
      baseUrl: "https://api.minimaxi.com/v1",
      api: "openai-completions",
      apiKey: "",
      models: [
        piModel("minimax/minimax-m2.7", {
          id: "MiniMax-M2.7",
        }),
      ],
    },
    category: "cn_official",
    theme: {
      backgroundColor: "#f64551",
      textColor: "#FFFFFF",
    },
    icon: "minimax",
    iconColor: "#FF6B6B",
  },
  {
    name: "MiniMax en",
    providerKey: "loongport-mini-max-en",
    websiteUrl: "https://platform.minimax.io",
    apiKeyUrl: "https://platform.minimax.io/subscribe/coding-plan",
    settingsConfig: {
      name: "MiniMax en",
      baseUrl: "https://api.minimax.io/v1",
      api: "openai-completions",
      apiKey: "",
      models: [
        piModel("minimax/minimax-m2.7", {
          id: "MiniMax-M2.7",
        }),
      ],
    },
    category: "cn_official",
    theme: {
      backgroundColor: "#f64551",
      textColor: "#FFFFFF",
    },
    icon: "minimax",
    iconColor: "#FF6B6B",
  },
  {
    name: "BaiLing",
    providerKey: "loongport-bai-ling",
    websiteUrl: "https://alipaytbox.yuque.com/sxs0ba/ling/get_started",
    settingsConfig: {
      name: "BaiLing",
      baseUrl: "https://api.tbox.cn/v1",
      api: "openai-completions",
      apiKey: "",
      models: [
        piModel("inclusionai/ling-2.5-1t", {
          id: "Ling-2.5-1T",
        }),
      ],
    },
    category: "cn_official",
  },
  {
    name: "Xiaomi MiMo",
    providerKey: "loongport-xiaomi-mi-mo",
    websiteUrl: "https://platform.xiaomimimo.com",
    apiKeyUrl: "https://platform.xiaomimimo.com/#/console/api-keys",
    settingsConfig: {
      name: "Xiaomi MiMo",
      baseUrl: "https://api.xiaomimimo.com/v1",
      api: "openai-completions",
      apiKey: "",
      models: [
        {
          ...piModel("xiaomi/mimo-v2.5-pro", {
            id: "mimo-v2.5-pro",
          }),
          compat: { ...XIAOMI_THINKING_COMPAT },
        },
        {
          ...piModel("xiaomi/mimo-v2.5", {
            id: "mimo-v2.5",
          }),
          compat: { ...XIAOMI_THINKING_COMPAT },
        },
      ],
    },
    category: "cn_official",
    icon: "xiaomimimo",
    iconColor: "#000000",
  },
  {
    name: "Xiaomi MiMo Token Plan (China)",
    providerKey: "loongport-xiaomi-mi-mo-token-plan-china",
    websiteUrl: "https://platform.xiaomimimo.com/#/token-plan",
    apiKeyUrl: "https://platform.xiaomimimo.com/#/console/plan-manage",
    settingsConfig: {
      name: "Xiaomi MiMo Token Plan (China)",
      baseUrl: "https://token-plan-cn.xiaomimimo.com/v1",
      api: "openai-completions",
      apiKey: "",
      models: [
        piModel("xiaomi/mimo-v2.5-pro", {
          id: "mimo-v2.5-pro",
        }),
        piModel("xiaomi/mimo-v2.5", {
          id: "mimo-v2.5",
        }),
      ],
    },
    category: "cn_official",
    icon: "xiaomimimo",
    iconColor: "#000000",
  },
  {
    name: "OpenCode Go",
    providerKey: "loongport-open-code-go",
    websiteUrl: "https://opencode.ai/go",
    apiKeyUrl: "https://opencode.ai",
    settingsConfig: {
      name: "OpenCode Go",
      baseUrl: "https://opencode.ai/zen/go/v1",
      api: "openai-completions",
      apiKey: "",
      models: [
        {
          ...piModel("zai/glm-5.2", {
            id: "glm-5.2",
            name: "GLM 5.2",
            thinkingProfile: "openCodeGoGlm52",
          }),
          compat: { ...OPENAI_COMPLETIONS_COMPAT },
        },
        {
          ...piModel("moonshotai/kimi-k2.7-code", {
            id: "kimi-k2.7-code",
          }),
          compat: { ...OPENAI_COMPLETIONS_COMPAT },
        },
        {
          ...piModel("deepseek/deepseek-v4-pro", {
            id: "deepseek-v4-pro",
            thinkingProfile: "deepseekV4",
          }),
          compat: { ...DEEPSEEK_THINKING_COMPAT },
        },
        {
          ...piModel("deepseek/deepseek-v4-flash", {
            id: "deepseek-v4-flash",
            thinkingProfile: "deepseekV4",
          }),
          compat: { ...DEEPSEEK_THINKING_COMPAT },
        },
        {
          ...piModel("xiaomi/mimo-v2.5-pro", {
            id: "mimo-v2.5-pro",
          }),
          compat: { ...OPENAI_COMPLETIONS_COMPAT },
        },
      ],
    },
    category: "third_party",
    icon: "opencode",
    iconColor: "#211E1E",
  },
  {
    name: "AiHubMix",
    providerKey: "loongport-ai-hub-mix",
    websiteUrl: "https://aihubmix.com",
    apiKeyUrl: "https://aihubmix.com",
    settingsConfig: {
      name: "AiHubMix",
      baseUrl: "https://aihubmix.com",
      api: "anthropic-messages",
      apiKey: "",
      models: [
        piModel("anthropic/claude-sonnet-5", {
          id: "claude-sonnet-5",
        }),
        piModel("anthropic/claude-opus-5", {
          id: "claude-opus-5",
        }),
      ],
    },
    category: "aggregator",
    icon: "aihubmix",
    iconColor: "#006FFB",
  },
  {
    name: "CherryIN",
    providerKey: "loongport-cherry-in",
    websiteUrl: "https://open.cherryin.ai",
    apiKeyUrl: "https://open.cherryin.ai/console/token",
    settingsConfig: {
      name: "CherryIN",
      baseUrl: "https://open.cherryin.net",
      api: "anthropic-messages",
      apiKey: "",
      models: [
        piModel("anthropic/claude-sonnet-5", {
          id: "anthropic/claude-sonnet-5",
        }),
        piModel("anthropic/claude-opus-5", {
          id: "anthropic/claude-opus-5",
        }),
      ],
    },
    category: "aggregator",
    icon: "cherryin",
  },
  {
    name: "OpenRouter",
    providerKey: "loongport-open-router",
    websiteUrl: "https://openrouter.ai",
    apiKeyUrl: "https://openrouter.ai/keys",
    settingsConfig: {
      name: "OpenRouter",
      baseUrl: "https://openrouter.ai/api",
      api: "anthropic-messages",
      apiKey: "",
      models: [
        piModel("anthropic/claude-sonnet-5", {
          id: "anthropic/claude-sonnet-5",
        }),
        piModel("anthropic/claude-opus-5", {
          id: "anthropic/claude-opus-5",
        }),
      ],
    },
    category: "aggregator",
    icon: "openrouter",
    iconColor: "#6566F1",
  },
  {
    name: "TheRouter",
    providerKey: "loongport-the-router",
    websiteUrl: "https://therouter.ai",
    apiKeyUrl: "https://dashboard.therouter.ai",
    settingsConfig: {
      name: "TheRouter",
      baseUrl: "https://api.therouter.ai/v1",
      api: "openai-completions",
      apiKey: "",
      models: [
        piModel("anthropic/claude-sonnet-5", {
          id: "anthropic/claude-sonnet-5",
        }),
        piModel("openai/gpt-5.3-codex", {
          id: "openai/gpt-5.3-codex",
        }),
        piModel("openai/gpt-5.2", {
          id: "openai/gpt-5.2",
        }),
        piModel("google/gemini-3.6-flash", {
          id: "google/gemini-3.6-flash",
        }),
        piModel("qwen/qwen3-coder-480b", {
          id: "qwen/qwen3-coder-480b",
        }),
      ],
    },
    category: "aggregator",
  },
  {
    name: "Novita AI",
    providerKey: "loongport-novita-ai",
    websiteUrl: "https://novita.ai",
    apiKeyUrl: "https://novita.ai",
    settingsConfig: {
      name: "Novita AI",
      baseUrl: "https://api.novita.ai/openai",
      api: "openai-completions",
      apiKey: "",
      models: [
        piModel("zai/glm-5.1", {
          id: "zai-org/glm-5.1",
        }),
      ],
    },
    category: "aggregator",
    icon: "novita",
    iconColor: "#000000",
  },
  {
    name: "Nvidia",
    providerKey: "loongport-nvidia",
    websiteUrl: "https://build.nvidia.com",
    apiKeyUrl: "https://build.nvidia.com/settings/api-keys",
    settingsConfig: {
      name: "Nvidia",
      baseUrl: "https://integrate.api.nvidia.com/v1",
      api: "openai-completions",
      apiKey: "",
      models: [
        piModel("moonshotai/kimi-k2.5", {
          id: "moonshotai/kimi-k2.5",
        }),
      ],
    },
    category: "aggregator",
    icon: "nvidia",
    iconColor: "#000000",
  },
  {
    name: "PIPELLM",
    providerKey: "loongport-pipellm",
    websiteUrl: "https://code.pipellm.ai",
    apiKeyUrl: "https://code.pipellm.ai/login",
    settingsConfig: {
      name: "PIPELLM",
      baseUrl: "https://cc-api.pipellm.ai",
      api: "anthropic-messages",
      apiKey: "",
      models: [
        piModel("anthropic/claude-opus-5", {
          id: "claude-opus-5",
          name: "claude-opus-5",
        }),
        piModel("anthropic/claude-sonnet-5", {
          id: "claude-sonnet-5",
          name: "claude-sonnet-5",
        }),
        piModel("anthropic/claude-haiku-4.5-20251001", {
          id: "claude-haiku-4-5-20251001",
          name: "claude-haiku-4-5-20251001",
        }),
      ],
    },
    category: "aggregator",
    icon: "pipellm",
  },
  {
    name: "E-FlowCode",
    providerKey: "loongport-e-flow-code",
    websiteUrl: "https://e-flowcode.cc",
    apiKeyUrl: "https://e-flowcode.cc",
    settingsConfig: {
      name: "E-FlowCode",
      baseUrl: "https://e-flowcode.cc/v1",
      api: "openai-responses",
      apiKey: "",
      models: [
        piModel("openai/gpt-5.2-codex", {
          id: "gpt-5.2-codex",
          name: "gpt-5.2-codex",
        }),
        piModel("openai/gpt-5.3-codex", {
          id: "gpt-5.3-codex",
          name: "gpt-5.3-codex",
        }),
      ],
    },
    category: "third_party",
    icon: "eflowcode",
    iconColor: "#000000",
  },
  {
    name: "AWS Bedrock",
    providerKey: "loongport-aws-bedrock",
    websiteUrl: "https://aws.amazon.com/bedrock/",
    settingsConfig: {
      name: "AWS Bedrock",
      baseUrl: "https://bedrock-runtime.us-east-1.amazonaws.com",
      api: "bedrock-converse-stream",
      apiKey: "",
      models: [
        piModel("anthropic/claude-opus-5", {
          id: "global.anthropic.claude-opus-5",
          thinkingProfile: "xhighAndMax",
        }),
        piModel("anthropic/claude-sonnet-5", {
          id: "global.anthropic.claude-sonnet-5",
          thinkingProfile: "xhighAndMax",
        }),
        piModel("anthropic/claude-haiku-4.5-20251001", {
          id: "global.anthropic.claude-haiku-4-5-20251001-v1:0",
        }),
        piModel("amazon/nova-pro", {
          id: "us.amazon.nova-pro-v1:0",
        }),
        piModel("meta/llama-4-maverick", {
          id: "us.meta.llama4-maverick-17b-instruct-v1:0",
        }),
        piModel("deepseek/deepseek-r1", {
          id: "us.deepseek.r1-v1:0",
        }),
      ],
    },
    category: "cloud_provider",
    icon: "aws",
    iconColor: "#FF9900",
  },
];

function materializeVerifiedThinkingProfiles(
  preset: PiProviderPreset,
): PiProviderPreset {
  return {
    ...preset,
    settingsConfig: {
      ...preset.settingsConfig,
      models: preset.settingsConfig.models.map((model) => {
        const reference = getPiModelCatalogReference(model);
        if (!reference) return model;
        const resolved = reference.presetThinkingProfileId
          ? getPiThinkingProfile(reference.presetThinkingProfileId)
          : resolvePiThinkingProfile({
              catalogKey: reference.catalogKey,
              api: preset.settingsConfig.api,
            });
        return {
          ...model,
          ...(model.reasoning ? { thinkingLevelMap: resolved?.map ?? {} } : {}),
          ...(resolved?.modelCompat
            ? {
                compat: {
                  ...model.compat,
                  ...resolved.modelCompat,
                },
              }
            : {}),
        };
      }),
    },
  };
}

export const piProviderPresets = piProviderPresetDefinitions.map(
  materializeVerifiedThinkingProfiles,
);
