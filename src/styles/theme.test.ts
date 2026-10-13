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

  // ---- vibrant accent gradient tokens (accent-a / accent-b / on-accent) ----
  "every adaptive and named theme defines a hex accent gradient palette":
    () => {
      for (const name of ["light", "dark", "aurora"]) {
        for (const part of ["accent-a", "accent-b", "on-accent"]) {
          const v = declared(css, `--${name}-color-${part}`);
          assert.ok(v && /^#[0-9a-fA-F]{6}$/.test(v), `${name} ${part}: ${v}`);
        }
      }
    },
  "the default :root, the dark media query and every data-theme block map the active accent tokens to the right palette":
    () => {
      const root = css.match(/:root\s*\{([\s\S]*?)\n\}/)?.[1] ?? "";
      const dark =
        css.match(
          /@media \(prefers-color-scheme: dark\)\s*\{\s*:root\s*\{([^}]*)\}/,
        )?.[1] ?? "";
      const blocks: Record<string, string> = {
        light: root,
        dark,
        "data-light": themeBlock("light"),
        "data-dark": themeBlock("dark"),
        "data-aurora": themeBlock("aurora"),
      };
      const expectPalette: Record<string, string> = {
        light: "light",
        dark: "dark",
        "data-light": "light",
        "data-dark": "dark",
        "data-aurora": "aurora",
      };
      for (const [key, block] of Object.entries(blocks)) {
        for (const part of ["accent-a", "accent-b", "on-accent"]) {
          assert.equal(
            declared(block, `--color-${part}`),
            `var(--${expectPalette[key]}-color-${part})`,
            `${key} must map --color-${part}`,
          );
        }
      }
    },
  "on-accent text keeps WCAG contrast >= 4.5 on both ends of every gradient":
    () => {
      const lum = (hex: string): number => {
        const c = [1, 3, 5].map((i) => {
          const v = parseInt(hex.slice(i, i + 2), 16) / 255;
          return v <= 0.03928 ? v / 12.92 : ((v + 0.055) / 1.055) ** 2.4;
        });
        return 0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2];
      };
      const ratio = (a: string, b: string): number => {
        const [hi, lo] = [lum(a), lum(b)].sort((x, y) => y - x);
        return (hi + 0.05) / (lo + 0.05);
      };
      for (const name of ["light", "dark", "aurora"]) {
        const on = declared(css, `--${name}-color-on-accent`)!;
        for (const end of ["accent-a", "accent-b"]) {
          const bg = declared(css, `--${name}-color-${end}`)!;
          assert.ok(
            ratio(on, bg) >= 4.5,
            `${name} ${end} contrast ${ratio(on, bg).toFixed(2)} < 4.5`,
          );
        }
      }
    },
  "accent tokens are registered as Tailwind colors in App.css": () => {
    const appCss = readFileSync(
      fileURLToPath(new URL("../App.css", import.meta.url)),
      "utf8",
    );
    for (const t of ["accent-a", "accent-b", "on-accent"]) {
      assert.ok(
        new RegExp(`--color-${t}:\\s*var\\(--color-${t}\\)`).test(appCss),
        `@theme inline must register --color-${t}`,
      );
    }
  },
  "settings-card keeps overflow visible and its gradient strip is inert":
    () => {
      const appCss = readFileSync(
        fileURLToPath(new URL("../App.css", import.meta.url)),
        "utf8",
      );
      const card = appCss.match(/\.settings-card\s*\{([^}]*)\}/)?.[1] ?? "";
      const strip =
        appCss.match(/\.settings-card::before\s*\{([^}]*)\}/)?.[1] ?? "";
      assert.match(card, /position:\s*relative/);
      assert.match(card, /overflow:\s*visible/);
      assert.match(strip, /position:\s*absolute/);
      assert.match(strip, /height:\s*3px/);
      assert.match(strip, /pointer-events:\s*none/);
      assert.match(strip, /border-radius:/);
      assert.match(appCss, /\.settings-card-title/);
    },
  "the custom theme placeholder block defines the accent gradient tokens":
    () => {
      const block = themeBlock("custom");
      for (const token of [
        "--color-accent-a",
        "--color-accent-b",
        "--color-on-accent",
      ]) {
        assert.ok(declared(block, token), `custom block must set ${token}`);
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
