/*
 * Orb globe colour (DESIGN Revision 3, G1, D5, A1-A4, A2b). The centre globe is sampled from the
 * rendered pixels (not computed styles): a 6x6 px screenshot clip at the centre of each sphere is
 * decoded in the page with a canvas, and its average colour is compared with the GLOBE_STOPS
 * family (hi -> mid -> lo) or, in app mode, with the colour-mix family of the effective app accent.
 *
 * Known deviation (documented, not hidden): the orb page renders the `sleep` face as `idle`
 * (orb-face.spec), so the sleep colour is sampled on the conversation window, which does show it.
 *
 * Authoritative: an assertion here is never weakened. A wrong one is reported, not changed.
 */
import { mkdirSync } from "node:fs";
import { test, expect, type Locator, type Page } from "@playwright/test";
import {
  emitMockEvent,
  getMockCalls,
  installTauriMock,
} from "./fixtures/tauriMock";
import { appRingColour, GLOBE_STOPS, mixHex } from "../src/orb/globePalette";
import { ACCENTS, type AccentId } from "../src/styles/accents";
import { contrastRatio } from "../src/styles/wcag";

type Rgb = { r: number; g: number; b: number };
type Snapshot = Record<string, unknown>;

const ORB_PAGE = "/src/orb/index.html";
const CHAT_PAGE = "/src/chat/index.html";
const SHOTS_DIR = "/tmp/voxorb2/shots";
const ORB_AVATAR = '[data-testid="orb"] svg.vox-avatar';
const CHAT_AVATAR = ".chat-root svg.vox-avatar";
const PREVIEW_AVATAR = '[data-testid="appearance-preview-orb"] svg.vox-avatar';
const ORB_STATUS = '[data-testid="orb-refusal-status"]';
const ORB_CALL = "change_orb_accent_setting";
const AUTO = "Auto (changes with what Vox is doing)";
const FOLLOW_APP = "Follow app colour";
const TOLERANCE = 35;
const CYAN = {
  text: "#E6FBFF",
  background: "#0B1220",
  accent: "#00E5FF",
  accent_secondary: "#7C3AED",
  warning: "#F59E0B",
  error: "#EF4444",
};
const GREEN = { ...CYAN, accent: "#22C55E" };
const ACCENT_ORDER: readonly AccentId[] = [
  "amber",
  "blue",
  "violet",
  "emerald",
  "rose",
  "slate",
];
/** The state accents of Auto (D1). Sleep is not on the orb page (it renders idle). */
const AUTO_FACE_ACCENT: Record<string, AccentId> = {
  idle: "blue",
  listening: "emerald",
  thinking: "violet",
  acting: "rose",
  speaking: "amber",
};

// ---------------------------------------------------------------- colour helpers

const hex = (value: string): Rgb => ({
  r: parseInt(value.slice(1, 3), 16),
  g: parseInt(value.slice(3, 5), 16),
  b: parseInt(value.slice(5, 7), 16),
});
const dist = (a: Rgb, b: Rgb): number =>
  Math.hypot(a.r - b.r, a.g - b.g, a.b - b.b);

function segmentDistance(p: Rgb, a: Rgb, b: Rgb): number {
  const ab = { r: b.r - a.r, g: b.g - a.g, b: b.b - a.b };
  const len2 = ab.r * ab.r + ab.g * ab.g + ab.b * ab.b;
  const t =
    len2 === 0
      ? 0
      : Math.max(
          0,
          Math.min(
            1,
            ((p.r - a.r) * ab.r + (p.g - a.g) * ab.g + (p.b - a.b) * ab.b) /
              len2,
          ),
        );
  return dist(p, { r: a.r + ab.r * t, g: a.g + ab.g * t, b: a.b + ab.b * t });
}

/** Distance from a colour to the polyline through the stops (hi -> mid -> lo). */
function polylineDistance(p: Rgb, stops: readonly Rgb[]): number {
  let best = Infinity;
  for (let i = 1; i < stops.length; i += 1) {
    const a = stops[i - 1] as Rgb;
    const b = stops[i] as Rgb;
    best = Math.min(best, segmentDistance(p, a, b));
  }
  return best;
}

const tableFamily = (accent: AccentId): Rgb[] => [
  hex(GLOBE_STOPS[accent].hi),
  hex(GLOBE_STOPS[accent].mid),
  hex(GLOBE_STOPS[accent].lo),
];
/** color-mix(in srgb, X 55%, white) / X / color-mix(in srgb, X 55%, black), as the app mode paints. */
const appFamily = (accent: Rgb): Rgb[] => [
  {
    r: accent.r * 0.55 + 255 * 0.45,
    g: accent.g * 0.55 + 255 * 0.45,
    b: accent.b * 0.55 + 255 * 0.45,
  },
  accent,
  { r: accent.r * 0.55, g: accent.g * 0.55, b: accent.b * 0.55 },
];

