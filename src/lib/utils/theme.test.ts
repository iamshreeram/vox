import type { AppearanceSnapshot } from "@/bindings";
import assert from "node:assert/strict";
import {
  relativeLuminance,
  neutralBucketFor,
  onAccentFor,
  derivedAccentProperties,
  DEFAULT_CUSTOM_THEME_COLORS,
  applyResolvedAppearance,
  resolveAppearance,
} from "./theme";

// Same hand-computed test matrix as settings.rs's relative_luminance tests --
// this MUST stay in sync with that Rust implementation (see theme.ts's doc
// comment on relativeLuminance for why it's duplicated here at all).

const closeTo = (actual: number | null, expected: number, epsilon: number) => {
  assert.notEqual(actual, null, `expected a number, got null`);
  assert.ok(
    Math.abs((actual as number) - expected) < epsilon,
    `expected ${actual} to be within ${epsilon} of ${expected}`,
  );
};

closeTo(relativeLuminance("#000000"), 0.0, 1e-9);
closeTo(relativeLuminance("#ffffff"), 1.0, 1e-6);
closeTo(relativeLuminance("#808080"), 0.216, 1e-3);
closeTo(relativeLuminance("#ff0000"), 0.2126, 1e-3);
closeTo(relativeLuminance("#00ff00"), 0.7152, 1e-3);
closeTo(relativeLuminance("#bbbbbb"), 0.4966, 1e-3);
closeTo(relativeLuminance("#bcbcbc"), 0.5028, 1e-3);
assert.equal(relativeLuminance("not-hex"), null);
assert.equal(relativeLuminance("#fff"), null, "3-digit shorthand rejected");
assert.equal(relativeLuminance(""), null, "empty string rejected");

assert.equal(neutralBucketFor("#000000"), "dark");
assert.equal(neutralBucketFor("#ffffff"), "light");
assert.equal(neutralBucketFor("#808080"), "dark");
// The exact pair that straddles the 0.5 threshold either side.
assert.equal(neutralBucketFor("#bbbbbb"), "dark");
assert.equal(neutralBucketFor("#bcbcbc"), "light");
// Invalid hex defensively defaults to the dark bucket.
assert.equal(neutralBucketFor("not-hex"), "dark");

// ---- accent gradient tokens for the custom theme ----
const contrast = (a: string, b: string): number => {
  const [hi, lo] = [relativeLuminance(a)!, relativeLuminance(b)!].sort(
    (x, y) => y - x,
  );
  return (hi + 0.05) / (lo + 0.05);
};

// Dark accent ends need light text, light ends need dark text.
assert.equal(onAccentFor("#0f766e", "#7c3aed"), "#ffffff");
assert.equal(onAccentFor("#fbbf24", "#fb923c"), "#111111");
// The default (Aurora-like) custom palette: bright teal + lavender -> dark text.
assert.equal(
  onAccentFor(
    DEFAULT_CUSTOM_THEME_COLORS.accent,
    DEFAULT_CUSTOM_THEME_COLORS.accent_secondary,
  ),
  "#111111",
);
// Mixed ends: the worst-case end decides, so the text stays readable on both.
for (const [a, b] of [
  ["#ffffff", "#000000"],
  ["#2dd4bf", "#1e1b4b"],
  ["#facc15", "#7c3aed"],
]) {
  const on = onAccentFor(a, b);
  const worst = Math.min(contrast(on, a), contrast(on, b));
  const other = on === "#ffffff" ? "#111111" : "#ffffff";
  assert.ok(
    worst >= Math.min(contrast(other, a), contrast(other, b)),
    `${a}/${b}: ${on} is not the better worst-case choice`,
  );
}
// Unparseable input never throws and falls back to dark text.
assert.equal(onAccentFor("nope", "#fff"), "#111111");

const props = derivedAccentProperties({
  ...DEFAULT_CUSTOM_THEME_COLORS,
  accent: "#0f766e",
  accent_secondary: "#7c3aed",
});
assert.deepEqual(props, {
  "--color-accent-a": "#0f766e",
  "--color-accent-b": "#7c3aed",
  "--color-on-accent": "#ffffff",
  "--color-background-ui": "#0f766e",
});

// ---- UI enterprise S2: resolveAppearance table and DOM application (A1-A5) ----

const snap = (over: Partial<AppearanceSnapshot> = {}): AppearanceSnapshot => ({
  rev: 1,
  theme: "system",
  custom_colors: null,
  accent: "amber",
  orb_accent: "auto",
  chat_theme: "dark",
  visual_style: "vibrant",
  ...over,
});

