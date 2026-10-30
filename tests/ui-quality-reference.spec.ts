import { test, expect, type Page } from "@playwright/test";
import * as Q from "./helpers/uiQuality";

/*
 * Browser-reference tests for the UI quality helpers (UI enterprise S5, DESIGN Revision 5 A6e(f)).
 * The pure self-tests (tests/helpers/uiQuality.selftest.ts) cannot run colour-mix() or rendering,
 * so these cases prove in a real Chrome that:
 *   1. colour-mix(in oklab, ...) text colours are resolved by the browser and measured, and
 *   2. the swept colour range of a gradient or a translucent colour agrees with real pixels.
 * Run: bun run test:ui-quality:reference:e2e
 */

/** Chain layers and text of one element, collected in the page (same rules as ui-quality.spec.ts). */
function collectOne(id: string) {
  const el = document.getElementById(id);
  if (!el) throw new Error(`no #${id} in the fixture`);
  const chain: Element[] = [];
  for (let n: Element | null = el; n; n = n.parentElement) chain.unshift(n);
  let cum = 1;
  const layers = chain.map((n) => {
    const s = getComputedStyle(n);
    cum *= parseFloat(s.opacity);
    return {
      background: s.backgroundColor,
      backgroundImage: s.backgroundImage,
      opacity: cum,
      mixBlendMode: s.mixBlendMode,
      maskImage: s.maskImage,
      backdropFilter: s.backdropFilter,
      filter: s.filter,
    };
  });
  const s = getComputedStyle(el);
  return {
    color: s.color,
    fontSizePx: parseFloat(s.fontSize),
    fontWeight: parseFloat(s.fontWeight),
    opacity: cum,
    layers,
    filters: [] as string[],
  };
}

/** Decodes a PNG in the page and samples a grid of pixels (RGBA), so no image library is needed. */
async function samplePixels(
  page: Pick<Page, "evaluate">,
  png: Buffer,
  step: number,
): Promise<Array<[number, number, number, number]>> {
  return page.evaluate(
    async ({ b64, every }: { b64: string; every: number }) => {
      const img = new Image();
      img.src = `data:image/png;base64,${b64}`;
      await img.decode();
      const canvas = document.createElement("canvas");
      canvas.width = img.width;
      canvas.height = img.height;
      const ctx = canvas.getContext("2d", { colorSpace: "srgb" });
      if (ctx === null) throw new Error("sRGB canvas unavailable");
      ctx.drawImage(img, 0, 0);
      const out: Array<[number, number, number, number]> = [];
      for (let y = 0; y < canvas.height; y += every) {
        for (let x = 0; x < canvas.width; x += every) {
          const d = ctx.getImageData(x, y, 1, 1).data;
          out.push([d[0], d[1], d[2], d[3]]);
        }
      }
      return out;
    },
    { b64: png.toString("base64"), every: step },
  ) as Promise<Array<[number, number, number, number]>>;
}

