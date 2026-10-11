/**
 * Reusable-workflow permission guard.
 *
 * A job that calls a reusable workflow (`uses: ./.github/workflows/x.yml`) can
 * never receive more GITHUB_TOKEN permission than the CALLER job grants. When
 * the callee asks for more, GitHub fails the whole run before any job starts
 * (`startup_failure`, zero jobs, no useful log). This script detects that
 * statically by comparing each caller job's grant with the permissions every
 * job of the callee requests.
 *
 * Usage: bun scripts/check-workflow-permissions.ts [dir]   (default .github/workflows)
 * Exit code 1 on any escalation or on a load/parse error; warnings do not fail.
 */
import { readdirSync, readFileSync } from "node:fs";
import { basename, join } from "node:path";

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

const ALL_SCOPES = [
  "actions",
  "attestations",
  "checks",
  "contents",
  "deployments",
  "discussions",
  "id-token",
  "issues",
  "models",
  "packages",
  "pages",
  "pull-requests",
  "security-events",
  "statuses",
] as const;

const LOCAL_REUSABLE_PREFIX = "./.github/workflows/";

const RANK: Record<Level, number> = { none: 0, read: 1, write: 2 };

type Grant = Map<string, Level>;

function toLevel(value: unknown, scope: string): Level {
  if (value === "none" || value === "read" || value === "write") return value;
  throw new Error(
    `unrecognized permission level ${JSON.stringify(value)} for scope ${scope}`,
  );
}

/** Normalizes a `permissions` value. `undefined` means "not declared". */
function normalize(value: unknown): Grant | undefined {
  if (value === undefined || value === null) return undefined;
  if (value === "read-all") {
    return new Map<string, Level>(
      ALL_SCOPES.map((s): [string, Level] => [
        s,
        s === "id-token" ? "none" : "read",
      ]),
    );
  }
  if (value === "write-all") {
    return new Map<string, Level>(
      ALL_SCOPES.map((s): [string, Level] => [s, "write"]),
    );
  }
  if (typeof value === "object" && !Array.isArray(value)) {
    const map = new Map<string, Level>(
      ALL_SCOPES.map((s): [string, Level] => [s, "none"]),
    );
    for (const [scope, level] of Object.entries(value)) {
      map.set(scope, toLevel(level, scope));
    }
    return map;
  }
  throw new Error(`unrecognized permissions value: ${JSON.stringify(value)}`);
}

function jobsOf(wf: WorkflowFile): [string, Record<string, any>][] {
  const jobs = wf.doc.jobs;
  if (!jobs || typeof jobs !== "object") return [];
  return Object.entries(jobs).filter(
    (entry): entry is [string, Record<string, any>] =>
      typeof entry[1] === "object" && entry[1] !== null,
  );
}

export function findPermissionEscalations(
  workflows: WorkflowFile[],
): Finding[] {
  const byName = new Map(workflows.map((w) => [w.file, w]));
  const findings: Finding[] = [];

  for (const caller of workflows) {
    for (const [callerJob, job] of jobsOf(caller)) {
      const uses = job.uses;
      if (typeof uses !== "string" || !uses.startsWith(LOCAL_REUSABLE_PREFIX)) {
        continue;
      }
      const calleeFile = basename(uses);
      const callee = byName.get(calleeFile);
      if (!callee) {
        throw new Error(
          `${caller.file} job "${callerJob}" calls ${uses}, but ${calleeFile} was not found among the loaded workflows`,
        );
      }

      const grant = normalize(job.permissions ?? caller.doc.permissions);

      for (const [calleeJob, calleeJobDef] of jobsOf(callee)) {
        const requested =
          normalize(calleeJobDef.permissions ?? callee.doc.permissions) ??
          new Map<string, Level>();

        for (const [scope, level] of requested) {
          if (level === "none") continue;
          const base = {
            callerFile: caller.file,
            callerJob,
            calleeFile,
            calleeJob,
            scope,
            requested: level,
          };
          if (grant === undefined) {
            findings.push({
              kind: "unknown-grant",
              granted: "unspecified",
              ...base,
            });
            continue;
          }
          const granted = grant.get(scope) ?? "none";
          if (RANK[level] > RANK[granted]) {
            findings.push({ kind: "escalation", granted, ...base });
          }
        }
      }
    }
  }

  return findings.sort(
    (a, b) =>
      compare(a.callerFile, b.callerFile) ||
      compare(a.callerJob, b.callerJob) ||
      compare(a.calleeJob, b.calleeJob) ||
      compare(a.scope, b.scope),
  );
}

function compare(a: string, b: string): number {
  return a < b ? -1 : a > b ? 1 : 0;
}

export function loadWorkflows(dir: string): WorkflowFile[] {
  let names: string[];
  try {
    names = readdirSync(dir)
      .filter((name) => name.endsWith(".yml") || name.endsWith(".yaml"))
      .sort();
  } catch (error) {
    throw new Error(
      `cannot read workflow directory ${dir}: ${(error as Error).message}`,
    );
  }
  return names.map((file) => {
    const text = readFileSync(join(dir, file), "utf8");
    let doc: unknown;
    try {
      doc = Bun.YAML.parse(text);
    } catch (error) {
      throw new Error(`failed to parse ${file}: ${(error as Error).message}`);
    }
    return { file, doc: (doc ?? {}) as Record<string, any> };
  });
}

if (import.meta.main) {
  const dir = process.argv[2] ?? ".github/workflows";
  try {
    const workflows = loadWorkflows(dir);
    const findings = findPermissionEscalations(workflows);
    let errors = 0;
    let warnings = 0;
    for (const f of findings) {
      if (f.kind === "escalation") {
        errors += 1;
        console.log(
          `ERROR ${f.callerFile} job "${f.callerJob}" grants ${f.scope}: ${f.granted} but ${f.calleeFile} job "${f.calleeJob}" requests ${f.scope}: ${f.requested} (GitHub fails such runs with startup_failure; raise the caller's permission or lower the callee's)`,
        );
      } else {
        warnings += 1;
        console.log(
          `WARN ${f.callerFile} job "${f.callerJob}" declares no permissions; ${f.calleeFile} job "${f.calleeJob}" requests ${f.scope}: ${f.requested} (effective grant depends on repository defaults)`,
        );
      }
    }
    console.log(
      `workflow permissions: ${workflows.length} workflow(s) checked, ${errors} escalation(s), ${warnings} unknown grant warning(s)`,
    );
    if (errors > 0) process.exitCode = 1;
  } catch (error) {
    console.error(`ERROR ${(error as Error).message}`);
    process.exitCode = 1;
  }
}