function nearestAccent(colour: Rgb): AccentId {
  let best: AccentId = "blue";
  let bestDistance = Infinity;
  for (const accent of ACCENT_ORDER) {
    const d = polylineDistance(colour, tableFamily(accent));
    if (d < bestDistance) {
      bestDistance = d;
      best = accent;
    }
  }
  return best;
}

const isRed = (c: Rgb): boolean => c.r > c.g + 60 && c.r > c.b + 60;
const isBlue = (c: Rgb): boolean => c.b > c.r + 60 && c.b >= c.g;
const isGreen = (c: Rgb): boolean => c.g > c.r + 60 && c.g > c.b + 20;

// ---------------------------------------------------------------- page helpers

function snap(over: Snapshot): Snapshot {
  return {
    rev: 1,
    theme: "dark",
    custom_colors: null,
    accent: "amber",
    chat_theme: "dark",
    visual_style: "vibrant",
    orb_accent: "auto",
    ...over,
  };
}

const seeded = (path: string, s: Snapshot): string =>
  `${path}?ap=${encodeURIComponent(JSON.stringify(s))}`;

async function openOrb(page: Page, s: Snapshot, face = "idle"): Promise<void> {
  await installTauriMock(page, { commands: { orb_get_state: { face } } });
  await page.goto(seeded(ORB_PAGE, s));
  await expect(page.getByTestId("orb")).toBeVisible();
}

async function openChat(page: Page, s: Snapshot): Promise<void> {
  await installTauriMock(page, {
    commands: {
      get_chat_snapshot: {
        seq: 0,
        entries: [],
        state: "idle",
        end: null,
        recording: false,
      },
    },
  });
  await page.goto(seeded(CHAT_PAGE, s));
  await expect(page.locator(".chat-root")).toBeVisible();
}

/** Drives the face through converse-chat state events with increasing seq. */
function faceDriver(page: Page) {
  let seq = 0;
  return async (state: string, end: string | null = null): Promise<void> => {
    seq += 1;
    await emitMockEvent(page, "converse-chat", {
      seq,
      kind: { type: "state", state, end },
    });
  };
}

/** Average colour of a 6x6 px screenshot clip at the centre of the element, decoded in the page. */
async function centreColour(page: Page, selector: string): Promise<Rgb> {
  const target = page.locator(selector).first();
  await target.scrollIntoViewIfNeeded();
  const box = await target.boundingBox();
  if (box === null) throw new Error(`no box for ${selector}`);
  const half = 3;
  const png = await page.screenshot({
    clip: {
      x: box.x + box.width / 2 - half,
      y: box.y + box.height / 2 - half,
      width: half * 2,
      height: half * 2,
    },
    animations: "disabled",
  });
  return page.evaluate(async (base64: string) => {
    const img = new Image();
    img.src = `data:image/png;base64,${base64}`;
    await img.decode();
    const canvas = document.createElement("canvas");
    canvas.width = img.width;
    canvas.height = img.height;
    const ctx = canvas.getContext("2d");
    if (ctx === null) throw new Error("no 2d context");
    ctx.drawImage(img, 0, 0);
    const pixels = ctx.getImageData(0, 0, canvas.width, canvas.height).data;
    let r = 0;
    let g = 0;
    let b = 0;
    let n = 0;
    for (let i = 0; i < pixels.length; i += 4) {
      r += pixels[i] ?? 0;
      g += pixels[i + 1] ?? 0;
      b += pixels[i + 2] ?? 0;
      n += 1;
    }
    return { r: r / n, g: g / n, b: b / n };
  }, png.toString("base64"));
}

async function ringStroke(page: Page, avatar: string): Promise<string> {
  return page.evaluate(
    (sel) =>
      getComputedStyle(document.querySelector(`${sel} .vox-ring-1`) as Element)
        .stroke,
    avatar,
  );
}

/** Parses a computed `rgb(r, g, b)` or `rgba(...)` string. */
/** Asserts a drawn colour equals an expected #RRGGBB within a small rounding tolerance. */
function expectNear(actual: Rgb, expectedHex: string, label: string): void {
  const want = hex(expectedHex);
  expect(
    Math.max(
      Math.abs(actual.r - want.r),
      Math.abs(actual.g - want.g),
      Math.abs(actual.b - want.b),
    ),
    `${label}: ${JSON.stringify(actual)} vs ${expectedHex}`,
  ).toBeLessThanOrEqual(2);
}

