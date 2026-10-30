// Self-tests that PROVE the UI quality helpers (UI enterprise S5). Run with:
//   bun tests/helpers/uiQuality.selftest.ts
// Each case either proves a known-good fixture passes or a known-bad fixture is caught.
import assert from "node:assert/strict";
import {
  ConsoleErrorCollector,
  MAX_TAB_STEPS,
  MIN_SAMPLES_PER_SEGMENT,
  accessibleName,
  analyzeTabWalk,
  blurViolation,
  checkAccessibleName,
  checkFocusRing,
  contrastRatio,
  convergenceMismatches,
  evaluateTextContrast,
  evaluateTextOverBackdrops,
  expectedAppearance,
  fontSizeOk,
  motionViolation,
  overflowPx,
  parseBackgroundImage,
  parseCssColor,
  requiredContrast,
  rovingArrowKey,
  targetSizeOk,
  type FocusStyle,
  type LayerCase,
  type TextCase,
} from "./uiQuality";
import type { AppearanceSnapshot } from "@/bindings";

const GRAY_767676 = "rgb(118, 118, 118)";
const BLACK_TO_WHITE =
  "linear-gradient(rgb(0, 0, 0) 0%, rgb(255, 255, 255) 100%)";
const DARK_RAMP = "linear-gradient(rgb(17, 17, 17) 0%, rgb(34, 34, 34) 100%)";

function layer(
  backgroundImage: string,
  background = "rgba(0, 0, 0, 0)",
  opacity = 1,
): LayerCase {
  return { background, backgroundImage, opacity };
}

function text(
  color: string,
  layers: LayerCase[],
  extra: Partial<TextCase> = {},
): TextCase {
  return {
    color,
    fontSizePx: 14,
    fontWeight: 400,
    opacity: 1,
    layers,
    filters: [],
    ...extra,
  };
}

const NO_FOCUS: FocusStyle = {
  outlineStyle: "none",
  outlineWidth: "0px",
  outlineColor: "rgb(0, 0, 0)",
  boxShadow: "none",
};

const cases: Array<[string, () => void]> = [];
const scenario = (name: string, body: () => void) => cases.push([name, body]);

// --- colour model ---------------------------------------------------------
scenario("parses rgb, rgba, hex, transparent and color(srgb)", () => {
  assert.deepEqual(parseCssColor("rgb(118, 118, 118)"), {
    r: 118,
    g: 118,
    b: 118,
    a: 1,
  });
  assert.deepEqual(parseCssColor("rgba(0, 0, 0, 0.5)"), {
    r: 0,
    g: 0,
    b: 0,
    a: 0.5,
  });
  assert.deepEqual(parseCssColor("#767676"), { r: 118, g: 118, b: 118, a: 1 });
  assert.equal(parseCssColor("transparent")?.a, 0);
  assert.equal(parseCssColor("color(srgb 1 0 0)")?.r, 255);
  assert.equal(
    parseCssColor("oklch(0.7 0.1 200)"),
    null,
    "oklch is unsupported, not guessed",
  );
});

scenario(
  "#767676 on black and on white are both above 4.5:1 (endpoints)",
  () => {
    const onBlack = contrastRatio(
      { r: 118, g: 118, b: 118 },
      { r: 0, g: 0, b: 0 },
    );
    const onWhite = contrastRatio(
      { r: 118, g: 118, b: 118 },
      { r: 255, g: 255, b: 255 },
    );
    assert.ok(onBlack > 4.5, `endpoint on black ${onBlack}`);
    assert.ok(onWhite > 4.5, `endpoint on white ${onWhite}`);
  },
);

// --- contrast over layered alpha and gradients ----------------------------
scenario("#767676 text on a black->white linear gradient MUST FAIL", () => {
  const verdict = evaluateTextContrast(
    text(GRAY_767676, [layer(BLACK_TO_WHITE)]),
    "white",
  );
  assert.equal(
    verdict.pass,
    false,
    "mid-gradient luminance equals the text: must fail",
  );
  assert.ok(verdict.ratio < 1.5, `worst ratio ${verdict.ratio}`);
  assert.ok(
    verdict.worstAt !== null && Math.abs(verdict.worstAt - 0.46) < 0.05,
    `worstAt ${verdict.worstAt}`,
  );
});

scenario("the gradient is sampled >= 64 points per segment", () => {
  const verdict = evaluateTextContrast(
    text(GRAY_767676, [layer(BLACK_TO_WHITE)]),
    "black",
  );
  assert.ok(
    verdict.samples >= MIN_SAMPLES_PER_SEGMENT + 1,
    `samples ${verdict.samples}`,
  );
});

scenario(
  "a known-passing dark gradient passes (white text on #111 to #222)",
  () => {
    const verdict = evaluateTextOverBackdrops(
      text("rgb(255, 255, 255)", [layer(DARK_RAMP)]),
    );
    assert.equal(verdict.pass, true, verdict.reason ?? "");
    assert.ok(verdict.ratio > 12, `ratio ${verdict.ratio}`);
  },
);

