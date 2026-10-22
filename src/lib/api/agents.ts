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
