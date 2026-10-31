// Safe reply rendering (UI enterprise T2, DESIGN B7). A pure, single-pass, line-based parser that turns
// a reply into a small AST: paragraphs, bullet and numbered lists, fenced code blocks, inline code,
// **bold** and *italic*. Everything else is plain text. There is NO raw HTML, NO links and NO images:
// the AST holds only strings and these few node kinds, and ChatMarkdown renders them as React text
// nodes (never a raw-HTML injection prop). Every scan is linear or amortised linear: no regex
// backtracking over user text, and emphasis recursion is depth-limited.

export type InlineNode =
  | { kind: "text"; text: string }
  | { kind: "code"; text: string }
  | { kind: "strong"; children: InlineNode[] }
  | { kind: "em"; children: InlineNode[] };

export type BlockNode =
  | { kind: "paragraph"; lines: InlineNode[][] }
  | { kind: "list"; ordered: boolean; items: InlineNode[][][] }
  | { kind: "code"; lang: string; text: string };

const MAX_EMPHASIS_DEPTH = 2;
const MAX_LANG_LENGTH = 32;
const CACHE_LIMIT = 2000;

/** Leading spaces (0 to 3) before a block marker; -1 when there are more, which is not a marker. */
function markerIndent(line: string): number {
  let n = 0;
  while (n < line.length && line[n] === " ") n += 1;
  return n <= 3 ? n : -1;
}

/** A code fence: 3+ backticks after up to 3 spaces, with an info string that has no backtick. */
function fenceOpen(line: string): { run: number; lang: string } | null {
  const indent = markerIndent(line);
  if (indent < 0) return null;
  let i = indent;
  while (line[i] === "`") i += 1;
  const run = i - indent;
  if (run < 3) return null;
  const info = line.slice(i).trim();
  if (info.includes("`")) return null;
  const lang = (info.split(/\s+/)[0] ?? "").slice(0, MAX_LANG_LENGTH);
  return { run, lang };
}

/** A closing fence: a backtick run at least as long as the opener, and nothing else on the line. */
function fenceClose(line: string, run: number): boolean {
  const trimmed = line.trim();
  if (trimmed.length < run) return false;
  for (let i = 0; i < trimmed.length; i += 1) {
    if (trimmed[i] !== "`") return false;
  }
  return true;
}

interface ListMarker {
  ordered: boolean;
  content: string;
}

/** A bullet (-, *, +) or numbered (1. or 1)) list item start, with its text. */
function listMarker(line: string): ListMarker | null {
  const indent = markerIndent(line);
  if (indent < 0) return null;
  const first = line[indent];
  if (first === "-" || first === "*" || first === "+") {
    const after = line[indent + 1];
    if (after !== " " && after !== "\t") return null;
    return { ordered: false, content: line.slice(indent + 2).trim() };
  }
  let i = indent;
  while (i < line.length && i - indent < 9) {
    const c = line.charCodeAt(i);
    if (c < 48 || c > 57) break;
    i += 1;
  }
  if (i === indent) return null;
  const punct = line[i];
  if ((punct !== "." && punct !== ")") || line[i + 1] !== " ") return null;
  return { ordered: true, content: line.slice(i + 2).trim() };
}

function isBlank(line: string): boolean {
  return line.trim().length === 0;
}

function isIndented(line: string): boolean {
  return line.startsWith("  ") || line.startsWith("\t");
}

/** Index of the first backtick run of `length` that starts after `after`, or -1. */
function findRun(
  starts: number[],
  positions: Map<number, number>,
  after: number,
  length: number,
): number {
  let lo = 0;
  let hi = starts.length;
  while (lo < hi) {
    const mid = (lo + hi) >>> 1;
    if ((starts[mid] ?? 0) <= after) lo = mid + 1;
    else hi = mid;
  }
  for (let k = lo; k < starts.length; k += 1) {
    const s = starts[k] ?? 0;
    if (positions.get(s) === length) return s;
  }
  return -1;
}

