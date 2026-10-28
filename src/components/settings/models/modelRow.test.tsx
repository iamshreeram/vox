import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import React from "react";
import { renderToStaticMarkup } from "react-dom/server";
import ModelRow, { type ModelRowLabels } from "./ModelRow";
import { rowActions, type RowStatus } from "./modelRowState";

// P2 spec (DESIGN.md Rev 5): ONE presentational row for every model-like thing (STT model,
// TTS engine/voice pack, later user-added and endpoint models). Pure: labels come in as props.
const read = (relative: string): string =>
  readFileSync(new URL(relative, import.meta.url), "utf8");

const labels: ModelRowLabels = {
  select: "Use",
  active: "In use",
  download: "Download",
  cancel: "Cancel",
  delete: "Delete",
  retry: "Retry",
  edit: "Edit",
  verifying: "Verifying",
  extracting: "Extracting",
  switching: "Switching",
  userAdded: "Added by you",
  progress: "Download progress",
};

const render = (
  props: Partial<React.ComponentProps<typeof ModelRow>> & {
    status: RowStatus;
  },
): string =>
  renderToStaticMarkup(
    React.createElement(ModelRow, {
      id: "m1",
      name: "Mock voice",
      description: "A test model",
      badges: ["Recommended", "120 MB"],
      labels,
      onSelect: () => {},
      onDownload: () => {},
      onCancel: () => {},
      onDelete: () => {},
      onRetry: () => {},
      ...props,
    }),
  );

const has = (html: string, testId: string): boolean =>
  html.includes(`data-testid="${testId}"`);

