import assert from "node:assert/strict";
import {
  shortcutAnnouncesLoadFailure,
  shortcutLoadView,
  SHORTCUT_IDS,
} from "./shortcutLoadState";

// T5 (U9): a failed shortcut load shows the message with a real Retry button. The message is
// announced once (role=alert on one row only), Retry is disabled while busy, and repeated failure
// keeps the same actionable text.

const scenarios: Record<string, () => void> = {
  "no failure shows no error and no alert": () => {
    const view = shortcutLoadView({
      loadFailed: false,
      retrying: false,
      shortcutId: "transcribe",
    });
    assert.equal(view.showError, false);
    assert.equal(view.role, undefined);
  },

  "a failure shows the error with an enabled Retry button": () => {
    const view = shortcutLoadView({
      loadFailed: true,
      retrying: false,
      shortcutId: "transcribe",
    });
    assert.equal(view.showError, true);
    assert.equal(view.retryDisabled, false);
    assert.equal(view.busy, false);
    assert.equal(view.retryLabelKey, "settings.general.shortcut.retry");
  },

  "busy disables Retry and shows the busy label": () => {
    const view = shortcutLoadView({
      loadFailed: true,
      retrying: true,
      shortcutId: "transcribe",
    });
    assert.equal(view.retryDisabled, true);
    assert.equal(view.busy, true);
    assert.equal(view.retryLabelKey, "settings.general.shortcut.retrying");
  },

  "fail -> Retry -> success: the message disappears": () => {
    const failed = shortcutLoadView({
      loadFailed: true,
      retrying: false,
      shortcutId: "transcribe",
    });
    const busy = shortcutLoadView({
      loadFailed: true,
      retrying: true,
      shortcutId: "transcribe",
    });
    const recovered = shortcutLoadView({
      loadFailed: false,
      retrying: false,
      shortcutId: "transcribe",
    });
    assert.equal(failed.showError, true);
    assert.equal(busy.retryDisabled, true);
    assert.equal(recovered.showError, false);
    assert.equal(recovered.role, undefined);
  },

  "fail -> Retry -> fail: the same actionable text stays with Retry enabled":
    () => {
      const first = shortcutLoadView({
        loadFailed: true,
        retrying: false,
        shortcutId: "transcribe",
      });
      const again = shortcutLoadView({
        loadFailed: true,
        retrying: false,
        shortcutId: "transcribe",
      });
      assert.equal(again.showError, true);
      assert.equal(again.retryDisabled, false);
      assert.equal(again.retryLabelKey, first.retryLabelKey);
    },

  "exactly one shortcut row announces the failure": () => {
    const announcing = SHORTCUT_IDS.filter((id) =>
      shortcutAnnouncesLoadFailure(id),
    );
    assert.equal(announcing.length, 1);
    const view = (id: string) =>
      shortcutLoadView({ loadFailed: true, retrying: false, shortcutId: id });
    const roles = SHORTCUT_IDS.map((id) => view(id).role);
    assert.equal(roles.filter((role) => role === "alert").length, 1);
    assert.equal(view(announcing[0]).role, "alert");
  },

  "a page that shows the post-process row alone announces from that row":
    () => {
      const alone = shortcutLoadView({
        loadFailed: true,
        retrying: false,
        shortcutId: "transcribe_with_post_process",
        announces: true,
      });
      assert.equal(alone.showError, true);
      assert.equal(alone.role, "alert");
      const defaulted = shortcutLoadView({
        loadFailed: true,
        retrying: false,
        shortcutId: "transcribe_with_post_process",
      });
      assert.equal(defaulted.role, undefined);
    },

  "an explicit non-announcing row never takes the alert, even on the announcer id":
    () => {
      const silent = shortcutLoadView({
        loadFailed: true,
        retrying: false,
        shortcutId: "transcribe",
        announces: false,
      });
      assert.equal(silent.showError, true);
      assert.equal(silent.role, undefined);
    },

  "every row still shows the error and Retry, only the announcer is live":
    () => {
      for (const id of SHORTCUT_IDS) {
        const view = shortcutLoadView({
          loadFailed: true,
          retrying: false,
          shortcutId: id,
        });
        assert.equal(view.showError, true);
        assert.equal(view.retryDisabled, false);
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
if (failed > 0) throw new Error(`${failed} shortcut load scenarios failed`);
console.log(
  `shortcut load state: ${Object.keys(scenarios).length} scenarios passed`,
);
