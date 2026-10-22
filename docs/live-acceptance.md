# Live acceptance: command composer, speech and conversation context

These tests check the spoken-reply path with real components. They live in
`src-tauri/src/managers/live_acceptance_tests.rs`. Every test is marked
`#[ignore = "live: needs code-puppy and network"]`, so the normal suite never
runs them.

## Prerequisites

- macOS, because the speech step uses `/usr/bin/say`.
- A command-line coding agent installed locally. The tests were written against
  a build that lists `--model` and `--no-tools` in `--help`.
- A network connection and a model the agent can reach (the tests use
  `claude-haiku-5-5`).
- The agent binary is named `code-puppy` and is found on `PATH`. Set
  `VOX_LIVE_CODE_PUPPY=/absolute/path/to/agent` to use a different location. If
  neither exists the tests fail with `GATE UNMET`; they never skip silently.

If a prerequisite is missing, each test fails with `GATE UNMET: <what>`. No test
skips silently.

## How to run

```sh
export PATH=$HOME/.cargo/bin:$PATH CMAKE_POLICY_VERSION_MINIMUM=3.5
cd src-tauri
VOX_LIVE_CODE_PUPPY=/absolute/path/to/agent \
  cargo test --lib managers::live_acceptance_tests -- --ignored --nocapture --test-threads=1
```

Lines starting with `LIVE-OUTPUT` show the spoken sentences, latencies, word
counts and emotions. The whole run takes about three minutes.

To confirm the suite is inactive by default:

```sh
cargo test --lib managers::live_acceptance_tests
```

All six tests report as ignored.

## Configuration to use Command mode

In the app's speech composer settings, choose Command mode and set:

| Setting         | Value                                                                                                                        |
| --------------- | ---------------------------------------------------------------------------------------------------------------------------- |
| Command path    | absolute path to the command-line coding agent                                                                               |
| Extra arguments | `--model claude-haiku-5-5 --no-tools` (`--no-tools` makes it a tool-free summarizer; drop it if your agent does not list it) |
| Prompt flag     | `-p`                                                                                                                         |
| Acknowledgement | accept the command in the settings UI                                                                                        |

The app runs the command with no shell, as `[path] + extra arguments + [-p, prompt]`.
The acknowledgement is stored as the exact tuple (path, arguments, flag). If you
change any of the three, the app asks for acknowledgement again.

## What each test proves

| Test                                                                              | Proves                                                                                                                                                                                                                                                                                                                                                                                        | Does not prove                                                                                                    |
| --------------------------------------------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ----------------------------------------------------------------------------------------------------------------- |
| `a1_long_short_and_error_replies_become_one_short_sentence`                       | The production runner (`speech_backends::compose_with_deadline`) returns a composed result for a 200-word technical reply, a short factual reply and an error-log excerpt. Each result is non-empty, within the 400-character speakable limit, free of markup (backticks, asterisks, hashes, pipes, slashes, angle brackets, URLs) and one or two sentences. Latency and emotion are printed. | That the model always obeys the sentence rule. It currently does not for the technical reply (see Known results). |
| `a2_timeout_returns_timeout_within_budget_and_leaves_no_process`                  | With a 2-second deadline against a wrapper that runs `sleep 10`, the runner returns `Timeout` within 2.5 seconds, and neither the wrapper nor its child is still running afterwards (checked with `pgrep` and `ps`).                                                                                                                                                                          | Behaviour under heavy load.                                                                                       |
| `a3_missing_binary_is_a_configuration_error_not_a_panic`                          | A non-existent command path returns an error from the runner and does not panic. The error is `Backend`, which is the production contract for a spawn failure.                                                                                                                                                                                                                                | That it is `BadConfig` or `NotConfigured`. See Known results.                                                     |
| `b_each_composed_sentence_is_spoken_and_exits_zero`                               | Each sentence from A1 is spoken with the speech backend's own argument and stdin builders (`tts::say_args_styled`, `tts::say_stdin_text`) through `/usr/bin/say`. The process exits with status 0 within 30 seconds.                                                                                                                                                                          | Audible output on a real speaker.                                                                                 |
| `c1_turn_two_prompt_carries_turn_one_persona_bounded_history_and_no_session_leak` | The turn-2 prompt built by `converse_plan::build_plan_prompt` contains turn 1's text, the persona ("You are Vox") and the style. Fifty scripted turns are cut to `MAX_HISTORY_ITEMS` (12), the newest turn is kept and the oldest kept turn is turn 38. A new session with an empty history contains nothing from an earlier session.                                                         | The store-side cap in `converse::push_history`, which is private (see Limits).                                    |
| `c2_live_turn_two_recalls_teal_through_the_command_path`                          | Three live runs of the production brain path (`speech_backends::complete` then `converse_plan::parse_plan`) answer turn 2 with the word "teal". The test requires all three to pass.                                                                                                                                                                                                          | A specific pass rate for a different model or prompt.                                                             |

## What these tests do NOT prove