/** Hue in degrees of a Rgb colour. */
function hueOfRgb(c: Rgb): number {
  const [r, g, b] = [c.r, c.g, c.b].map((v) => v / 255);
  const max = Math.max(r, g, b);
  const min = Math.min(r, g, b);
  const d = max - min;
  if (d === 0) return 0;
  let h: number;
  if (max === r) h = ((g - b) / d) % 6;
  else if (max === g) h = (b - r) / d + 2;
  else h = (r - g) / d + 4;
  return (h * 60 + 360) % 360;
}

const hueDelta = (a: number, b: number): number => {
  const d = Math.abs(a - b) % 360;
  return Math.min(d, 360 - d);
};

/** The conversation window's surface colour (--chat-bg-rgb) as #RRGGBB. */
async function chatSurfaceHex(page: Page): Promise<string> {
  const triplet = await page.evaluate(() =>
    getComputedStyle(
      document.querySelector(".chat-root") as Element,
    ).getPropertyValue("--chat-bg-rgb"),
  );
  return toHex(parseCss(triplet));
}

/** True when a drawn colour is within the rounding tolerance of an expected #RRGGBB. */
function closeTo(actual: Rgb, expectedHex: string): boolean {
  const want = hex(expectedHex);
  return (
    Math.max(
      Math.abs(actual.r - want.r),
      Math.abs(actual.g - want.g),
      Math.abs(actual.b - want.b),
    ) <= 2
  );
}

const toHex = (c: Rgb): string =>
  `#${[c.r, c.g, c.b].map((v) => Math.round(v).toString(16).padStart(2, "0")).join("")}`;

/** The Custom light theme of (j): a violet accent on a near-white background. */
const LIGHT_VIOLET = {
  text: "#111111",
  background: "#F7F8FA",
  accent: "#7C3AED",
  accent_secondary: "#6D28D9",
  warning: "#B45309",
  error: "#B91C1C",
};

/** Parses a computed colour: `rgb(r, g, b)`, or Chrome's `color(srgb r g b)` (0-1 floats). */
function parseCss(value: string): Rgb {
  const parts = (value.match(/[\d.]+/g) ?? []).map(Number);
  const scale = value.startsWith("color(") ? 255 : 1;
  return {
    r: (parts[0] ?? 0) * scale,
    g: (parts[1] ?? 0) * scale,
    b: (parts[2] ?? 0) * scale,
  };
}

const navItem = (page: Page, label: string): Locator =>
  page.locator("p", { hasText: new RegExp(`^${label}$`) });
const section = (page: Page, id: string): Locator =>
  page.locator(`[data-settings-section="${id}"]`);
const orbGroup = (page: Page): Locator =>
  page.getByRole("radiogroup", { name: "Orb colour" });
const orbRadio = (page: Page, name: string): Locator =>
  orbGroup(page).getByRole("radio", { name, exact: true });

async function openAppearance(
  page: Page,
  settings: Record<string, unknown> = {},
): Promise<void> {
  await installTauriMock(page, { settings });
  await page.goto("/");
  await navItem(page, "Appearance").click();
  await expect(section(page, "appearance")).toBeVisible();
}

// ---------------------------------------------------------------- (a) auto