test.describe("ui-quality helpers: browser reference", () => {
  test.setTimeout(60_000);

  test("color-mix(in oklab) text colour is resolved by the browser and measured", async ({
    page,
  }) => {
    await page.setContent(
      '<body style="margin:0;background:#ffffff"><p id="t" style="font-size:16px;color:color-mix(in oklab, rgb(0 0 0) 50%, rgb(255 255 255))">Mixed</p></body>',
    );
    const data = (await page.evaluate(collectOne, "t")) as Q.TextCase;
    const resolve = await Q.resolveColourMap(
      page,
      Q.candidateColours(data.layers, [data.color]),
    );
    const fg = resolve(data.color);
    expect(fg, "the browser must resolve color-mix(in oklab)").not.toBeNull();
    // Oklab lightness 0.5 is about sRGB 99 grey. A value far from that means the resolution is wrong.
    expect(fg?.r ?? 0, "oklab midpoint of black and white").toBeGreaterThan(90);
    expect(fg?.r ?? 0, "oklab midpoint of black and white").toBeLessThan(110);
    const verdict = Q.evaluateTextContrast(data, resolve);
    expect(
      verdict.reason ?? "",
      "the row must be measured, not unresolved",
    ).not.toMatch(/unresolvable|unsupported/);
    expect(verdict.pass, verdict.reason ?? "contrast").toBe(true);
  });

  test("pixel reference: transparent coloured radial stops agree with the swept range", async ({
    page,
  }) => {
    await page.setContent(
      '<body style="margin:0;background:#ffffff"><div id="g" style="width:200px;height:120px;background-color:#ffffff;background-image:radial-gradient(rgba(255,0,0,0) 0%, rgba(0,0,255,0.6) 100%)"></div></body>',
    );
    const data = (await page.evaluate(collectOne, "g")) as Q.TextCase;
    const resolve = await Q.resolveColourMap(
      page,
      Q.candidateColours(data.layers),
    );
    const range = Q.composedRange(data.layers, resolve);
    expect(typeof range, "the stack must be measurable").not.toBe("string");
    const { min, max } = range as { min: Q.Rgb; max: Q.Rgb };
    // Blue stays 255 over white at both ends; the red channel carries the gradient (255 to about 102).
    expect(max.r - min.r, "the red range is non-degenerate").toBeGreaterThan(
      50,
    );

    const png = await page.locator("#g").screenshot();
    const pixels = await samplePixels(page, png, 7);
    expect(pixels.length).toBeGreaterThan(100);
    const outside = pixels.filter(
      ([r, g, b]) =>
        r < min.r - 2 ||
        r > max.r + 2 ||
        g < min.g - 2 ||
        g > max.g + 2 ||
        b < min.b - 2 ||
        b > max.b + 2,
    );
    expect(
      outside.length,
      `pixels outside the swept range +-2: ${JSON.stringify(outside.slice(0, 3))}`,
    ).toBe(0);
  });

  test("pixel reference: alpha oklab colour over white agrees with the composed colour", async ({
    page,
  }) => {
    await page.setContent(
      '<body style="margin:0;background:#ffffff"><div id="c" style="width:200px;height:120px;background-color:color-mix(in oklab, rgb(0 0 255) 60%, transparent)"></div></body>',
    );
    const data = (await page.evaluate(collectOne, "c")) as Q.TextCase;
    const resolve = await Q.resolveColourMap(
      page,
      Q.candidateColours(data.layers),
    );
    const range = Q.composedRange(data.layers, resolve);
    expect(typeof range, "the stack must be measurable").not.toBe("string");
    const { min, max } = range as { min: Q.Rgb; max: Q.Rgb };
    expect(max.r - min.r, "a flat colour has one composed value").toBeLessThan(
      1,
    );
    // A translucent blue over white must lighten red and green, so the check is not vacuous.
    expect(min.r, "blue over white is not pure blue").toBeGreaterThan(50);

    const png = await page.locator("#c").screenshot();
    const pixels = await samplePixels(page, png, 9);
    expect(pixels.length).toBeGreaterThan(50);
    const outside = pixels.filter(
      ([r, g, b]) =>
        Math.abs(r - min.r) > 2 ||
        Math.abs(g - min.g) > 2 ||
        Math.abs(b - min.b) > 2,
    );
    expect(
      outside.length,
      `pixels off the composed colour +-2: ${JSON.stringify(outside.slice(0, 3))}`,
    ).toBe(0);
  });

  test("focus ring read after settle: a transitioning box-shadow is wrong at t0 and right once settled (R5g)", async ({
    page,
  }) => {
    // The ring arrives over 400ms (transition: box-shadow), like react-select's `transition: all`.
    await page.setContent(
      '<style>#b{transition:box-shadow 400ms linear}#b:focus-visible{box-shadow:0 0 0 4px rgb(0,0,0)}</style><body style="margin:0;background:#ffffff"><button id="b" style="margin:40px">Save</button></body>',
    );
    await page.keyboard.press("Tab");
    // t0: read straight after the focus, while the transition has only just started.
    const t0 = await page.evaluate(
      () => getComputedStyle(document.getElementById("b") as Element).boxShadow,
    );
    // Settle: wait for every finite animation on the element (the same rule as the gate).
    const settled = await page.evaluate(async () => {
      const b = document.getElementById("b") as Element;
      const finite = b
        .getAnimations()
        .filter((a) => a.effect?.getComputedTiming().iterations !== Infinity);
      await Promise.all(finite.map((a) => a.finished.catch(() => undefined)));
      return getComputedStyle(b).boxShadow;
    });
    expect(t0, "the ring must still be transitioning at t0").not.toBe(settled);
    expect(settled, "the settled ring is a 4px black shadow").toBe(
      "rgb(0, 0, 0) 0px 0px 0px 4px",
    );
    // The gate reads the settled value: one outer ring, black, spread 4px. t0 does not show it.
    expect(Q.parseShadows(settled)).toEqual([
      { color: "rgb(0, 0, 0)", spread: 4 },
    ]);
    expect(
      Q.parseShadows(t0).some(
        (s) => s.spread >= 4 && s.color === "rgb(0, 0, 0)",
      ),
      "t0 must not show the settled ring",
    ).toBe(false);
  });
});
