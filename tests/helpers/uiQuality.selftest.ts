// Self-tests that PROVE the UI quality helpers (UI enterprise S5, DESIGN Revision 5). Run with:
//   bun tests/helpers/uiQuality.selftest.ts
// Each case either proves a known-good fixture passes or a known-bad fixture is caught.
// These cases use parseSrgbLiteral as the colour resolver (no browser in bun). The browser-resolved
// cases (color-mix in oklab, pixel agreement) live in tests/ui-quality-reference.spec.ts.
import assert from "node:assert/strict";
import {
  BACKDROP_LEVELS,
  ConsoleErrorCollector,
  MAX_TAB_STEPS,
  MIN_SAMPLES_PER_SEGMENT,
  accessibleName,
  analyzeTabWalk,
  blurViolation,
  candidateColours,
  checkAccessibleName,
  checkFocusRing,
  contrastRatio,
  convergenceMismatches,
  evaluateTextContrast,
  expectedAppearance,
  fontSizeOk,
  motionViolation,
  overflowPx,
  parseBackgroundImage,
  parseSrgbLiteral,
  requiredContrast,
  resolverOf,
  rovingArrowKey,
  targetSizeOk,
  type FocusStyle,
  type LayerCase,
  type TextCase,
} from "./uiQuality";
import type { AppearanceSnapshot } from "@/bindings";

/** The resolver the self-tests use: the literal parser stands in for the browser. */
const R = parseSrgbLiteral;

const GRAY_767676 = "rgb(118, 118, 118)";
const BLACK_TO_WHITE =
  "linear-gradient(rgb(0, 0, 0) 0%, rgb(255, 255, 255) 100%)";
const DARK_RAMP = "linear-gradient(rgb(17, 17, 17) 0%, rgb(34, 34, 34) 100%)";
const RADIAL_BLACK_WHITE = "radial-gradient(rgb(0, 0, 0), rgb(255, 255, 255))";
const RADIAL_DARK = "radial-gradient(rgb(17, 17, 17), rgb(34, 34, 34))";
const TRANSPARENT_RADIAL =
  "radial-gradient(rgba(255, 255, 255, 0) 0%, rgba(255, 255, 255, 0.5) 100%)";

function layer(
  backgroundImage: string,
  background = "rgba(0, 0, 0, 0)",
  opacity = 1,
  extra: Partial<LayerCase> = {},
): LayerCase {
  return { background, backgroundImage, opacity, ...extra };
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
  assert.deepEqual(parseSrgbLiteral("rgb(118, 118, 118)"), {
    r: 118,
    g: 118,
    b: 118,
    a: 1,
  });
  assert.deepEqual(parseSrgbLiteral("rgba(0, 0, 0, 0.5)"), {
    r: 0,
    g: 0,
    b: 0,
    a: 0.5,
  });
  assert.deepEqual(parseSrgbLiteral("#767676"), {
    r: 118,
    g: 118,
    b: 118,
    a: 1,
  });
  assert.equal(parseSrgbLiteral("transparent")?.a, 0);
  assert.equal(parseSrgbLiteral("color(srgb 1 0 0)")?.r, 255);
  assert.equal(
    parseSrgbLiteral("oklch(0.7 0.1 200)"),
    null,
    "oklch is unresolvable by the literal parser, not guessed",
  );
});

scenario("the browser resolver fails closed for unknown strings", () => {
  const resolve = resolverOf({ "rgb(1, 2, 3)": { r: 1, g: 2, b: 3, a: 1 } });
  assert.deepEqual(resolve("rgb(1, 2, 3)"), { r: 1, g: 2, b: 3, a: 1 });
  assert.equal(resolve("color-mix(in oklab, red, blue)"), null);
  assert.equal(resolve("constructor"), null, "prototype keys are not colours");
});