const LIGHT_CUSTOM = { ...DEFAULT_CUSTOM_THEME_COLORS, background: "#ffffff" };

const THEMES = ["system", "light", "dark", "aurora", "custom"] as const;
const CHAT_THEMES = ["dark", "follow_app", "light"] as const;

function fakeRoot() {
  const props = new Map<string, string>();
  const dataset: Record<string, string | undefined> = {};
  return {
    props,
    dataset,
    style: {
      setProperty: (name: string, value: string) => {
        props.set(name, value);
      },
      removeProperty: (name: string) => {
        props.delete(name);
        return "";
      },
    },
  };
}

for (const theme of THEMES) {
  for (const prefersDark of [false, true]) {
    for (const chatTheme of CHAT_THEMES) {
      const label = `${theme} prefersDark=${prefersDark} chat=${chatTheme}`;
      const custom = theme === "custom";
      const resolved = resolveAppearance(
        snap({
          theme,
          chat_theme: chatTheme,
          accent: "blue",
          visual_style: "restrained",
          custom_colors: custom ? LIGHT_CUSTOM : null,
        }),
        prefersDark,
      );
      const bucket =
        theme === "dark" || theme === "aurora"
          ? "dark"
          : theme === "light" || custom
            ? "light"
            : prefersDark
              ? "dark"
              : "light";
      const chatPalette = chatTheme === "follow_app" ? bucket : chatTheme;
      assert.equal(resolved.theme, theme, `${label}: theme`);
      assert.equal(resolved.bucket, bucket, `${label}: bucket`);
      assert.equal(resolved.chatPalette, chatPalette, `${label}: chatPalette`);
      assert.equal(
        resolved.accent,
        custom || theme === "aurora" ? null : "blue",
        `${label}: accent applies only under system/light/dark`,
      );
      assert.equal(resolved.style, "restrained", `${label}: style`);
      assert.deepEqual(
        resolved.customColors,
        custom ? LIGHT_CUSTOM : null,
        `${label}: customColors`,
      );
    }
  }
}

{
  const root = fakeRoot();
  applyResolvedAppearance(
    root,
    resolveAppearance(snap({ accent: "blue" }), false),
  );
  assert.equal(root.dataset.accent, "blue", "system sets data-accent");
  assert.equal(root.dataset.theme, undefined, "system removes data-theme");

  applyResolvedAppearance(
    root,
    resolveAppearance(snap({ accent: "amber" }), false),
  );
  assert.equal(
    root.dataset.accent,
    undefined,
    "amber is the default, no attribute",
  );

  applyResolvedAppearance(
    root,
    resolveAppearance(snap({ theme: "aurora", accent: "rose" }), false),
  );
  assert.equal(root.dataset.accent, undefined, "aurora ignores the accent");
  assert.equal(root.dataset.theme, "aurora");

  applyResolvedAppearance(
    root,
    resolveAppearance(
      snap({ theme: "custom", accent: "rose", custom_colors: LIGHT_CUSTOM }),
      false,
    ),
  );
  assert.equal(root.dataset.accent, undefined, "custom ignores the accent");
}

{
  const root = fakeRoot();
  applyResolvedAppearance(
    root,
    resolveAppearance(
      snap({ theme: "custom", custom_colors: DEFAULT_CUSTOM_THEME_COLORS }),
      false,
    ),
  );
  assert.ok(root.props.has("--color-text"), "custom sets inline text");
  assert.ok(root.props.has("--color-accent-a"), "custom sets inline accent");
  assert.ok(root.props.has("--s-muted"), "custom sets inline overlay neutrals");
  applyResolvedAppearance(
    root,
    resolveAppearance(snap({ theme: "dark" }), false),
  );
  for (const residue of [
    "--color-text",
    "--color-background",
    "--color-logo-primary",
    "--color-accent-a",
    "--color-accent-b",
    "--color-on-accent",
    "--color-background-ui",
    "--s-muted",
  ]) {
    assert.ok(!root.props.has(residue), `Custom -> Dark leaves ${residue}`);
  }
  assert.equal(root.dataset.theme, "dark");
}

{
  const root = fakeRoot();
  applyResolvedAppearance(
    root,
    resolveAppearance(snap({ visual_style: "restrained" }), false),
  );
  assert.equal(root.dataset.style, "restrained", "restrained sets data-style");
  applyResolvedAppearance(root, resolveAppearance(snap(), false));
  assert.equal(root.dataset.style, undefined, "vibrant removes data-style");
}

console.log("theme luminance and accent tokens: all assertions passed");
