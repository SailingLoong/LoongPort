# ZCode personal providers

Enable **ZCode** from the application bar's **+** menu (or application visibility settings). Add a provider with its API endpoint, API key, and model IDs, one per line. Supported protocols are Anthropic Messages, OpenAI Chat Completions, and OpenAI Responses.

The native ZCode personal configuration file remains authoritative. Providers created here can be edited or removed; other personal providers are displayed read-only. Account sign-in, the default model, proxying, MCP, and skills remain managed by ZCode. Model configuration in this first stage covers provider model IDs, not advanced model capability overrides.

Keys are saved only in the local ZCode configuration file and are not returned to the frontend or stored in LoongPort's provider database. When editing, a blank key preserves the existing key. Removing a provider asks for confirmation.

## Configuration location

Point LoongPort at the personal file ZCode is actually using. LoongPort chooses:

1. `ZCODE_PERSONAL_PROVIDER_CONFIG_FILE`: absolute path to the personal provider file
2. Otherwise, `ZCODE_DATA_BASE_DIR` (or the home directory), followed by `.zcode/v2/provider_config.json`

Overrides are read from the LoongPort process environment and do not change ZCode settings. If ZCode uses an application-selected data directory, set the matching override in LoongPort. Relative paths are rejected. When configuring ZCode directly, follow its runtime requirements: explicit provider-path overrides require both its built-in and personal paths.

The adapter supports native schema version 1. Unsupported versions, invalid JSON or rule structure, stale revisions, or lock contention fail without overwriting the file. After an external edit, cancel the editor, refresh, and reopen it. Unrelated provider rules, model overrides, ordering, and the default selection are preserved. Writes use the native directory-lock protocol and LoongPort's private atomic-file writer. Abandoned locks are left for ZCode to reclaim.

Contract references: [native file codec](https://github.com/zai-org/ZCode/blob/29628c9a/packages/provider-node/src/provider-config-file-codec.ts), [provider schema](https://github.com/zai-org/ZCode/blob/29628c9a/packages/provider/src/config/rule-data-schema.ts), and [file locking](https://github.com/zai-org/ZCode/blob/29628c9a/packages/shared/src/node/atomicFileLock.ts). Compatibility is checked by schema, not an application version allowlist.