scenario(
  "translucent layers compose over the base (white vs black backdrop)",
  () => {
    // Black text under a 50% black layer: over white the layer is #808080, over black it is #000.
    const t = text("rgb(0, 0, 0)", [layer("none", "rgba(0, 0, 0, 0.5)")]);
    const overWhite = evaluateTextContrast(t, "white");
    const overBlack = evaluateTextContrast(t, "black");
    assert.ok(
      overWhite.ratio > 3.5 && overWhite.ratio < 6,
      `white ${overWhite.ratio}`,
    );
    assert.equal(overBlack.ratio, 1, "black text over black is 1:1");
    assert.equal(overBlack.pass, false);
  },
);

scenario("cumulative opacity fades the layer and the text", () => {
  const faded = text(
    "rgb(0, 0, 0)",
    [layer("none", "rgb(255, 255, 255)", 0.2)],
    { opacity: 0.2 },
  );
  // Layer over white stays white; text at 20% over white is still legible-ish but far below 4.5.
  const verdict = evaluateTextContrast(faded, "black");
  assert.equal(verdict.pass, false);
});

scenario("the large-text rule uses 3:1 at 24px and 18.66px bold", () => {
  assert.equal(requiredContrast(24, 400), 3);
  assert.equal(requiredContrast(18.66, 700), 3);
  assert.equal(requiredContrast(18, 700), 4.5);
});

scenario(
  "unsupported paints FAIL explicitly: radial, conic, url(), filters, non-sRGB, hints",
  () => {
    const cases2: TextCase[] = [
      text("rgb(0, 0, 0)", [
        layer("radial-gradient(rgb(0, 0, 0), rgb(255, 255, 255))"),
      ]),
      text("rgb(0, 0, 0)", [
        layer("conic-gradient(rgb(0, 0, 0), rgb(255, 255, 255))"),
      ]),
      text("rgb(0, 0, 0)", [layer('url("bg.png")')]),
      text("rgb(0, 0, 0)", [
        layer("linear-gradient(in oklab, rgb(0, 0, 0), rgb(255, 255, 255))"),
      ]),
      text("rgb(0, 0, 0)", [
        layer("linear-gradient(rgb(0, 0, 0), 30%, rgb(255, 255, 255))"),
      ]),
      text("rgb(0, 0, 0)", [layer("none")], { filters: ["blur(2px)"] }),
      text("oklch(0.2 0 0)", [layer("none", "rgb(255, 255, 255)")]),
    ];
    for (const c of cases2) {
      const verdict = evaluateTextOverBackdrops(c);
      assert.equal(
        verdict.pass,
        false,
        `should fail: ${c.layers[0].backgroundImage || c.color}`,
      );
      assert.ok(verdict.reason, "an unsupported result carries a reason");
    }
    assert.equal(parseBackgroundImage("none").length, 0);
    assert.equal(
      parseBackgroundImage(
        "linear-gradient(in srgb, rgb(0, 0, 0), rgb(1, 1, 1))",
      )[0].kind,
      "linear",
    );
  },
);

// --- focus ring -------------------------------------------------------------
scenario("a transparent focus ring is detected as missing", () => {
  const focused: FocusStyle = {
    outlineStyle: "solid",
    outlineWidth: "2px",
    outlineColor: "rgba(0, 0, 0, 0)",
    boxShadow: "none",
  };
  const verdict = checkFocusRing(
    focused,
    NO_FOCUS,
    [layer("none", "rgb(255, 255, 255)")],
    "white",
  );
  assert.equal(verdict.pass, false, verdict.reason ?? "");
});

scenario(
  "no change between focused and base is reported as no indicator",
  () => {
    const verdict = checkFocusRing(
      NO_FOCUS,
      NO_FOCUS,
      [layer("none", "rgb(255, 255, 255)")],
      "white",
    );
    assert.equal(verdict.pass, false);
    assert.match(verdict.reason ?? "", /no focus indicator/);
  },
);

scenario("a solid dark outline and a blue box-shadow ring pass", () => {
  const outline: FocusStyle = {
    outlineStyle: "solid",
    outlineWidth: "2px",
    outlineColor: "rgb(0, 0, 0)",
    boxShadow: "none",
  };
  assert.equal(
    checkFocusRing(
      outline,
      NO_FOCUS,
      [layer("none", "rgb(255, 255, 255)")],
      "white",
    ).pass,
    true,
  );
  const shadow: FocusStyle = {
    ...NO_FOCUS,
    boxShadow: "rgb(0, 96, 223) 0px 0px 0px 2px",
  };
  assert.equal(
    checkFocusRing(
      shadow,
      NO_FOCUS,
      [layer("none", "rgb(255, 255, 255)")],
      "white",
    ).pass,
    true,
  );
});

// --- bounded Tab walk -------------------------------------------------------
scenario("an unreachable enabled button is reported by the Tab walk", () => {
  const result = analyzeTabWalk(["ok", "unreachable"], ["ok"], 4);
  assert.deepEqual(result.unreached, ["unreachable"]);
});