const scenarios: Record<string, () => void> = {
  "action matrix per status": () => {
    const all = (status: RowStatus, deletable = true) =>
      rowActions(status, { deletable });
    assert.deepEqual(all("notInstalled"), {
      select: false,
      download: true,
      cancel: false,
      delete: false,
      retry: false,
    });
    assert.deepEqual(all("downloading"), {
      select: false,
      download: false,
      cancel: true,
      delete: false,
      retry: false,
    });
    for (const busy of ["verifying", "extracting", "switching"] as const) {
      assert.deepEqual(all(busy), {
        select: false,
        download: false,
        cancel: false,
        delete: false,
        retry: false,
      });
    }
    assert.deepEqual(all("ready"), {
      select: true,
      download: false,
      cancel: false,
      delete: true,
      retry: false,
    });
    assert.deepEqual(all("ready", false), {
      select: true,
      download: false,
      cancel: false,
      delete: false,
      retry: false,
    });
    assert.deepEqual(all("active"), {
      select: false,
      download: false,
      cancel: false,
      delete: true,
      retry: false,
    });
    assert.deepEqual(all("active", false), {
      select: false,
      download: false,
      cancel: false,
      delete: false,
      retry: false,
    });
    assert.deepEqual(all("error"), {
      select: false,
      download: false,
      cancel: false,
      delete: true,
      retry: true,
    });
  },

  "every status renders one row carrying data-status": () => {
    for (const status of [
      "notInstalled",
      "downloading",
      "verifying",
      "extracting",
      "switching",
      "ready",
      "active",
      "error",
    ] as const) {
      const html = render({ status });
      assert.ok(has(html, "model-row"), `${status}: row testid`);
      assert.ok(
        html.includes(`data-status="${status}"`),
        `${status}: data-status`,
      );
      assert.ok(html.includes("Mock voice"), `${status}: name`);
      assert.ok(html.includes("A test model"), `${status}: description`);
      assert.ok(
        html.includes("Recommended") && html.includes("120 MB"),
        `${status}: badges`,
      );
    }
  },

  "buttons follow the action matrix in the markup": () => {
    const expectButtons = (status: RowStatus, present: string[]) => {
      const html = render({ status });
      for (const id of ["select", "download", "cancel", "delete", "retry"]) {
        assert.equal(
          has(html, `model-row-${id}`),
          present.includes(id),
          `${status}: model-row-${id}`,
        );
      }
    };
    expectButtons("notInstalled", ["download"]);
    expectButtons("downloading", ["cancel"]);
    expectButtons("verifying", []);
    expectButtons("ready", ["select", "delete"]);
    expectButtons("active", ["delete"]);
    expectButtons("error", ["retry", "delete"]);
  },

  "non-deletable rows never offer delete": () => {
    for (const status of ["ready", "active", "error"] as const) {
      assert.ok(!has(render({ status, deletable: false }), "model-row-delete"));
    }
  },

  "downloading shows an accessible progress bar and speed": () => {
    const html = render({
      status: "downloading",
      progress: 42,
      speedMbPerSecond: 3.5,
    });
    assert.match(html, /role="progressbar"/);
    assert.match(html, /aria-valuenow="42"/);
    assert.match(html, /aria-valuemin="0"/);
    assert.match(html, /aria-valuemax="100"/);
    assert.ok(html.includes("Download progress"), "progress label");
    assert.ok(html.includes("3.5"), "speed shown");
    const idle = render({ status: "ready" });
    assert.ok(
      !idle.includes('role="progressbar"'),
      "no bar when not downloading",
    );
  },

  "progress is clamped to 0..100": () => {
    assert.match(
      render({ status: "downloading", progress: 250 }),
      /aria-valuenow="100"/,
    );
    assert.match(
      render({ status: "downloading", progress: -5 }),
      /aria-valuenow="0"/,
    );
    assert.match(render({ status: "downloading" }), /aria-valuenow="0"/);
  },

  "the active row is marked and labelled": () => {
    const html = render({ status: "active" });
    assert.ok(html.includes('data-active="true"'));
    assert.ok(html.includes("In use"));
    assert.ok(!render({ status: "ready" }).includes('data-active="true"'));
  },

  "busy states show their label and no actions": () => {
    assert.ok(render({ status: "verifying" }).includes("Verifying"));
    assert.ok(render({ status: "extracting" }).includes("Extracting"));
    assert.ok(render({ status: "switching" }).includes("Switching"));
  },

  "error shows a role=alert message": () => {
    const html = render({ status: "error", errorMessage: "Checksum mismatch" });
    assert.match(html, /role="alert"/);
    assert.ok(html.includes("Checksum mismatch"));
    assert.ok(!render({ status: "ready" }).includes('role="alert"'));
  },

  "children render only for the active row by default": () => {
    const child = React.createElement(
      "div",
      { "data-testid": "extra" },
      "voice picker",
    );
    assert.ok(has(render({ status: "active", children: child }), "extra"));
    for (const status of [
      "notInstalled",
      "downloading",
      "ready",
      "error",
    ] as const) {
      assert.ok(!has(render({ status, children: child }), "extra"), status);
    }
    assert.ok(
      has(
        render({ status: "ready", children: child, showChildren: "always" }),
        "extra",
      ),
      "showChildren=always",
    );
  },

  "user-added rows are labelled and show the licence": () => {
    const html = render({
      status: "ready",
      userAdded: true,
      licence: "Apache-2.0",
    });
    assert.ok(html.includes("Added by you"));
    assert.ok(html.includes("Apache-2.0"));
    assert.ok(has(html, "model-row-edit") === false, "edit needs onEdit");
    const editable = render({
      status: "ready",
      userAdded: true,
      onEdit: () => {},
    });
    assert.ok(has(editable, "model-row-edit"));
    assert.ok(!render({ status: "ready" }).includes("Added by you"));
  },

  "a custom testid and the featured variant are supported": () => {
    const html = render({
      status: "ready",
      testId: "tts-engine-card",
      variant: "featured",
    });
    assert.ok(has(html, "tts-engine-card"));
    assert.ok(html.includes('data-variant="featured"'));
    assert.ok(render({ status: "ready" }).includes('data-variant="default"'));
  },

  "selected row exposes aria-current and the row is a labelled group": () => {
    const html = render({ status: "active" });
    assert.match(html, /aria-current="true"/);
    assert.match(html, /aria-labelledby="/);
  },

  "STT and TTS cards are built on the shared row": () => {
    const stt = read("../../onboarding/ModelCard.tsx");
    const tts = read("../TtsEngineCard.tsx");
    assert.match(
      stt,
      /from "@\/components\/settings\/models\/ModelRow"|from "\.\.\/settings\/models\/ModelRow"/,
    );
    assert.match(
      tts,
      /from "\.\/models\/ModelRow"|from "@\/components\/settings\/models\/ModelRow"/,
    );
    assert.ok(stt.includes("<ModelRow"), "ModelCard renders ModelRow");
    assert.ok(tts.includes("<ModelRow"), "TtsEngineCard renders ModelRow");
    assert.ok(tts.includes("tts-engine-card"), "existing e2e testid kept");
  },

  "the row component has no hard-coded user-visible English": () => {
    const row = read("./ModelRow.tsx");
    for (const word of [
      "Download",
      "Cancel",
      "Delete",
      "Retry",
      "In use",
      "Verifying",
    ]) {
      assert.ok(
        !row.includes(`>${word}<`) && !row.includes(`"${word}"`),
        `hard-coded ${word}`,
      );
    }
    assert.ok(
      !row.includes("useTranslation"),
      "ModelRow is pure; callers pass labels",
    );
  },
};

let failed = 0;
for (const [name, run] of Object.entries(scenarios)) {
  try {
    run();
    console.log(`ok - ${name}`);
  } catch (error) {
    failed += 1;
    console.error(`FAIL - ${name}`);
    console.error(error);
  }
}
if (failed > 0) {
  console.error(`${failed} scenario(s) failed`);
  process.exit(1);
}
console.log("modelRow ok");