scenario("candidateColours lists background and gradient stop colours", () => {
  const found = candidateColours(
    [layer(BLACK_TO_WHITE, "rgb(9, 9, 9)")],
    [GRAY_767676],
  );
  for (const c of [
    "rgb(9, 9, 9)",
    "rgb(0, 0, 0)",
    "rgb(255, 255, 255)",
    GRAY_767676,
  ]) {
    assert.ok(found.includes(c), `missing candidate ${c}`);
  }
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

// --- linear gradients -------------------------------------------------------
scenario("#767676 text on a black->white linear gradient MUST FAIL", () => {
  const verdict = evaluateTextContrast(
    text(GRAY_767676, [layer(BLACK_TO_WHITE)]),
    R,
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
    R,
  );
  assert.ok(
    verdict.samples >= MIN_SAMPLES_PER_SEGMENT + 1,
    `samples ${verdict.samples}`,
  );
});

scenario(
  "a known-passing dark gradient passes (white text on #111 to #222)",
  () => {
    const verdict = evaluateTextContrast(
      text("rgb(255, 255, 255)", [layer(DARK_RAMP)]),
      R,
    );
    assert.equal(verdict.pass, true, verdict.reason ?? "");
    assert.ok(verdict.ratio > 12, `ratio ${verdict.ratio}`);
  },
);

scenario(
  "an interior crossing in a linear gradient is caught (negative)",
  () => {
    // Text #767676 on a gradient whose ENDPOINTS are both fine but whose middle matches the text.
    const verdict = evaluateTextContrast(
      text(GRAY_767676, [
        layer(
          "linear-gradient(rgb(0, 0, 0), rgb(118, 118, 118), rgb(255, 255, 255))",
        ),
      ]),
      R,
    );
    assert.equal(verdict.pass, false);
  },
);

// --- radial and conic gradients (A6d): measured, not 'unsupported' ----------
scenario("#767676 on a radial black->white gradient MUST FAIL", () => {
  const verdict = evaluateTextContrast(
    text(GRAY_767676, [layer(RADIAL_BLACK_WHITE)]),
    R,
  );
  assert.equal(verdict.pass, false, "radial mid-colour equals the text");
  assert.ok(verdict.ratio < 1.5, `ratio ${verdict.ratio}`);
});

scenario("#767676 on a conic black->white gradient MUST FAIL", () => {
  const verdict = evaluateTextContrast(
    text(GRAY_767676, [
      layer("conic-gradient(rgb(0, 0, 0), rgb(255, 255, 255))"),
    ]),
    R,
  );
  assert.equal(verdict.pass, false);
});

scenario(
  "#767676 text over a transparent radial gradient atop #767676 MUST FAIL",
  () => {
    const verdict = evaluateTextContrast(
      text(GRAY_767676, [layer(TRANSPARENT_RADIAL, GRAY_767676)]),
      R,
    );
    assert.equal(verdict.pass, false, verdict.reason ?? "");
    assert.ok(verdict.ratio < 1.05, `ratio ${verdict.ratio}`);
  },
);

scenario(
  "the same text over a contributing url() image above the colour MUST FAIL",
  () => {
    const verdict = evaluateTextContrast(
      text(GRAY_767676, [
        layer(`${TRANSPARENT_RADIAL}, url("bg.png")`, GRAY_767676),
      ]),
      R,
    );
    assert.equal(verdict.pass, false);
    assert.match(verdict.reason ?? "", /url\(\) image/);
  },
);

scenario(
  "an opaque radial passing control passes (white on #111->#222)",
  () => {
    const verdict = evaluateTextContrast(
      text("rgb(255, 255, 255)", [layer(RADIAL_DARK)]),
      R,
    );
    assert.equal(verdict.pass, true, verdict.reason ?? "");
  },
);

scenario(
  "a url() image BENEATH an opaque colour is bypassed; above it fails",
  () => {
    const bypassed = evaluateTextContrast(
      text("rgb(0, 0, 0)", [
        layer('url("bg.png")', "rgba(0, 0, 0, 0)"),
        layer("none", "rgb(255, 255, 255)"),
      ]),
      R,
    );
    assert.equal(bypassed.pass, true, bypassed.reason ?? "");
    const above = evaluateTextContrast(
      text("rgb(0, 0, 0)", [layer('url("bg.png")', "rgb(255, 255, 255)")]),
      R,
    );
    assert.equal(above.pass, false);
  },
);

// --- translucent surfaces over every grey level (A6e(c)) -------------------
scenario(
  "translucent layers compose over every grey level (black text, 50% black)",
  () => {
    // Over grey 0 the layer is #000 and black text is 1:1: must fail at level 0.
    const t = text("rgb(0, 0, 0)", [layer("none", "rgba(0, 0, 0, 0.5)")]);
    const verdict = evaluateTextContrast(t, R);
    assert.equal(verdict.pass, false);
    assert.equal(verdict.ratio, 1, "black text over black is 1:1");
    assert.equal(verdict.backdrop, 0, "the worst level is grey 0");
  },
);

scenario(
  "white text on 50% black over no opaque layer fails at the brightest level",
  () => {
    // Over grey 255 the layer is 127.5 grey: contrast with white is about 4.0 (< 4.5).
    const verdict = evaluateTextContrast(
      text("rgb(255, 255, 255)", [layer("none", "rgba(0, 0, 0, 0.5)")]),
      R,
    );
    assert.equal(verdict.pass, false, verdict.reason ?? "");
    assert.equal(verdict.backdrop, 255);
    assert.ok(BACKDROP_LEVELS.includes(255));
    assert.equal(BACKDROP_LEVELS.length, 33);
  },
);

scenario("cumulative opacity below 1 on the chain fails", () => {
  const faded = text(
    "rgb(0, 0, 0)",
    [layer("none", "rgb(255, 255, 255)", 0.2)],
    { opacity: 0.2 },
  );
  assert.equal(evaluateTextContrast(faded, R).pass, false);
});

scenario("the large-text rule uses 3:1 at 24px and 18.66px bold", () => {
  assert.equal(requiredContrast(24, 400), 3);
  assert.equal(requiredContrast(18.66, 700), 3);
  assert.equal(requiredContrast(18, 700), 4.5);
});

// --- chain effects and explicit failures ----------------------------------
scenario(
  "blend mode, mask and backdrop-filter on the chain FAIL (opacity is measured)",
  () => {
    const cases2: LayerCase[][] = [
      [layer("none", "rgb(255, 255, 255)", 1, { mixBlendMode: "multiply" })],
      [layer("none", "rgb(255, 255, 255)", 1, { maskImage: "url(m.png)" })],
      [
        layer("none", "rgb(255, 255, 255)", 1, {
          backdropFilter: "blur(4px)",
        }),
      ],
    ];
    for (const layers of cases2) {
      const verdict = evaluateTextContrast(text("rgb(0, 0, 0)", layers), R);
      assert.equal(verdict.pass, false, verdict.reason ?? "");
      assert.match(verdict.reason ?? "", /effect|opacity|unsupported/);
    }
  },
);

scenario(
  "unsupported paints FAIL explicitly: url(), non-sRGB spaces, hints, filters, bad colours",
  () => {
    const cases2: TextCase[] = [
      text("rgb(0, 0, 0)", [layer('url("bg.png")')]),
      text("rgb(0, 0, 0)", [
        layer("linear-gradient(in oklab, rgb(0, 0, 0), rgb(255, 255, 255))"),
      ]),
      text("rgb(0, 0, 0)", [
        layer(
          "radial-gradient(in oklab circle, rgb(0, 0, 0), rgb(255, 255, 255))",
        ),
      ]),
      text("rgb(0, 0, 0)", [
        layer("conic-gradient(in hsl, rgb(0, 0, 0), rgb(255, 255, 255))"),
      ]),
      text("rgb(0, 0, 0)", [
        layer("linear-gradient(rgb(0, 0, 0), 30%, rgb(255, 255, 255))"),
      ]),
      text("rgb(0, 0, 0)", [
        layer("repeating-radial-gradient(rgb(0, 0, 0), rgb(255, 255, 255))"),
      ]),
      text("rgb(0, 0, 0)", [layer("none")], { filters: ["blur(2px)"] }),
      text("oklch(0.2 0 0)", [layer("none", "rgb(255, 255, 255)")]),
      text("rgb(0, 0, 0)", [
        layer("linear-gradient(rgb(0, 0, 0), rgb(255, 255, 255))"),
        layer("linear-gradient(rgb(0, 0, 0), rgb(255, 255, 255))"),
      ]),
    ];
    for (const c of cases2) {
      const verdict = evaluateTextContrast(c, R);
      assert.equal(
        verdict.pass,
        false,
        `should fail: ${c.layers[0].backgroundImage || c.color}`,
      );
      assert.ok(verdict.reason, "an unsupported result carries a reason");
    }
    assert.equal(parseBackgroundImage("none", R).length, 0);
    assert.equal(
      parseBackgroundImage(
        "linear-gradient(in srgb, rgb(0, 0, 0), rgb(1, 1, 1))",
        R,
      )[0].kind,
      "gradient",
    );
  },
);

// --- multi-layer and opacity measurement (A6e(b)) ----------------------------
const WHITE_30 =
  "radial-gradient(rgba(255, 255, 255, 0) 0%, rgba(255, 255, 255, 0.3) 100%)";

scenario(
  "#767676 text over two overlapping transparent radial layers atop #767676 MUST FAIL",
  () => {
    const verdict = evaluateTextContrast(
      text(GRAY_767676, [
        layer(`${TRANSPARENT_RADIAL}, ${TRANSPARENT_RADIAL}`, GRAY_767676),
      ]),
      R,
    );
    assert.equal(verdict.pass, false, verdict.reason ?? "");
    assert.ok(verdict.ratio < 1.05, `ratio ${verdict.ratio}`);
  },
);

scenario(
  "two layers that jointly fail, where neither layer alone fails, MUST FAIL",
  () => {
    // White text. One white layer at alpha 0..0.3 over black stays <= 76 grey: passes alone.
    // Two stacked, the composite reaches about 130 grey: fails (about 3.8:1).
    const single = evaluateTextContrast(
      text("rgb(255, 255, 255)", [layer(WHITE_30, "rgb(0, 0, 0)")]),
      R,
    );
    assert.equal(single.pass, true, `single layer: ${single.reason ?? ""}`);
    const joint = evaluateTextContrast(
      text("rgb(255, 255, 255)", [
        layer(`${WHITE_30}, ${WHITE_30}`, "rgb(0, 0, 0)"),
      ]),
      R,
    );
    assert.equal(joint.pass, false, "two layers together must fail");
    assert.ok(joint.ratio < 4.5, `joint ratio ${joint.ratio}`);
  },
);

scenario("a passing two-layer control passes (white on dark radials)", () => {
  const verdict = evaluateTextContrast(
    text("rgb(255, 255, 255)", [
      layer(`${RADIAL_DARK}, ${RADIAL_DARK}`, "rgb(0, 0, 0)"),
    ]),
    R,
  );
  assert.equal(verdict.pass, true, verdict.reason ?? "");
});

scenario(
  "#444 text at opacity .4 on white FAILS (measured, not waived)",
  () => {
    const verdict = evaluateTextContrast(
      text(
        "rgb(68, 68, 68)",
        [
          layer("none", "rgb(255, 255, 255)", 1),
          layer("none", "rgba(0, 0, 0, 0)", 0.4),
        ],
        { opacity: 0.4 },
      ),
      R,
    );
    assert.equal(verdict.pass, false, verdict.reason ?? "");
  },
);

scenario("black text at opacity .9 on white passes (measured)", () => {
  const verdict = evaluateTextContrast(
    text(
      "rgb(0, 0, 0)",
      [
        layer("none", "rgb(255, 255, 255)", 1),
        layer("none", "rgba(0, 0, 0, 0)", 0.9),
      ],
      { opacity: 0.9 },
    ),
    R,
  );
  assert.equal(verdict.pass, true, verdict.reason ?? "");
});

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
    R,
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
      R,
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
    checkFocusRing(outline, NO_FOCUS, [layer("none", "rgb(255, 255, 255)")], R)
      .pass,
    true,
  );
  const shadow: FocusStyle = {
    ...NO_FOCUS,
    boxShadow: "rgb(0, 96, 223) 0px 0px 0px 2px",
  };
  assert.equal(
    checkFocusRing(shadow, NO_FOCUS, [layer("none", "rgb(255, 255, 255)")], R)
      .pass,
    true,
  );
});

scenario(
  "a focus ring over a gradient is measured over the composed background",
  () => {
    // Black ring over a black->white gradient: the bright end has contrast 1, so it must fail.
    const outline: FocusStyle = {
      outlineStyle: "solid",
      outlineWidth: "2px",
      outlineColor: "rgb(0, 0, 0)",
      boxShadow: "none",
    };
    const verdict = checkFocusRing(
      outline,
      NO_FOCUS,
      [layer(BLACK_TO_WHITE)],
      R,
    );
    assert.equal(verdict.pass, false, verdict.reason ?? "");
  },
);

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
