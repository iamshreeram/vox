// STUB (test-first scaffold): the real implementation replaces this file body.
export type Level = "none" | "read" | "write";

export interface WorkflowFile {
  /** File name, e.g. "build.yml". */
  file: string;
  /** Parsed YAML document. */
  doc: Record<string, any>;
}

export interface Finding {
  kind: "escalation" | "unknown-grant";
  callerFile: string;
  callerJob: string;
  calleeFile: string;
  calleeJob: string;
  scope: string;
  granted: Level | "unspecified";
  requested: Level;
}

export function findPermissionEscalations(_workflows: WorkflowFile[]): Finding[] {
  return [];
}

export function loadWorkflows(_dir: string): WorkflowFile[] {
  throw new Error("not implemented");
}
