// UI enterprise S2 (DESIGN Revision 3, A3): the canonical accent table, its WCAG contrast and its
// mirror in theme.css. Pure node: no DOM. Workers must not weaken these assertions.
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import {
  ACCENT_CSS_PROPERTIES,
  ACCENT_IDS,
  ACCENT_ROLES,
  ACCENT_SURFACE,
  ACCENTS,
  DEFAULT_ACCENT,
  type AccentBucket,
  type AccentId,
} from "./accents";
import { contrastRatio } from "./wcag";

const css = readFileSync(
  fileURLToPath(new URL("./theme.css", import.meta.url)),
  "utf8",
);
const HEX = /^#[0-9a-f]{6}$/i;
const BUCKETS: AccentBucket[] = ["light", "dark"];
const NON_DEFAULT: AccentId[] = ACCENT_IDS.filter(
  (id) => id !== DEFAULT_ACCENT,
);

/** Declarations of the first `selector { ... }` block whose selector matches exactly. */
function blockFor(selector: string): string {
  const escaped = selector.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
  const match = css.match(new RegExp(`${escaped}\\s*\\{([^}]*)\\}`));
  assert.ok(match, `theme.css has no block for ${selector}`);
  return match[1];
}

function declaredIn(block: string, property: string): string | null {
  const escaped = property.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
  const match = block.match(new RegExp(`${escaped}\\s*:\\s*([^;]+);`));
  return match ? match[1].trim() : null;
}

const scenarios: Record<string, () => void> = {
  "the six accents are exactly the canonical ids in order": () => {
    assert.deepEqual(ACCENT_IDS, [
      "amber",
      "blue",
      "violet",
      "emerald",
      "rose",
      "slate",
    ]);
    assert.equal(DEFAULT_ACCENT, "amber");
  },

  "every accent and bucket defines every role as a hex colour": () => {
    for (const accent of ACCENT_IDS) {
      for (const bucket of BUCKETS) {
        for (const role of ACCENT_ROLES) {
          const value = ACCENTS[accent][bucket][role];
          assert.ok(
            HEX.test(value),
            `${accent}/${bucket}/${role} is not a hex colour: ${value}`,
          );
        }
      }
    }
  },

  "amber keeps today's exact colours, so the default look is unchanged": () => {
    assert.deepEqual(ACCENTS.amber.light, {
      fillA: "#f59e0b",
      fillB: "#f97316",
      onFill: "#1a1204",
      text: "#7a4f00",
      soft: "#fef3c7",
      logo: "#a6790a",
      ui: "#c9971e",
    });
    assert.deepEqual(ACCENTS.amber.dark, {
      fillA: "#fbbf24",
      fillB: "#fb923c",
      onFill: "#1a1204",
      text: "#f4c430",
      soft: "#3d3418",
      logo: "#f4c430",
      ui: "#c9971e",
    });
  },

  "onFill keeps at least 4.5:1 on both gradient ends in every accent and bucket":
    () => {
      for (const accent of ACCENT_IDS) {
        for (const bucket of BUCKETS) {
          const roles = ACCENTS[accent][bucket];
          for (const end of ["fillA", "fillB"] as const) {
            const ratio = contrastRatio(roles.onFill, roles[end]);
            assert.ok(
              ratio >= 4.5,
              `${accent}/${bucket} onFill on ${end}: ${ratio.toFixed(2)}:1`,
            );
          }
        }
      }
    },

  "accent text keeps at least 4.5:1 on its soft tint and on the bucket surface":
    () => {
      for (const accent of ACCENT_IDS) {
        for (const bucket of BUCKETS) {
          const roles = ACCENTS[accent][bucket];
          for (const backdrop of [roles.soft, ACCENT_SURFACE[bucket]]) {
            const ratio = contrastRatio(roles.text, backdrop);
            assert.ok(
              ratio >= 4.5,
              `${accent}/${bucket} text on ${backdrop}: ${ratio.toFixed(2)}:1`,
            );
          }
        }
      }
    },

  "every non-default accent has its own root block per bucket mirroring the table":
    () => {
      for (const accent of NON_DEFAULT) {
        const block = blockFor(`:root[data-accent="${accent}"]`);
        for (const bucket of BUCKETS) {
          for (const role of ACCENT_ROLES) {
            const property = `--${bucket}-${ACCENT_CSS_PROPERTIES[role]}`;
            assert.equal(
              declaredIn(block, property),
              ACCENTS[accent][bucket][role],
              `${accent} ${property} must mirror the table`,
            );
          }
        }
      }
    },

  "the default :root block mirrors the amber row of the table": () => {
    const root = blockFor(":root");
    for (const bucket of BUCKETS) {
      for (const role of ACCENT_ROLES) {
        const property = `--${bucket}-${ACCENT_CSS_PROPERTIES[role]}`;
        assert.equal(
          declaredIn(root, property),
          ACCENTS.amber[bucket][role],
          `amber ${property} in :root must mirror the table`,
        );
      }
    }
  },

  "no accent selector exists for aurora or custom (their palette wins, A3)":
    () => {
      assert.doesNotMatch(
        css,
        /data-theme="(aurora|custom)"[^{]*data-accent/,
        "an accent selector must not apply under aurora or custom",
      );
      const selectors = [
        ...css.matchAll(/:root\[data-accent="([a-z]+)"\]/g),
      ].map((m) => m[1]);
      assert.deepEqual([...selectors].sort(), [...NON_DEFAULT].sort());
    },

  "every existing accent token is mapped to a role (token-mapping pin)": () => {
    const root = blockFor(":root");
    assert.equal(
      declaredIn(root, "--color-accent-a"),
      "var(--light-color-accent-a)",
    );
    assert.equal(
      declaredIn(root, "--color-accent-b"),
      "var(--light-color-accent-b)",
    );
    assert.equal(
      declaredIn(root, "--color-on-accent"),
      "var(--light-color-on-accent)",
    );
    assert.equal(
      declaredIn(root, "--color-accent-text"),
      "var(--light-color-accent-text)",
    );
    assert.equal(
      declaredIn(root, "--color-accent-soft"),
      "var(--light-color-accent-soft)",
    );
    assert.equal(
      declaredIn(root, "--color-accent-ui"),
      "var(--light-color-accent-ui)",
    );
    assert.equal(
      declaredIn(root, "--color-logo-primary"),
      "var(--light-color-logo-primary)",
    );
    assert.equal(
      declaredIn(root, "--color-background-ui"),
      "var(--color-accent-ui)",
    );
    const dark = css.match(
      /@media \(prefers-color-scheme: dark\)\s*\{\s*:root\s*\{([^}]*)\}/,
    )?.[1];
    assert.ok(dark, "dark media block missing");
    assert.equal(
      declaredIn(dark, "--color-accent-text"),
      "var(--dark-color-accent-text)",
    );
    assert.equal(
      declaredIn(dark, "--color-accent-ui"),
      "var(--dark-color-accent-ui)",
    );
    assert.equal(
      declaredIn(blockFor(':root[data-theme="dark"]'), "--color-accent-soft"),
      "var(--dark-color-accent-soft)",
    );
    assert.equal(
      declaredIn(blockFor(':root[data-theme="light"]'), "--color-accent-ui"),
      "var(--light-color-accent-ui)",
    );
  },
};

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
    `${failed} of ${Object.keys(scenarios).length} accent scenarios failed`,
  );
}
console.log(`accents: ${Object.keys(scenarios).length} scenarios passed`);
