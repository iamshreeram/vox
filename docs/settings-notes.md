# Settings notes

## Settings applied after restart

Some settings are read only once, when a component is built at startup. Changing
them in the UI does not affect the running app until Vox is restarted.

- `wake_word_confidence_threshold`: the wake-word engine is built once at startup.
- `ambient_wake_name` and `ambient_window_ms`: ambient mode is built at startup.
- `wake_word_model_name` has no settings control by design. Only the bundled
  `hey_jarvis` model exists, so a selector would offer a choice that cannot work.
  A control will be added when a second model ships.