scenario(
  "a fully reached set is clean; the step cap is reported when it truncates",
  () => {
    assert.deepEqual(analyzeTabWalk(["a", "b"], ["b", "a"], 2).unreached, []);
    const capped = analyzeTabWalk(["a", "b"], ["a"], MAX_TAB_STEPS);
    assert.equal(capped.capped, true);
    assert.equal(MAX_TAB_STEPS, 300);
  },
);

scenario(
  "roving groups use arrow keys (horizontal by default, vertical when declared)",
  () => {
    assert.equal(rovingArrowKey("radiogroup", null), "ArrowRight");
    assert.equal(rovingArrowKey("tablist", "vertical"), "ArrowDown");
    assert.equal(rovingArrowKey("toolbar", null), null);
  },
);

// --- accessible names, sizes, overflow, motion, blur, console ---------------
scenario("accessible names follow the documented source order", () => {
  assert.equal(
    accessibleName({ ariaLabel: "Close", visibleText: "x" }),
    "Close",
  );
  assert.equal(accessibleName({ labelledBy: "Theme", title: "t" }), "Theme");
  assert.equal(accessibleName({ alt: "Logo" }), "Logo");
  assert.equal(checkAccessibleName({ visibleText: "   " }).pass, false);
});

scenario("font size >= 12px, target >= 24x24, overflow in px", () => {
  assert.equal(fontSizeOk(11.9), false);
  assert.equal(fontSizeOk(12), true);
  assert.equal(targetSizeOk(23, 40), false);
  assert.equal(targetSizeOk(24, 24), true);
  assert.equal(overflowPx(800, 760), 40);
  assert.equal(overflowPx(700, 760), 0);
});

scenario(
  "motion above 0.01s fails under reduced motion; ms and s both parsed",
  () => {
    const base = {
      selector: "div.x",
      transitionProperty: "none",
      transitionDuration: "0s",
    };
    assert.equal(
      motionViolation({
        ...base,
        animationName: "spin",
        animationDuration: "0.5s",
      }) !== null,
      true,
    );
    assert.equal(
      motionViolation({
        ...base,
        animationName: "none",
        animationDuration: "0s",
      }),
      null,
    );
    assert.equal(
      motionViolation({
        selector: "x",
        animationName: "none",
        animationDuration: "0s",
        transitionProperty: "color",
        transitionDuration: "200ms",
      }) !== null,
      true,
    );
    assert.equal(
      motionViolation({
        selector: "x",
        animationName: "none",
        animationDuration: "0s",
        transitionProperty: "color",
        transitionDuration: "0.01s",
      }),
      null,
    );
  },
);

scenario("backdrop-filter and blur() are flagged", () => {
  assert.match(blurViolation("blur(8px)", "none") ?? "", /backdrop-filter/);
  assert.match(blurViolation("none", "blur(2px)") ?? "", /filter/);
  assert.equal(blurViolation("none", "none"), null);
});

scenario(
  "the console collector keeps console errors and page errors, not warnings",
  () => {
    const handlers: Record<string, (value: unknown) => void> = {};
    const fakePage = {
      on(event: string, handler: (value: unknown) => void) {
        handlers[event] = handler;
        return fakePage;
      },
    };
    const collector = new ConsoleErrorCollector();
    collector.attach(fakePage as never);
    handlers.console({ type: () => "error", text: () => "boom" });
    handlers.console({ type: () => "warning", text: () => "meh" });
    handlers.pageerror(new Error("kaput"));
    assert.deepEqual(collector.drain(), ["boom", "pageerror: kaput"]);
  },
);

// --- appearance convergence -------------------------------------------------
const SNAP_OLD: AppearanceSnapshot = {
  rev: 2,
  theme: "light",
  custom_colors: null,
  accent: "amber",
  chat_theme: "dark",
  visual_style: "vibrant",
} as AppearanceSnapshot;
const SNAP_NEW: AppearanceSnapshot = {
  ...SNAP_OLD,
  rev: 3,
  accent: "blue",
  visual_style: "restrained",
} as AppearanceSnapshot;

scenario(
  "a dropped appearance event is detected by the convergence check",
  () => {
    const expected = expectedAppearance(SNAP_NEW, false);
    // The consumer never received rev 3: it still shows rev 2.
    const stale = expectedAppearance(SNAP_OLD, false);
    const mismatches = convergenceMismatches(expected, {
      ...stale,
      chatVariables: stale.chatVariables,
    });
    assert.ok(
      mismatches.length > 0,
      "dropped delivery must produce mismatches",
    );
    assert.ok(
      mismatches.some((m) => m.startsWith("data-accent")),
      mismatches.join("; "),
    );
  },
);

scenario("a converged consumer reports no mismatch", () => {
  const expected = expectedAppearance(SNAP_NEW, false);
  assert.deepEqual(convergenceMismatches(expected, { ...expected }), []);
});

// --- runner -----------------------------------------------------------------
let failed = 0;
for (const [name, body] of cases) {
  try {
    body();
    console.log(`PASS ${name}`);
  } catch (error) {
    failed += 1;
    console.log(`FAIL ${name}\n  ${(error as Error).message}`);
  }
}
console.log(
  `\nui-quality helpers: ${cases.length - failed} passed, ${failed} failed`,
);
if (failed > 0) process.exit(1);