test.describe("orb globe colour", () => {
  test("(a) auto: the globe follows each state, the states are pairwise distinct, concerned is red", async ({
    page,
  }) => {
    await openOrb(page, snap({ orb_accent: "auto" }));
    const drive = faceDriver(page);
    const avatar = page.locator(ORB_AVATAR);
    const samples = new Map<string, Rgb>();
    for (const [face, accent] of Object.entries(AUTO_FACE_ACCENT)) {
      await drive(face);
      await expect(avatar).toHaveAttribute("data-face", face);
      await expect(avatar).toHaveAttribute("data-orb-color", "auto");
      const colour = await centreColour(page, ORB_AVATAR);
      samples.set(face, colour);
      expect(nearestAccent(colour), `${face} globe`).toBe(accent);
      expect(
        polylineDistance(colour, tableFamily(accent)),
        `${face} globe within tolerance of ${accent}`,
      ).toBeLessThanOrEqual(TOLERANCE);
    }
    const faces = [...samples.keys()];
    for (let i = 0; i < faces.length; i += 1) {
      for (let j = i + 1; j < faces.length; j += 1) {
        const a = samples.get(faces[i] ?? "") as Rgb;
        const b = samples.get(faces[j] ?? "") as Rgb;
        expect(
          dist(a, b),
          `${faces[i]} vs ${faces[j]} must be clearly different`,
        ).toBeGreaterThanOrEqual(60);
      }
    }
    await emitMockEvent(page, "orb-refusal", { code: "dictation_busy" });
    await expect(avatar).toHaveAttribute("data-face", "concerned");
    expect(isRed(await centreColour(page, ORB_AVATAR))).toBe(true);
  });

  // ------------------------------------------------------------- (b) explicit blue

  test("(b) explicit blue: idle, thinking and sleep stay blue, concerned stays red", async ({
    page,
  }) => {
    await openOrb(page, snap({ orb_accent: "blue" }));
    const drive = faceDriver(page);
    const avatar = page.locator(ORB_AVATAR);
    for (const face of ["idle", "thinking"]) {
      await drive(face);
      await expect(avatar).toHaveAttribute("data-face", face);
      const colour = await centreColour(page, ORB_AVATAR);
      expect(isBlue(colour), `${face} is blue-dominant`).toBe(true);
      expect(nearestAccent(colour)).toBe("blue");
    }
    await emitMockEvent(page, "orb-refusal", { code: "dictation_busy" });
    await expect(avatar).toHaveAttribute("data-face", "concerned");
    expect(isRed(await centreColour(page, ORB_AVATAR))).toBe(true);

    // Sleep is shown on the conversation window (the orb page renders it as idle).
    await openChat(page, snap({ orb_accent: "blue" }));
    await faceDriver(page)("idle", "user_ended");
    await expect(page.locator(CHAT_AVATAR)).toHaveAttribute(
      "data-face",
      "sleep",
    );
    const sleep = await centreColour(page, CHAT_AVATAR);
    expect(isBlue(sleep), "sleep is blue-dominant").toBe(true);
  });

  // ------------------------------------------------------------- (c) app mode

  test("(c) app mode: the globe tracks the effective app accent live; explicit blue is unaffected", async ({
    page,
  }) => {
    await openOrb(
      page,
      snap({
        theme: "custom",
        custom_colors: CYAN,
        accent: "amber",
        orb_accent: "app",
      }),
    );
    const avatar = page.locator(ORB_AVATAR);
    await expect(avatar).toHaveAttribute("data-orb-color", "app");
    const cyan = await centreColour(page, ORB_AVATAR);
    expect(
      polylineDistance(cyan, appFamily(hex(CYAN.accent))),
      "cyan custom accent drives the globe",
    ).toBeLessThanOrEqual(TOLERANCE);
    expect(isGreen(cyan)).toBe(false);

    await emitMockEvent(
      page,
      "appearance",
      snap({
        rev: 2,
        theme: "custom",
        custom_colors: GREEN,
        accent: "amber",
        orb_accent: "app",
      }),
    );
    await expect
      .poll(async () => isGreen(await centreColour(page, ORB_AVATAR)), {
        timeout: 5000,
      })
      .toBe(true);
    const green = await centreColour(page, ORB_AVATAR);
    expect(
      polylineDistance(green, appFamily(hex(GREEN.accent))),
      "live custom green drives the globe",
    ).toBeLessThanOrEqual(TOLERANCE);

    await openOrb(
      page,
      snap({
        theme: "custom",
        custom_colors: CYAN,
        accent: "amber",
        orb_accent: "blue",
      }),
    );
    await expect(page.locator(ORB_AVATAR)).toHaveAttribute(
      "data-orb-color",
      "blue",
    );
    expect(nearestAccent(await centreColour(page, ORB_AVATAR))).toBe("blue");
  });

  // ------------------------------------------------------------- (d) two avatars

  test("(d) swatches and preview keep their own colours; ids unique; url(#id) resolves in its own svg", async ({
    page,
  }) => {
    await openAppearance(page, { orb_accent: "rose" });
    // The preview globe needs a real size: the 148px avatar box comes from avatar.css.
    const previewBox = await page.locator(PREVIEW_AVATAR).boundingBox();
    expect(previewBox?.width ?? 0).toBeGreaterThan(20);
    for (const id of ACCENT_ORDER) {
      const colour = await centreColour(
        page,
        `[data-testid="orb-swatch-${id}"] svg`,
      );
      expect(nearestAccent(colour), `swatch ${id} keeps its colour`).toBe(id);
    }
    expect(nearestAccent(await centreColour(page, PREVIEW_AVATAR))).toBe(
      "rose",
    );

    await orbRadio(page, "Blue").click();
    await expect(orbRadio(page, "Blue")).toHaveAttribute(
      "aria-checked",
      "true",
    );
    await expect
      .poll(async () => nearestAccent(await centreColour(page, PREVIEW_AVATAR)))
      .toBe("blue");
    expect(
      nearestAccent(
        await centreColour(page, '[data-testid="orb-swatch-rose"] svg'),
      ),
    ).toBe("rose");

    const audit = await page.evaluate(() => {
      const problems: string[] = [];
      const svgIds = [...document.querySelectorAll("svg [id]")].map(
        (el) => el.id,
      );
      const seen = new Set<string>();
      for (const id of svgIds) {
        if (seen.has(id)) problems.push(`duplicate svg id ${id}`);
        seen.add(id);
      }
      const svgs = [
        ...document.querySelectorAll(
          "svg.vox-avatar, [data-testid^='orb-swatch-'] svg",
        ),
      ];
      for (const svg of svgs) {
        for (const el of [svg, ...svg.querySelectorAll("*")]) {
          for (const attr of Array.from(el.attributes)) {
            for (const match of attr.value.matchAll(/url\(#([^)]+)\)/g)) {
              const id = match[1] ?? "";
              if (svg.querySelector(`[id="${id}"]`) === null) {
                problems.push(`${attr.name} url(#${id}) unresolved in its svg`);
              }
            }
          }
        }
      }
      return { problems, ids: svgIds.length, svgs: svgs.length };
    });
    expect(audit.problems).toEqual([]);
    expect(audit.svgs).toBeGreaterThanOrEqual(8);
    expect(audit.ids).toBeGreaterThanOrEqual(8);
  });

  // ------------------------------------------------------------- (e) chat window

  test("(e) conversation window: a newer event wins over a stale one; matches the orb page on an equivalent surface", async ({
    page,
  }) => {
    await openChat(
      page,
      snap({
        theme: "dark",
        chat_theme: "light",
        accent: "amber",
        orb_accent: "blue",
      }),
    );
    const drive = faceDriver(page);
    await drive("listening");
    await expect(page.locator(CHAT_AVATAR)).toHaveAttribute(
      "data-face",
      "listening",
    );
    expect(nearestAccent(await centreColour(page, CHAT_AVATAR))).toBe("blue");

    const pill = page.locator(".chat-pill");
    await expect(pill).toHaveAttribute("role", "status");
    const pillText = (await pill.textContent())?.trim() ?? "";
    expect(pillText.length).toBeGreaterThan(0);

    await emitMockEvent(
      page,
      "appearance",
      snap({
        rev: 2,
        theme: "dark",
        chat_theme: "light",
        accent: "amber",
        orb_accent: "emerald",
      }),
    );
    await expect
      .poll(async () => nearestAccent(await centreColour(page, CHAT_AVATAR)))
      .toBe("emerald");
    const ringNew = await ringStroke(page, CHAT_AVATAR);
    const globeNew = await centreColour(page, CHAT_AVATAR);

    // A stale event (rev 1, blue) must be ignored. Two frames let React render any change.
    await emitMockEvent(
      page,
      "appearance",
      snap({
        rev: 1,
        theme: "dark",
        chat_theme: "light",
        accent: "amber",
        orb_accent: "blue",
      }),
    );
    await page.evaluate(
      () =>
        new Promise<void>((resolve) =>
          requestAnimationFrame(() => requestAnimationFrame(() => resolve())),
        ),
    );
    expect(nearestAccent(await centreColour(page, CHAT_AVATAR))).toBe(
      "emerald",
    );
    expect(await ringStroke(page, CHAT_AVATAR)).toBe(ringNew);
    expect((await pill.textContent())?.trim()).toBe(pillText);
    await expect(pill).toHaveAttribute("role", "status");

    // Equivalent surface: app light -> the orb page's bucket is light, like the chat light surface.
    await openOrb(
      page,
      snap({ theme: "light", accent: "amber", orb_accent: "emerald", rev: 2 }),
      "listening",
    );
    await expect(page.locator(ORB_AVATAR)).toHaveAttribute(
      "data-face",
      "listening",
    );
    expect(await ringStroke(page, ORB_AVATAR)).toBe(ringNew);
    expect(
      dist(await centreColour(page, ORB_AVATAR), globeNew),
      "orb and chat globes match",
    ).toBeLessThanOrEqual(25);
  });

  // ------------------------------------------------------------- (i) conversation window, app mode (V2)

  test("(i) conversation window app mode: cyan, then a newer green, then a stale blue is ignored; status pill unchanged", async ({
    page,
  }) => {
    await openChat(
      page,
      snap({
        theme: "custom",
        custom_colors: CYAN,
        accent: "amber",
        chat_theme: "dark",
        orb_accent: "app",
      }),
    );
    const drive = faceDriver(page);
    const pill = page.locator(".chat-pill");
    await expect(pill).toHaveAttribute("role", "status");

    // Idle: the globe and the rings take the live custom cyan.
    expect(
      polylineDistance(
        await centreColour(page, CHAT_AVATAR),
        appFamily(hex(CYAN.accent)),
      ),
      "idle globe is the app cyan",
    ).toBeLessThanOrEqual(TOLERANCE);
    expect(
      parseCss(await ringStroke(page, CHAT_AVATAR)).b,
      "idle ring is cyan",
    ).toBeGreaterThan(200);

    // Then thinking: the globe and the rings still take the live custom cyan.
    await drive("thinking");
    await expect(page.locator(CHAT_AVATAR)).toHaveAttribute(
      "data-face",
      "thinking",
    );
    expect(
      polylineDistance(
        await centreColour(page, CHAT_AVATAR),
        appFamily(hex(CYAN.accent)),
      ),
      "thinking globe is the app cyan",
    ).toBeLessThanOrEqual(TOLERANCE);
    const cyanRing = parseCss(await ringStroke(page, CHAT_AVATAR));
    expectNear(cyanRing, mixHex(CYAN.accent, "dark"), "cyan app ring (A7)");
    const pillText = (await pill.textContent())?.trim() ?? "";
    expect(pillText.length).toBeGreaterThan(0);

    // A newer green custom accent recolours the globe and rings.
    await emitMockEvent(
      page,
      "appearance",
      snap({
        rev: 2,
        theme: "custom",
        custom_colors: GREEN,
        accent: "amber",
        chat_theme: "dark",
        orb_accent: "app",
      }),
    );
    await expect
      .poll(async () => isGreen(await centreColour(page, CHAT_AVATAR)), {
        timeout: 5000,
      })
      .toBe(true);
    const greenRing = parseCss(await ringStroke(page, CHAT_AVATAR));
    expectNear(greenRing, mixHex(GREEN.accent, "dark"), "green app ring (A7)");

    // A stale blue event (rev 1) is ignored: globe, rings and pill keep the green state.
    await emitMockEvent(
      page,
      "appearance",
      snap({
        rev: 1,
        theme: "custom",
        custom_colors: CYAN,
        accent: "amber",
        chat_theme: "dark",
        orb_accent: "app",
      }),
    );
    await page.evaluate(
      () =>
        new Promise<void>((resolve) =>
          requestAnimationFrame(() => requestAnimationFrame(() => resolve())),
        ),
    );
    expect(isGreen(await centreColour(page, CHAT_AVATAR))).toBe(true);
    expect(parseCss(await ringStroke(page, CHAT_AVATAR))).toEqual(greenRing);
    expect((await pill.textContent())?.trim()).toBe(pillText);
    await expect(pill).toHaveAttribute("role", "status");
  });

  // ------------------------------------------------------------- (j) app ring on light and dark (A7)

  test("(j) Custom light + app mode: the orb ring is the violet app ring, >= 3:1, and follows a live edit", async ({
    page,
  }) => {
    await openOrb(
      page,
      snap({
        theme: "custom",
        custom_colors: LIGHT_VIOLET,
        accent: "amber",
        orb_accent: "app",
      }),
    );
    const ring = parseCss(await ringStroke(page, ORB_AVATAR));
    expectNear(
      ring,
      mixHex("#7C3AED", "light"),
      "orb ring, Custom light violet",
    );
    expect(
      hueDelta(hueOfRgb(ring), hueOfRgb(hex("#7C3AED"))),
      "orb ring hue is the violet, not the placeholder teal",
    ).toBeLessThanOrEqual(20);
    expect(
      contrastRatio(toHex(ring), LIGHT_VIOLET.background),
      "orb ring vs the Custom light surface",
    ).toBeGreaterThanOrEqual(3);

    await emitMockEvent(
      page,
      "appearance",
      snap({
        rev: 2,
        theme: "custom",
        custom_colors: { ...LIGHT_VIOLET, accent: "#0000FF" },
        accent: "amber",
        orb_accent: "app",
      }),
    );
    await expect
      .poll(async () =>
        closeTo(
          parseCss(await ringStroke(page, ORB_AVATAR)),
          mixHex("#0000FF", "light"),
        ),
      )
      .toBe(true);
  });

  test("(j) Custom light + app mode: the Follow app outline and check are the violet app ring on the page surface", async ({
    page,
  }) => {
    await page.emulateMedia({ colorScheme: "light" });
    await openAppearance(page, {
      theme: "custom",
      custom_theme_colors: LIGHT_VIOLET,
      accent: "amber",
      orb_accent: "app",
    });
    const drawn = await orbRadio(page, FOLLOW_APP).evaluate((el) => {
      let surface = getComputedStyle(document.body).backgroundColor;
      let node: HTMLElement | null = el.parentElement;
      while (node) {
        const css = getComputedStyle(node).backgroundColor;
        if (css !== "rgba(0, 0, 0, 0)" && css !== "transparent") {
          surface = css;
          break;
        }
        node = node.parentElement;
      }
      return {
        outline: getComputedStyle(
          el.querySelector("[data-orb-outline]") as Element,
        ).stroke,
        check: getComputedStyle(el.querySelector("[data-orb-check]") as Element)
          .color,
        surface,
      };
    });
    const surface = toHex(parseCss(drawn.surface));
    for (const [label, css] of [
      ["outline", drawn.outline],
      ["check", drawn.check],
    ] as const) {
      const colour = parseCss(css);
      expectNear(colour, mixHex("#7C3AED", "light"), `Follow app ${label}`);
      expect(
        contrastRatio(toHex(colour), surface),
        `Follow app ${label} vs its surface`,
      ).toBeGreaterThanOrEqual(3);
      expect(
        hueDelta(hueOfRgb(colour), hueOfRgb(hex("#7C3AED"))),
        `Follow app ${label} hue`,
      ).toBeLessThanOrEqual(20);
    }
  });

  test("(j) chat app mode, app dark + chat light: the ring is the light-surface app ring and >= 3:1 on the chat surface", async ({
    page,
  }) => {
    await openChat(
      page,
      snap({
        theme: "dark",
        chat_theme: "light",
        accent: "amber",
        orb_accent: "app",
      }),
    );
    const ring = parseCss(await ringStroke(page, CHAT_AVATAR));
    expectNear(
      ring,
      mixHex(ACCENTS.amber.dark.fillA, "light"),
      "chat ring, app dark + chat light",
    );
    expect(
      contrastRatio(toHex(ring), await chatSurfaceHex(page)),
      "chat ring vs --chat-bg-rgb",
    ).toBeGreaterThanOrEqual(3);
  });

  test("(j) chat app mode, dark + dark: the ring is the dark-surface app ring and >= 3:1 on the chat surface", async ({
    page,
  }) => {
    await openChat(
      page,
      snap({
        theme: "dark",
        chat_theme: "dark",
        accent: "amber",
        orb_accent: "app",
      }),
    );
    const ring = parseCss(await ringStroke(page, CHAT_AVATAR));
    expectNear(
      ring,
      mixHex(ACCENTS.amber.dark.fillA, "dark"),
      "chat ring, dark + dark",
    );
    expect(
      contrastRatio(toHex(ring), await chatSurfaceHex(page)),
      "chat ring vs --chat-bg-rgb",
    ).toBeGreaterThanOrEqual(3);
  });

  // ------------------------------------------------------------- (f) appearance page

  test("(f) appearance page: option order, persistence as strings, reset to auto, arrows, check, no overflow", async ({
    page,
  }) => {
    await page.setViewportSize({ width: 760, height: 600 });
    await openAppearance(page);
    const radios = orbGroup(page).getByRole("radio");
    await expect(radios).toHaveCount(8);
    const names = [
      AUTO,
      FOLLOW_APP,
      "Amber",
      "Blue",
      "Violet",
      "Emerald",
      "Rose",
      "Slate",
    ];
    for (let i = 0; i < names.length; i += 1) {
      await expect(radios.nth(i)).toHaveAccessibleName(names[i] ?? "");
    }
    const order = await page
      .locator('[data-testid^="orb-swatch-"]')
      .evaluateAll((els) => els.map((el) => el.getAttribute("data-testid")));
    expect(order).toEqual(
      [
        "auto",
        "app",
        "amber",
        "blue",
        "violet",
        "emerald",
        "rose",
        "slate",
      ].map((id) => `orb-swatch-${id}`),
    );
    await expect(orbRadio(page, AUTO)).toHaveAttribute("aria-checked", "true");

    // Selected check is visible on the checked swatch.
    await expect(
      orbGroup(page).locator(
        '[role="radio"][aria-checked="true"] svg.lucide-check',
      ),
    ).toBeVisible();

    // Keyboard: arrows move and save strings.
    await orbRadio(page, AUTO).focus();
    await page.keyboard.press("ArrowRight");
    await expect
      .poll(async () => (await getMockCalls(page, ORB_CALL)).length)
      .toBe(1);
    await expect(orbRadio(page, FOLLOW_APP)).toBeFocused();
    await page.keyboard.press("ArrowLeft");
    await expect
      .poll(async () => (await getMockCalls(page, ORB_CALL)).length)
      .toBe(2);
    await expect(orbRadio(page, AUTO)).toBeFocused();

    // Persistence: a click saves a string; reset returns to "auto".
    await orbRadio(page, "Blue").click();
    await expect
      .poll(async () => (await getMockCalls(page, ORB_CALL)).length)
      .toBe(3);
    expect((await getMockCalls(page, ORB_CALL)).at(-1)).toEqual({
      color: "blue",
    });
    await page
      .getByRole("button", { name: "Reset appearance to defaults" })
      .click();
    await page.getByRole("button", { name: "Confirm reset" }).click();
    await expect
      .poll(async () => (await getMockCalls(page, ORB_CALL)).length)
      .toBe(4);
    expect((await getMockCalls(page, ORB_CALL)).at(-1)).toEqual({
      color: "auto",
    });
    const calls = await getMockCalls(page, ORB_CALL);
    expect(
      calls.every(
        (call) => typeof (call as { color?: unknown }).color === "string",
      ),
    ).toBe(true);
    await expect(orbRadio(page, AUTO)).toHaveAttribute("aria-checked", "true");

    const overflow = await page.evaluate(
      () =>
        document.documentElement.scrollWidth -
        document.documentElement.clientWidth,
    );
    expect(overflow).toBeLessThanOrEqual(0);
  });

  // ------------------------------------------------------------- (g) reduced motion

  test("(g) reduced motion switches the avatar animations off", async ({
    page,
  }) => {
    await page.emulateMedia({ reducedMotion: "reduce" });
    await openOrb(page, snap({ orb_accent: "blue" }));
    await faceDriver(page)("speaking");
    await expect(page.locator(ORB_AVATAR)).toHaveAttribute(
      "data-face",
      "speaking",
    );
    const animationsOff = await page.evaluate(
      (sel) =>
        [...document.querySelectorAll(`${sel}, ${sel} *`)].every(
          (el) => getComputedStyle(el).animationName === "none",
        ),
      ORB_AVATAR,
    );
    expect(animationsOff).toBe(true);
  });

  test("(g) control: with motion allowed the same face animates", async ({
    page,
  }) => {
    await page.emulateMedia({ reducedMotion: "no-preference" });
    await openOrb(page, snap({ orb_accent: "blue" }));
    await faceDriver(page)("speaking");
    await expect(page.locator(ORB_AVATAR)).toHaveAttribute(
      "data-face",
      "speaking",
    );
    const animates = await page.evaluate(
      (sel) =>
        [...document.querySelectorAll(`${sel}, ${sel} *`)].some(
          (el) => getComputedStyle(el).animationName !== "none",
        ),
      ORB_AVATAR,
    );
    expect(animates).toBe(true);
  });

  // ------------------------------------------------------------- (h) meaning carried in text

  test("(h) concerned keeps its text: the orb status and the conversation pill are announced", async ({
    page,
  }) => {
    await openOrb(page, snap({ orb_accent: "emerald" }));
    await emitMockEvent(page, "orb-refusal", { code: "dictation_busy" });
    await expect(page.locator(ORB_STATUS)).toHaveText(
      "Dictation is using the microphone.",
    );
    await expect(page.locator(ORB_AVATAR)).toHaveAttribute(
      "data-face",
      "concerned",
    );
    expect(isRed(await centreColour(page, ORB_AVATAR))).toBe(true);

    await openChat(page, snap({ orb_accent: "emerald" }));
    await faceDriver(page)("idle", "not_configured");
    await expect(page.locator(CHAT_AVATAR)).toHaveAttribute(
      "data-face",
      "concerned",
    );
    const pill = page.locator(".chat-pill");
    await expect(pill).toHaveAttribute("role", "status");
    await expect(pill).not.toHaveText("");
    expect(isRed(await centreColour(page, CHAT_AVATAR))).toBe(true);
  });

  // ------------------------------------------------------------- screenshots

  test("screenshots: the globe for each face and mode", async ({ page }) => {
    test.setTimeout(300_000);
    mkdirSync(SHOTS_DIR, { recursive: true });
    const faces = ["idle", "listening", "thinking", "acting", "speaking"];
    for (const mode of ["auto", "blue", "app"]) {
      await openOrb(page, snap({ orb_accent: mode }));
      const drive = faceDriver(page);
      for (const face of faces) {
        await drive(face);
        await expect(page.locator(ORB_AVATAR)).toHaveAttribute(
          "data-face",
          face,
        );
        await page
          .getByTestId("orb")
          .screenshot({ path: `${SHOTS_DIR}/orb-${face}-${mode}.png` });
      }
      await emitMockEvent(page, "orb-refusal", { code: "dictation_busy" });
      await expect(page.locator(ORB_AVATAR)).toHaveAttribute(
        "data-face",
        "concerned",
      );
      await page
        .getByTestId("orb")
        .screenshot({ path: `${SHOTS_DIR}/orb-concerned-${mode}.png` });
    }
    for (const mode of ["auto", "blue", "app"]) {
      await openChat(page, snap({ orb_accent: mode }));
      await faceDriver(page)("idle", "user_ended");
      await expect(page.locator(CHAT_AVATAR)).toHaveAttribute(
        "data-face",
        "sleep",
      );
      await page
        .locator(CHAT_AVATAR)
        .screenshot({ path: `${SHOTS_DIR}/chat-sleep-${mode}.png` });
    }
  });
});