/** Inline spans of one line: code, **strong**, *em*. Unmatched markers stay literal text. */
export function parseInline(line: string, depth = 0): InlineNode[] {
  const out: InlineNode[] = [];
  if (line.length === 0) return out;

  // Backtick runs, once per line. A run of length L closes at the next run of exactly length L.
  const runLength = new Map<number, number>();
  const byLength = new Map<number, number[]>();
  for (let i = 0; i < line.length; ) {
    if (line[i] !== "`") {
      i += 1;
      continue;
    }
    let j = i;
    while (line[j] === "`") j += 1;
    const len = j - i;
    runLength.set(i, len);
    const list = byLength.get(len);
    if (list) list.push(i);
    else byLength.set(len, [i]);
    i = j;
  }

  let text = "";
  const flush = (): void => {
    if (text.length > 0) out.push({ kind: "text", text });
    text = "";
  };
  let strongFailed = false;
  let emFailedFrom = Number.POSITIVE_INFINITY;

  let i = 0;
  while (i < line.length) {
    const ch = line[i];

    if (ch === "`") {
      const len = runLength.get(i) ?? 1;
      const starts = byLength.get(len) ?? [];
      const close = findRun(starts, runLength, i + len - 1, len);
      if (close > i) {
        flush();
        let content = line.slice(i + len, close);
        // One space is stripped from each side when both are present and the text is not blank.
        if (
          content.length > 2 &&
          content.startsWith(" ") &&
          content.endsWith(" ") &&
          content.trim().length > 0
        ) {
          content = content.slice(1, -1);
        }
        out.push({ kind: "code", text: content });
        i = close + len;
        continue;
      }
      text += line.slice(i, i + len);
      i += len;
      continue;
    }

    if (
      ch === "*" &&
      line[i + 1] === "*" &&
      !strongFailed &&
      depth <= MAX_EMPHASIS_DEPTH
    ) {
      const close = line.indexOf("**", i + 2);
      if (close > i + 2) {
        flush();
        const inner = line.slice(i + 2, close);
        out.push({
          kind: "strong",
          children:
            depth < MAX_EMPHASIS_DEPTH
              ? parseInline(inner, depth + 1)
              : [{ kind: "text", text: inner }],
        });
        i = close + 2;
        continue;
      }
      if (close === -1) strongFailed = true;
    }

    if (
      ch === "*" &&
      line[i + 1] !== "*" &&
      line[i + 1] !== " " &&
      i < emFailedFrom &&
      depth <= MAX_EMPHASIS_DEPTH &&
      (i === 0 || line[i - 1] !== "*")
    ) {
      const close = findEmClose(line, i + 1);
      if (close > i + 1) {
        flush();
        const inner = line.slice(i + 1, close);
        out.push({
          kind: "em",
          children:
            depth < MAX_EMPHASIS_DEPTH
              ? parseInline(inner, depth + 1)
              : [{ kind: "text", text: inner }],
        });
        i = close + 1;
        continue;
      }
      emFailedFrom = i;
    }

    text += ch;
    i += 1;
  }
  flush();
  return out;
}

/** A single '*' (not part of '**') that follows a non-space character, at or after `from`. */
function findEmClose(line: string, from: number): number {
  for (let j = from; j < line.length; j += 1) {
    if (line[j] !== "*") continue;
    if (line[j - 1] === "*" || line[j + 1] === "*") continue;
    if (line[j - 1] === " ") continue;
    return j;
  }
  return -1;
}

/** Parses a whole reply into blocks. Empty and whitespace-only input gives no blocks. */
export function parseMarkdown(source: string): BlockNode[] {
  const blocks: BlockNode[] = [];
  const lines = source.split("\n");
  let paragraph: InlineNode[][] | null = null;
  let list: { ordered: boolean; items: InlineNode[][][] } | null = null;
  let fence: { run: number; lang: string; body: string[] } | null = null;

  const closeParagraph = (): void => {
    if (paragraph) blocks.push({ kind: "paragraph", lines: paragraph });
    paragraph = null;
  };
  const closeList = (): void => {
    if (list)
      blocks.push({ kind: "list", ordered: list.ordered, items: list.items });
    list = null;
  };

  for (const raw of lines) {
    const line = raw.endsWith("\r") ? raw.slice(0, -1) : raw;

    if (fence) {
      if (fenceClose(line, fence.run)) {
        blocks.push({
          kind: "code",
          lang: fence.lang,
          text: fence.body.join("\n"),
        });
        fence = null;
      } else {
        fence.body.push(line);
      }
      continue;
    }

    const opened = fenceOpen(line);
    if (opened) {
      closeParagraph();
      closeList();
      fence = { run: opened.run, lang: opened.lang, body: [] };
      continue;
    }

    if (isBlank(line)) {
      closeParagraph();
      closeList();
      continue;
    }

    const marker = listMarker(line);
    if (marker) {
      closeParagraph();
      if (list && list.ordered !== marker.ordered) closeList();
      if (!list) list = { ordered: marker.ordered, items: [] };
      list.items.push([parseInline(marker.content)]);
      continue;
    }

    if (list && isIndented(line)) {
      // An indented line continues the last list item.
      const items = list.items;
      const last = items[items.length - 1];
      if (last) last.push(parseInline(line.trim()));
      continue;
    }

    closeList();
    if (!paragraph) paragraph = [];
    paragraph.push(parseInline(line.trim()));
  }

  if (fence) {
    // An unclosed fence runs to the end of the message.
    blocks.push({
      kind: "code",
      lang: fence.lang,
      text: fence.body.join("\n"),
    });
  }
  closeParagraph();
  closeList();
  return blocks;
}

const cache = new Map<string, BlockNode[]>();

/** Parsed blocks for one message, memoised by id and text (the same message is never parsed twice). */
export function renderedBlocksFor(id: number, text: string): BlockNode[] {
  const key = `${id}\u0000${text}`;
  const hit = cache.get(key);
  if (hit) return hit;
  const blocks = parseMarkdown(text);
  if (cache.size >= CACHE_LIMIT) {
    const oldest = cache.keys().next();
    if (!oldest.done) cache.delete(oldest.value);
  }
  cache.set(key, blocks);
  return blocks;
}
