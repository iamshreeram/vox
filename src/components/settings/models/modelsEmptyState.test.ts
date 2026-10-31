import assert from "node:assert/strict";
import {
  isDownloadedForEmptyState,
  modelsEmptyState,
  modelsFilterActive,
  type ModelsEmptyInput,
} from "./modelsEmptyState";

// T5 (U9): the Models empty state must say "No models downloaded yet" when nothing is on disk,
// and "No models match this filter" only when a filter or search is active and hides everything.
// The two must never show each other's text.

const base: ModelsEmptyInput = {
  loading: false,
  downloadedCount: 0,
  filteredCount: 0,
  filterActive: false,
};

const noFilters = {
  searchQuery: "",
  filterStreaming: false,
  filterTranslation: false,
  languageFilter: "all",
};

const scenarios: Record<string, () => void> = {
  "no models downloaded and no filter shows the none-downloaded state": () => {
    assert.equal(
      modelsEmptyState({ ...base, filteredCount: 8 }),
      "noneDownloaded",
    );
    assert.equal(
      modelsEmptyState({ ...base, filteredCount: 0 }),
      "noneDownloaded",
    );
  },

  "a filter with zero matches and models present shows the no-match state":
    () => {
      assert.equal(
        modelsEmptyState({
          loading: false,
          downloadedCount: 2,
          filteredCount: 0,
          filterActive: true,
        }),
        "noMatch",
      );
    },

  "a filter with matches and models present shows no empty state": () => {
    assert.equal(
      modelsEmptyState({
        loading: false,
        downloadedCount: 2,
        filteredCount: 3,
        filterActive: true,
      }),
      "none",
    );
  },

  "search text with zero downloaded models and zero matches shows no-match, not none-downloaded":
    () => {
      assert.equal(
        modelsEmptyState({
          loading: false,
          downloadedCount: 0,
          filteredCount: 0,
          filterActive: true,
        }),
        "noMatch",
      );
    },

  "search text with zero downloaded models and some matches shows none-downloaded":
    () => {
      assert.equal(
        modelsEmptyState({
          loading: false,
          downloadedCount: 0,
          filteredCount: 2,
          filterActive: true,
        }),
        "noneDownloaded",
      );
    },

  "nothing is decided while loading": () => {
    assert.equal(
      modelsEmptyState({ ...base, loading: true, filterActive: true }),
      "none",
    );
    assert.equal(modelsEmptyState({ ...base, loading: true }), "none");
  },

  "no-match is only ever produced when a filter is active": () => {
    for (const downloadedCount of [0, 1, 4]) {
      for (const filterActive of [false, true]) {
        for (const filteredCount of [0, 2]) {
          const state = modelsEmptyState({
            loading: false,
            downloadedCount,
            filteredCount,
            filterActive,
          });
          if (state === "noMatch") {
            assert.equal(filterActive, true);
            assert.equal(filteredCount, 0);
          }
          if (state === "noneDownloaded") assert.equal(downloadedCount, 0);
        }
      }
    }
  },

  "a filter is active for language, streaming, translation or non-blank search only":
    () => {
      assert.equal(modelsFilterActive(noFilters), false);
      assert.equal(
        modelsFilterActive({ ...noFilters, searchQuery: "  " }),
        false,
      );
      assert.equal(
        modelsFilterActive({ ...noFilters, searchQuery: "par" }),
        true,
      );
      assert.equal(
        modelsFilterActive({ ...noFilters, filterStreaming: true }),
        true,
      );
      assert.equal(
        modelsFilterActive({ ...noFilters, filterTranslation: true }),
        true,
      );
      assert.equal(
        modelsFilterActive({ ...noFilters, languageFilter: "de" }),
        true,
      );
    },

  "downloaded means on disk or user-added, matching the Downloaded section":
    () => {
      assert.equal(
        isDownloadedForEmptyState({ is_downloaded: true, is_custom: false }),
        true,
      );
      assert.equal(
        isDownloadedForEmptyState({ is_downloaded: false, is_custom: true }),
        true,
      );
      assert.equal(
        isDownloadedForEmptyState({ is_downloaded: false, is_custom: false }),
        false,
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
if (failed > 0)
  throw new Error(`${failed} models empty-state scenarios failed`);
console.log(
  `models empty state: ${Object.keys(scenarios).length} scenarios passed`,
);
