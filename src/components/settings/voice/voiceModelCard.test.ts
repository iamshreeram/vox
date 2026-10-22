import { describe, expect, it } from "vitest";
import { auxErrorDisplay, voiceModelCard } from "./voiceModelCard";

describe("voiceModelCard buttons per state", () => {
  it("not installed: only Download is available", () => {
    const view = voiceModelCard("not_installed", 0, 0, null);
    expect(view).toMatchObject({
      status: "not_installed",
      canDownload: true,
      canCancel: false,
      canDelete: false,
      busy: false,
    });
  });

  it("downloading: only Cancel is available, and it is busy", () => {
    const view = voiceModelCard("downloading", 50, 100, null);
    expect(view).toMatchObject({
      status: "downloading",
      canDownload: false,
      canCancel: true,
      canDelete: false,
      busy: true,
      percent: 50,
    });
  });

  it("verifying: nothing can be pressed, and it is busy", () => {
    const view = voiceModelCard("verifying", 0, 0, null);
    expect(view).toMatchObject({
      status: "verifying",
      canDownload: false,
      canCancel: false,
      canDelete: false,
      busy: true,
    });
  });

  it("ready: only Delete is available", () => {
    const view = voiceModelCard("ready", 0, 0, null);
    expect(view).toMatchObject({
      status: "ready",
      canDownload: false,
      canCancel: false,
      canDelete: true,
      busy: false,
    });
  });

  it("error: Download (retry) is available, Delete is not", () => {
    const view = voiceModelCard("error", 0, 0, "corrupt");
    expect(view).toMatchObject({
      status: "error",
      canDownload: true,
      canDelete: false,
      busy: false,
      error: "corrupt",
    });
  });

  it("an unknown state is treated as not installed", () => {
    const view = voiceModelCard("something_new", 0, 0, null);
    expect(view.status).toBe("not_installed");
    expect(view.canDelete).toBe(false);
  });
});

describe("voiceModelCard progress", () => {
  it("percent is floored and clamped to 0..100 with total 0 as zero", () => {
    expect(voiceModelCard("downloading", 1, 3, null).percent).toBe(33);
    expect(voiceModelCard("downloading", 5, 0, null).percent).toBe(0);
    expect(voiceModelCard("downloading", 9, 3, null).percent).toBe(100);
    expect(voiceModelCard("downloading", -4, 10, null).percent).toBe(0);
    expect(voiceModelCard("downloading", Number.NaN, 10, null).percent).toBe(0);
  });

  it("percent is zero outside a download", () => {
    expect(voiceModelCard("ready", 10, 10, null).percent).toBe(0);
  });
});

describe("auxErrorDisplay", () => {
  it("cancelled is not shown as an error", () => {
    expect(auxErrorDisplay("cancelled")).toBeNull();
    expect(auxErrorDisplay(null)).toBeNull();
  });

  it("known codes map to themselves", () => {
    for (const code of [
      "busy",
      "corrupt",
      "url_not_allowed",
      "download",
      "io",
      "not_installed",
    ]) {
      expect(auxErrorDisplay(code)).toBe(code);
    }
  });

  it("unknown codes map to unknown", () => {
    expect(auxErrorDisplay("something_new")).toBe("unknown");
  });
});
