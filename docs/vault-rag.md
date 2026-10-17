# Vault notes search

Vault notes search lets the voice assistant look up matching passages in the
notes of your vault and use them to answer. It is experimental and off by
default. This page says what it does, what leaves your machine, and what it
does not do.

## What it is

- A local keyword index of the folders you list, built from the Markdown notes
  in your vault folder.
- When you ask a question, Vox finds matching passages in that index and adds a
  short, capped amount of them to the model's context for that turn.
- Optional read-only vault tools can let the model look up a note or a list of
  notes during a turn. They run only when agent tools are on (see below).
- Search is keyword based. It does not use embeddings, so a note that uses
  different words for the same idea may not match.

## How to turn it on

1. Open Settings, then Experimental, then the Conversation memory group.
2. Set the Vault folder path. Vault notes search cannot be turned on without
   it; the toggle stays disabled and shows a hint until a path is set.
3. Turn on "Search my vault notes". The toggle needs the realtime model route
   to be in use.
4. Optional: turn on agent tools to let the model use the read-only vault tools.
   Without agent tools, only the keyword evidence is used.

The status line shows whether the index is off, indexing, ready, busy or
unavailable, with the number of notes, searchable sections and skipped files.
"Re-index now" rebuilds the index. It is disabled while indexing is running.
The status is refreshed about every two seconds, but only while the
Experimental page is visible and the toggle is on. Leaving the page or turning
the toggle off stops the refresh.

## What is sent to the model provider, and when

Turning the toggle on means you agree that matching passages from the listed
folders may be sent to the model provider you configured. Nothing from the
vault is sent while the toggle is off.

When the feature is active for a turn, the following go to the configured model
provider:

- your question and the recent turns of the conversation;
- the matching note snippets for that turn;
- tool arguments and tool results, when vault tools are used.

Notes outside the listed folders are never read for the index.

## Folders setting

- Enter folder names inside the vault, separated by commas.
- Each name must be a plain single name: no slash, no backslash, no `..`, no
  leading dot, and at most 64 characters.
- Blank entries are ignored and repeated names are kept once.
- At most 16 folders. At least one folder is required.
- The default is `Projects,Daily,Memory,Meetings,Resources`.
- The list is checked when you leave the field. If the check fails, the error
  is shown under the field, the previous value is restored, and nothing is
  saved.

## Vault tools (read only)

The vault tools only read notes. They cannot create, change, move or delete
anything in the vault. They run only when both of these are true:

- agent tools are on, and
- the direct model route is in use.

Tool results are treated as untrusted text. They are wrapped and labelled so
the model does not treat note content as instructions.

## Tool support probe and the unsupported fallback

Not every model route supports tool calls. "Test tool support" sends a small
test request on the current route. It shows one of three results: supported,
unsupported, or unknown. A failed request shows the error text instead.

The result is kept for the current route. Changing the endpoint or the model
resets it to untested.

If the result is unsupported, vault sessions on that route switch to evidence
only. They send the keyword evidence without any tools, and no tool results are
added to the history. The reply is then based on the evidence alone.

## Answer-only sessions, and why

While vault notes are part of a conversation, the session is answer-only. It
cannot paste text, open apps or run actions.

The reason: a note can contain text that looks like an instruction, such as
"ignore the previous instructions". The model could repeat such text in its
reply, or try to act on it. Limiting the session to answers keeps note content
from triggering actions.

## Replies are spoken aloud

Replies are read out loud. Anyone nearby may hear note content in a reply.
Keep this in mind when you use the feature in a shared room.

## Hiding secrets is best effort

Vox tries to hide obvious secrets in note text before they are indexed or sent,
such as key-like strings. This is best effort. It can miss secrets, and it can
hide text that is not a secret. Do not rely on it to protect sensitive notes.

## Cache location and how to delete it

The index is a local cache file, not a copy of your notes. It is stored in the
application data folder, with a name of the form `vault_index-<id>.db`. SQLite
may also keep `-wal` and `-shm` side files next to it. On Unix the file is
created with owner-only permissions.

To delete it, quit Vox and remove the `vault_index-*.db` files and their `-wal`
and `-shm` side files from the application data folder. Deleting the cache does
not change your notes. If the cache is found to be corrupt or from a newer
version, Vox deletes it and builds it again.

## Shutdown behaviour

When Vox quits, the vault runtime stops without waiting. Indexing or a running
probe is not finished first; background work ends with the process. The index
file is left as it is, and the next start checks it as described above.

## Known limits

- Keyword search only, not embeddings: different wording can miss a match.
- Large vaults take longer to index, and re-indexing runs in the background.
- Secret hiding is best effort (see above).
- Replies are spoken aloud, so note content can be heard by people nearby.
- Only Markdown notes in the listed folders are indexed. Hidden files and
  folders are skipped.
- Evidence per turn is capped (3000 characters in the current build).
- Real model providers and gateways, the settings page on every platform, and
  Windows and Linux builds have not all been verified yet. Treat the feature as
  experimental until they have.
