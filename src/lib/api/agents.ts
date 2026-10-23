import { invoke } from "@tauri-apps/api/core";

export const MAX_PARALLEL_ANALYSES = 8;

export interface AnalysisInput {
  input: string;
  model: string;
}
export interface AgentRun {
  taskId: string;
  agentId: string;
  dependsOnAnalysis?: boolean;
  result: {
    status: string;
    output?: string | null;
    reason?: string | null;
    outputUnavailable?: boolean;
  };
}
export interface AgentTaskGroup {
  taskId: string;
  status: string;
  canCancel: boolean;
  runs: AgentRun[];
}
export const agentsApi = {
  list: () => invoke<AgentTaskGroup[]>("list_agents_tasks", { grouped: true }),
  create: (
    requestId: string,
    input: string,
    model: string,
    analyses: AnalysisInput[],
  ) =>
    invoke<AgentTaskGroup[]>("create_agents_task", {
      requestId,
      input,
      model,
      analyses,
      grouped: true,
    }),
  cancel: (taskId: string) =>
    invoke<AgentTaskGroup[]>("cancel_agents_task", { taskId, grouped: true }),
};

export interface CodeCredential {
  id: string;
  allowedModels: string[];
}
export interface CodeTask {
  taskId: string;
  status: string;
  canCancel: boolean;
  processStopped: boolean;
  workspace?: string;
  baseCommit?: string;
  headCommit?: string;
  changeSummary?: string;
  output?: string;
  outputUnavailable?: boolean;
  error?: string;
}
export interface CodeTaskRequest {
  requestId: string;
  repository: string;
  baseRef: string;
  input: string;
  model: string;
  credentialId: string;
  routeApp: string;
}
export const codeAgentsApi = {
  access: () => invoke<CodeCredential[]>("code_agents_access"),
  authorize: () =>
    invoke<CodeCredential>("code_agents_access", { action: "authorize" }),
  revoke: (credentialId: string) =>
    invoke("code_agents_access", { action: "revoke", credentialId }),
  models: () => invoke<string[]>("code_agents_models"),
  list: () => invoke<CodeTask[]>("list_code_agents_tasks"),
  cancel: (cancelTaskId: string) =>
    invoke<CodeTask[]>("list_code_agents_tasks", { cancelTaskId }),
  create: (request: CodeTaskRequest) =>
    invoke<CodeTask>("create_code_agents_task", { request }),
};
