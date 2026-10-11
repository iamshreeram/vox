import assert from "node:assert/strict";
import { mkdtempSync, mkdirSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import {
  findPermissionEscalations,
  loadWorkflows,
  type Finding,
  type WorkflowFile,
} from "./check-workflow-permissions";

// Why this exists: a reusable workflow (`uses: ./.github/workflows/x.yml`)
// can never be granted more GITHUB_TOKEN permission than the CALLER job
// grants. If it asks for more, GitHub fails the run before any job starts
// (`startup_failure`, zero jobs). main-build.yml silently broke that way when
// its caller job was changed to `contents: read` while build.yml still asked
// for `contents: write`.

function wf(file: string, yaml: string): WorkflowFile {
  return { file, doc: Bun.YAML.parse(yaml) as Record<string, any> };
}

const escalations = (found: Finding[]) =>
  found.filter((f) => f.kind === "escalation");
const unknowns = (found: Finding[]) =>
  found.filter((f) => f.kind === "unknown-grant");

function caller(perms: string, workflowPerms = ""): WorkflowFile {
  return wf(
    "main.yml",
    `
name: main
on: push
${workflowPerms}
jobs:
  build:
${perms}
    strategy:
      matrix:
        include:
          - platform: linux
    uses: ./.github/workflows/build.yml
    with:
      platform: \${{ matrix.platform }}
    secrets: inherit
`,
  );
}

function callee(jobPerms: string, workflowPerms = ""): WorkflowFile {
  return wf(
    "build.yml",
    `
name: Build
on:
  workflow_call:
${workflowPerms}
jobs:
  build:
${jobPerms}
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v5
`,
  );
}

const P = (body: string) => `    permissions:\n${body}`;

const scenarios: Record<string, () => void> = {
  "the repo bug: caller contents:read vs callee contents:write is an escalation"() {
    const found = findPermissionEscalations([
      caller(P("      contents: read")),
      callee(P("      contents: write")),
    ]);
    assert.equal(escalations(found).length, 1);
    const f = escalations(found)[0];
    assert.equal(f.scope, "contents");
    assert.equal(f.granted, "read");
    assert.equal(f.requested, "write");
    assert.equal(f.callerFile, "main.yml");
    assert.equal(f.callerJob, "build");
    assert.equal(f.calleeFile, "build.yml");
    assert.equal(f.calleeJob, "build");
    assert.equal(unknowns(found).length, 0);
  },

  "equal grants are fine"() {
    assert.deepEqual(
      findPermissionEscalations([
        caller(P("      contents: write")),
        callee(P("      contents: write")),
      ]),
      [],
    );
  },

  "caller grants more than the callee needs"() {
    assert.deepEqual(
      findPermissionEscalations([
        caller(P("      contents: write")),
        callee(P("      contents: read")),
      ]),
      [],
    );
  },

  "a scope missing from an explicit caller map counts as none"() {
    const found = findPermissionEscalations([
      caller(P("      contents: write")),
      callee(P("      packages: read")),
    ]);
    assert.equal(escalations(found).length, 1);
    assert.equal(escalations(found)[0].scope, "packages");
    assert.equal(escalations(found)[0].granted, "none");
    assert.equal(escalations(found)[0].requested, "read");
  },

  "job-level `permissions: {}` grants nothing"() {
    const found = findPermissionEscalations([
      caller("    permissions: {}"),
      callee(P("      contents: read")),
    ]);
    assert.equal(escalations(found).length, 1);
    assert.equal(escalations(found)[0].granted, "none");
  },

  "workflow-level caller permissions apply when the job declares none"() {
    assert.deepEqual(
      findPermissionEscalations([
        caller("", "permissions:\n  contents: write"),
        callee(P("      contents: write")),
      ]),
      [],
    );
  },

  "job-level caller permissions override workflow-level ones"() {
    const found = findPermissionEscalations([
      caller(P("      contents: read"), "permissions:\n  contents: write"),
      callee(P("      contents: write")),
    ]);
    assert.equal(escalations(found).length, 1);
    assert.equal(escalations(found)[0].granted, "read");
  },

  "workflow-level read-all caller: write is an escalation, read is fine"() {
    const bad = findPermissionEscalations([
      caller("", "permissions: read-all"),
      callee(P("      contents: write")),
    ]);
    assert.equal(escalations(bad).length, 1);
    assert.equal(escalations(bad)[0].granted, "read");
    assert.deepEqual(
      findPermissionEscalations([
        caller("", "permissions: read-all"),
        callee(P("      contents: read")),
      ]),
      [],
    );
  },

  "write-all caller satisfies everything, including id-token"() {
    assert.deepEqual(
      findPermissionEscalations([
        caller("    permissions: write-all"),
        callee(
          P("      contents: write\n      id-token: write\n      packages: write"),
        ),
      ]),
      [],
    );
  },

  "id-token has no read level: read-all does not grant it"() {
    const found = findPermissionEscalations([
      caller("    permissions: read-all"),
      callee(P("      id-token: write")),
    ]);
    assert.equal(escalations(found).length, 1);
    assert.equal(escalations(found)[0].scope, "id-token");
    assert.equal(escalations(found)[0].granted, "none");
    assert.equal(escalations(found)[0].requested, "write");
  },

  "id-token: write granted explicitly is fine"() {
    assert.deepEqual(
      findPermissionEscalations([
        caller(P("      id-token: write")),
        callee(P("      id-token: write")),
      ]),
      [],
    );
  },

  "a scope the callee does not mention is never an escalation"() {
    assert.deepEqual(
      findPermissionEscalations([
        caller(P("      contents: write\n      packages: read")),
        callee(P("      packages: read")),
      ]),
      [],
    );
  },

  "a callee that declares no permissions requests nothing"() {
    assert.deepEqual(
      findPermissionEscalations([caller(P("      contents: read")), callee("")]),
      [],
    );
    assert.deepEqual(
      findPermissionEscalations([caller(""), callee("")]),
      [],
      "even with an unspecified caller",
    );
  },

  "a callee requesting only `none` is never a problem"() {
    assert.deepEqual(
      findPermissionEscalations([
        caller(""),
        callee(P("      contents: none")),
      ]),
      [],
    );
  },

  "unspecified caller grant is only a warning (kind unknown-grant)"() {
    const found = findPermissionEscalations([
      caller(""),
      callee(P("      contents: write")),
    ]);
    assert.equal(escalations(found).length, 0);
    assert.equal(unknowns(found).length, 1);
    assert.equal(unknowns(found)[0].granted, "unspecified");
    assert.equal(unknowns(found)[0].scope, "contents");
    assert.equal(unknowns(found)[0].requested, "write");
  },

  "callee workflow-level permissions apply when its job declares none"() {
    const found = findPermissionEscalations([
      caller(P("      contents: read")),
      callee("", "permissions:\n  contents: write"),
    ]);
    assert.equal(escalations(found).length, 1);
    assert.equal(escalations(found)[0].scope, "contents");
  },

  "callee job-level permissions override its workflow-level ones"() {
    assert.deepEqual(
      findPermissionEscalations([
        caller(P("      contents: read")),
        callee(P("      contents: read"), "permissions:\n  contents: write"),
      ]),
      [],
    );
  },

  "every job of the callee is checked"() {
    const twoJobs = wf(
      "build.yml",
      `
name: Build
on:
  workflow_call:
jobs:
  a:
    permissions:
      contents: read
    runs-on: ubuntu-latest
    steps: []
  b:
    permissions:
      contents: write
    runs-on: ubuntu-latest
    steps: []
`,
    );
    const found = findPermissionEscalations([
      caller(P("      contents: read")),
      twoJobs,
    ]);
    assert.equal(escalations(found).length, 1);
    assert.equal(escalations(found)[0].calleeJob, "b");
  },

  "non-local reusable workflows and step-level `uses` are ignored"() {
    const remote = wf(
      "remote.yml",
      `
name: remote
on: push
jobs:
  call:
    permissions:
      contents: read
    uses: octo/repo/.github/workflows/x.yml@v1
  other:
    permissions:
      contents: read
    runs-on: ubuntu-latest
    steps:
      - uses: ./.github/actions/local-action
`,
    );
    assert.deepEqual(findPermissionEscalations([remote]), []);
  },

  "a local reusable workflow that does not exist is a loud error"() {
    assert.throws(
      () => findPermissionEscalations([caller(P("      contents: write"))]),
      /build\.yml/,
    );
  },

  "workflows that call nothing produce no findings"() {
    assert.deepEqual(
      findPermissionEscalations([
        wf("plain.yml", "name: p\non: push\njobs:\n  j:\n    runs-on: x\n    steps: []\n"),
      ]),
      [],
    );
  },

  "the repository's own workflows have no permission escalations"() {
    const found = findPermissionEscalations(loadWorkflows(".github/workflows"));
    assert.deepEqual(
      escalations(found).map(
        (f) =>
          `${f.callerFile}:${f.callerJob} grants ${f.scope}:${f.granted} but ${f.calleeFile}:${f.calleeJob} needs ${f.scope}:${f.requested}`,
      ),
      [],
    );
  },
};

// ---- CLI behavior (exit codes) against throwaway workflow directories ------

function runCli(files: Record<string, string>) {
  const dir = mkdtempSync(join(tmpdir(), "wfperm-"));
  try {
    mkdirSync(dir, { recursive: true });
    for (const [name, body] of Object.entries(files)) {
      writeFileSync(join(dir, name), body);
    }
    const proc = Bun.spawnSync([
      "bun",
      "scripts/check-workflow-permissions.ts",
      dir,
    ]);
    return {
      code: proc.exitCode,
      out: `${proc.stdout.toString()}${proc.stderr.toString()}`,
    };
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
}

const callerYaml = (perms: string) => `name: main
on: push
jobs:
  build:
${perms}
    uses: ./.github/workflows/build.yml
`;
const calleeYaml = `name: Build
on:
  workflow_call:
jobs:
  build:
    permissions:
      contents: write
    runs-on: ubuntu-latest
    steps: []
`;

scenarios["CLI exits 1 and names the offending caller on an escalation"] =
  () => {
    const { code, out } = runCli({
      "main.yml": callerYaml("    permissions:\n      contents: read"),
      "build.yml": calleeYaml,
    });
    assert.equal(code, 1, out);
    assert.match(out, /main\.yml/);
    assert.match(out, /contents/);
  };

scenarios["CLI exits 0 when permissions line up"] = () => {
  const { code, out } = runCli({
    "main.yml": callerYaml("    permissions:\n      contents: write"),
    "build.yml": calleeYaml,
  });
  assert.equal(code, 0, out);
};

scenarios["CLI warns but exits 0 on an unspecified caller grant"] = () => {
  const { code, out } = runCli({
    "main.yml": callerYaml(""),
    "build.yml": calleeYaml,
  });
  assert.equal(code, 0, out);
  assert.match(out, /warn/i);
};

scenarios["CLI exits non-zero on a missing local callee instead of passing"] =
  () => {
    const { code } = runCli({
      "main.yml": callerYaml("    permissions:\n      contents: write"),
    });
    assert.notEqual(code, 0);
  };

// ---- runner -----------------------------------------------------------------

let failed = 0;
for (const [name, run] of Object.entries(scenarios)) {
  try {
    run();
  } catch (error) {
    failed += 1;
    console.error(`FAIL: ${name}\n  ${(error as Error).message}`);
  }
}
if (failed > 0) {
  throw new Error(
    `${failed} of ${Object.keys(scenarios).length} workflow-permission scenarios failed`,
  );
}
console.log(
  `workflow permissions: ${Object.keys(scenarios).length} scenarios passed`,
);