- Microphone capture, voice-activity detection and wake words. The tests start
  from text, not audio.
- Room acoustics, speaker output and loudness. `say` writes to the default
  output device; nobody listens to it.
- Speech-to-text. The local transcription model is installed on the test
  machine, but `TranscriptionManager` needs a running Tauri `AppHandle`, which a
  unit test cannot create. That case is therefore not covered.
- Model nondeterminism. The wording, sentence count and recall can change
  between runs and between model versions. A failing live test may reflect the
  model, not the code.
- The real text-to-speech engine. Step B uses the macOS `say` command with the
  production argument and stdin builders, because the production `SpeechHandle`
  does not expose an exit status.

## Known results

- All six live tests pass (A1, A2, A3, B, C1, C2). An earlier run found that
  the model sometimes returned four sentences for a long technical reply,
  because the two-sentence rule was only in the prompt. The parser now enforces
  it (`speech_compose.rs`, `MAX_COMPOSED_SENTENCES`), and A1 passes with it.
- Latency with a command-line agent is dominated by process start-up and the
  model call: about 16 to 35 seconds per composed reply in the recorded runs.
  Speaking the composed sentence took 4 to 14 seconds.
- A3 returns `Backend`, not `BadConfig` or `NotConfigured`. The production
  runner documents a spawn failure as `Backend` (`speech_backends.rs`,
  `run_command` doc comment). The assertion follows production. Whether a
  missing binary should be a configuration error is a decision for the project.

## Limits of the conversation-context checks

- The store-side history cap is `converse::push_history`, which is private to
  the `converse` module. The test cannot reach it, so it builds the history by
  hand and checks the prompt-side cap instead. The store-side cap is covered by
  an existing unit test in `converse.rs`.
- Session isolation is checked at the prompt level: each session starts with an
  empty history, as `converse::run_turns` does. The test does not run two real
  sessions.

## Phonon text-to-speech: local acceptance

The Phonon engine has its own two live tests. They are separate from the tests
above, and they do not need the command-line agent. They run on a Mac with the
four pinned Phonon files in a folder. See [Text-to-speech engines](tts-engines.md)
for what the engine is, and [BUILD.md](../BUILD.md) for the build notes.

| Test                                                          | What it checks                                                                                                                        |
| ------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------- |
| `natural_phonon_live_speaks_and_reverifies`                   | Installs from the folder, verifies the files, loads the model, synthesises a short English sentence and checks the audio.             |
| `natural_phonon_e2e_live_speaks_through_the_router_and_stops` | Speaks a sentence through the production router to a fake output device, checks the audible span, and checks that speaking goes idle. |

Both tests are marked `#[ignore]`, so the normal suite never runs them.

### Prerequisites

- macOS (Apple silicon was used for the recorded numbers).
- The four pinned files in one folder, `<folder>`: `model.q8.gguf`,
  `default-voice.safetensors`, `tokenizer.json` and `config.json`. Get them from
  the Hugging Face page of `gradium/phonon-31m-en` at the pinned commit (see
  [Model provenance](model-provenance.md), section 3).
- A Rust toolchain that can build the Phonon crate (see BUILD.md).

### How to run

Run from the `src-tauri` folder:

```sh
VOX_PHONON_DIR=<folder> cargo test --lib natural_phonon_live -- --ignored --nocapture
VOX_PHONON_DIR=<folder> cargo test --lib natural_phonon_e2e_live -- --ignored --nocapture
```

Replace `<folder>` with the path to the folder that holds the four files.

**Important:** if `VOX_PHONON_DIR` is not set, each test prints
`SKIPPED: VOX_PHONON_DIR is not set` and reports as passed. A run with no
`SKIPPED` line is the only evidence that the audio checks ran.

### What a pass prints

A passing run prints these lines (the values vary by machine), then
`test result: ok. 1 passed` for each test.

- `natural_phonon_live_speaks_and_reverifies`: `load_ms=...`, `first_synth_ms=...`,
  `duration_s=...` (must be between 1.0 and 6.0 s), `rms=...` (must be above
  0.005), `peak=...`.
- `natural_phonon_e2e_live_speaks_through_the_router_and_stops`: `first_pcm_ms=...`
  and `sentence_span_s=... sentence_rms=...` (the span must be between 1.0 and
  8.0 s, and the RMS above 0.005).

The tests also check that all samples are finite and within bounds. A failure prints the assertion that did not hold. The printed lines
come from the test code at the time of writing; if they differ, the assertions
still decide the result.

### What these tests do NOT prove

- They use a fake output device, not a real speaker. Nobody listens to the output.
- They do not measure the vendor's claimed speeds. The recorded numbers in
  [Text-to-speech engines](tts-engines.md) are indicative only, from one machine.
- They do not cover Linux or Windows. Continuous integration runs on Linux, so the
  macOS audio paths are covered only by this local acceptance.
- They do not cover speech recognition, barge-in or echo cancellation, which are
  not part of this change.
