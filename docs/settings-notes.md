# Settings notes

## Settings applied after restart

Some settings are read only once, when a component is built at startup. Changing
them in the UI does not affect the running app until Vox is restarted.

- `wake_word_confidence_threshold`: the wake-word engine is built once at startup.
- `ambient_wake_name` and `ambient_window_ms`: ambient mode is built at startup.
- `wake_word_action`: retired. A wake word always starts a conversation, so the
  setting is rewritten to `conversation` once (schema 3) and any stored value loads as
  `conversation`. A downgraded build keeps `conversation`; the old one-shot choice is
  not restored.
- `ambient_mode_enabled`: ambient listening runs only while `experimental_enabled` is
  also on (`ambient_effective`). Turning Experimental off stops it but keeps the stored
  flag, so turning Experimental back on restarts it.
- `wake_word_model_name` has no settings control by design. Only the bundled
  `hey_jarvis` model exists, so a selector would offer a choice that cannot work.
  A control will be added when a second model ships.

## Where settings live

| Tab          | What is there                                                                                                                                                                                                                                    |
| ------------ | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| General      | Voice commands, custom voice shortcuts, media controls; Voice conversation, with the wake word "Hey Jarvis" (always on); ambient mode (Listening, Experimental only) (start and stop, silence and turn limits, barge-in, persona, latency panel) |
| Models       | Model settings (language, translate to English), model unload timeout, transcription models, voice activity detection engine, spoken replies, smart turn model, speaker filter model, reply style, realtime conversation                         |
| Advanced     | Engine group: post-processing toggle, keyboard implementation, acceleration, keep microphone open                                                                                                                                                |
| Post Process | Post-processing provider and model                                                                                                                                                                                                               |
| Agents       | ACP agent, agent bridge, agent tools, coding jobs, memory, conversation memory and vault index                                                                                                                                                   |
| Advanced     | The Experimental features switch (Experimental features). Off hides every experimental control                                                                                                                                                   |

Experimental features are gated by that switch. The Agents tab and the
experimental controls on the other tabs stay hidden until it is turned on.
