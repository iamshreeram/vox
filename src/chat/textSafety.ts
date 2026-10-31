// Strips bidi control and invisible format characters from transcript text (security). A right-to-left
// override (U+202E) or an isolate (U+2066-2069) can make the displayed text differ from the text that
// is copied, so both the rendered text and the copied text pass through here first. Pure and linear:
// one character-class replace with no nested quantifiers.
//
// Deliberate exception: U+200D (ZERO WIDTH JOINER) and U+FE0F (VARIATION SELECTOR-16) are NOT removed,
// because emoji ZWJ sequences (for example a family) and emoji presentation depend on them.
const BIDI_AND_INVISIBLE =
  /[\u061C\u200B\u200C\u200E\u200F\u202A-\u202E\u2060\u2066-\u2069\uFEFF]/g;

/** Returns `text` without bidi controls and invisible format characters (except ZWJ and FE0F). */
export function stripBidiAndInvisible(text: string): string {
  return text.replace(BIDI_AND_INVISIBLE, "");
}
