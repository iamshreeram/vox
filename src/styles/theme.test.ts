import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";

const css = readFileSync(
  fileURLToPath(new URL("./theme.css", import.meta.url)),
  "utf8",
);

/** Declarations inside the `:root[data-theme="<name>"] { ... }` block. */
function themeBlock(name: string): string {
  const match = css.match(
    new RegExp(`:root\\[data-theme="${name}"\\]\\s*\\{([^}]*)\\}`),
  );
  assert.ok(match, `theme.css has no block for data-theme="${name}"`);
  return match[1];
}

function declared(block: string, token: string): string | null {
  const match = block.match(new RegExp(`${token}\\s*:\\s*([^;]+);`));
  return match ? match[1].trim() : null;
}

const defaultAccentFill = css
  .match(/:root\s*\{[\s\S]*?--color-background-ui:\s*([^;]+);/)?.[1]
  .trim();

const scenarios: Record<string, () => void> = {
  "the default accent fill exists": () => {
    assert.ok(defaultAccentFill, "no default --color-background-ui");
  },
  "aurora overrides the accent fill used by toggles, sliders and buttons":
    () => {
      const value = declared(themeBlock("aurora"), "--color-background-ui");
      assert.ok(value, "aurora must set --color-background-ui");
      assert.notEqual(
        value,
        defaultAccentFill,
        "aurora must not inherit the gold accent fill",
      );
    },
  "aurora's accent fill comes from its own palette variable": () => {
    const palette = declared(css, "--aurora-color-background-ui");
    assert.ok(palette, "--aurora-color-background-ui is not defined");
    assert.equal(
      declared(themeBlock("aurora"), "--color-background-ui"),
      "var(--aurora-color-background-ui)",
    );
  },
  "light and dark keep the existing accent fill": () => {
    for (const name of ["light", "dark"]) {
      const value = declared(themeBlock(name), "--color-background-ui");
      assert.ok(
        value === null || value === defaultAccentFill,
        `${name} must not change the accent fill`,
      );
    }
  },
  "every non-adaptive theme sets an accent fill": () => {
    const names = [...css.matchAll(/:root\[data-theme="([a-z-]+)"\]/g)].map(
      (m) => m[1],
    );
    for (const name of names) {
      if (name === "light" || name === "dark") continue;
      assert.ok(
        declared(themeBlock(name), "--color-background-ui"),
        `theme "${name}" has its own logo color but no accent fill`,
      );
    }
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
    `${failed} of ${Object.keys(scenarios).length} theme scenarios failed`,
  );
}
console.log(`theme tokens: ${Object.keys(scenarios).length} scenarios passed`);
