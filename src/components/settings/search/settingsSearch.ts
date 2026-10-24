import { SECTION_LABEL_KEY } from "./settingsSearchIndex";
import type {
  MatchRange,
  Resolve,
  SearchContext,
  SearchEntry,
  SearchHit,
  TextPair,
} from "./types";

const MAX_QUERY_CHARS = 100;
const DEFAULT_LIMIT = 30;
const MAX_UNLOCK_DEPTH = 3;

const SCORE_LABEL_WORD_PREFIX = 100;
const SCORE_LABEL_SUBSTRING = 70;
const SCORE_KEYWORD_WORD_PREFIX = 60;
const SCORE_KEYWORD_SUBSTRING = 45;
const SCORE_GROUP_OR_SECTION = 30;
const SCORE_DESCRIPTION = 15;
const SCORE_LABEL_STARTS_WITH_QUERY = 25;

// Fixed expressions only. Nothing here is ever built from user input.
const COMBINING_MARKS = /\p{M}/gu;
const WHITESPACE_RUN = /\s+/gu;
const WHITESPACE = /\s/u;
const WORD_CHARACTER = /[\p{L}\p{N}]/u;

interface Gate {
  locked: boolean;
  unlockChain: readonly string[];
}

interface SearchFields {
  labels: readonly string[];
  keywords: readonly string[];
  groups: readonly string[];
  descriptions: readonly string[];
}

/** NFKD, strip combining marks, lowercase. No whitespace handling. */
function fold(text: string): string {
  return text.normalize("NFKD").replace(COMBINING_MARKS, "").toLowerCase();
}

export function normalize(text: string): string {
  return fold(text).replace(WHITESPACE_RUN, " ").trim();
}

function capQuery(query: string): string {
  return Array.from(normalize(query)).slice(0, MAX_QUERY_CHARS).join("");
}

function tokenize(normalizedQuery: string): string[] {
  return normalizedQuery.split(" ").filter((token) => token.length > 0);
}

function lastCodePoint(text: string): string {
  return Array.from(text).pop() ?? "";
}

/** True when the token occurs at index 0 or right after a code point that is not a letter or digit. */
function hasWordPrefix(field: string, token: string): boolean {
  let index = field.indexOf(token);
  while (index !== -1) {
    if (
      index === 0 ||
      !WORD_CHARACTER.test(lastCodePoint(field.slice(0, index)))
    ) {
      return true;
    }
    index = field.indexOf(token, index + 1);
  }
  return false;
}

function scoreField(
  field: string,
  token: string,
  wordPrefixScore: number,
  substringScore: number,
): number {
  if (!field.includes(token)) return 0;
  return hasWordPrefix(field, token) ? wordPrefixScore : substringScore;
}

function bestTokenScore(token: string, fields: SearchFields): number {
  let best = 0;
  for (const label of fields.labels) {
    best = Math.max(
      best,
      scoreField(label, token, SCORE_LABEL_WORD_PREFIX, SCORE_LABEL_SUBSTRING),
    );
  }
  for (const keyword of fields.keywords) {
    best = Math.max(
      best,
      scoreField(
        keyword,
        token,
        SCORE_KEYWORD_WORD_PREFIX,
        SCORE_KEYWORD_SUBSTRING,
      ),
    );
  }
  for (const group of fields.groups) {
    best = Math.max(
      best,
      scoreField(group, token, SCORE_GROUP_OR_SECTION, SCORE_GROUP_OR_SECTION),
    );
  }
  for (const description of fields.descriptions) {
    best = Math.max(
      best,
      scoreField(description, token, SCORE_DESCRIPTION, SCORE_DESCRIPTION),
    );
  }
  return best;
}

function textsOf(pair: TextPair): string[] {
  return [normalize(pair.local), normalize(pair.en)];
}

function fieldsOf(entry: SearchEntry, resolve: Resolve): SearchFields {
  const groupKeys = [SECTION_LABEL_KEY[entry.section]];
  if (entry.groupKey !== undefined) groupKeys.push(entry.groupKey);
  return {
    labels: textsOf(resolve(entry.labelKey)),
    keywords: (entry.keywords ?? []).map(normalize),
    groups: groupKeys.flatMap((key) => textsOf(resolve(key))),
    descriptions:
      entry.descriptionKey === undefined
        ? []
        : textsOf(resolve(entry.descriptionKey)),
  };
}

/**
 * True when the entry can be used now. A row that needs the Experimental switch is not
 * reachable while the switch is off, even when its page is always visible.
 */
function isReachable(entry: SearchEntry, ctx: SearchContext): boolean {
  if (
    entry.requiresExperimental === true &&
    ctx.experimentalEnabled === false
  ) {
    return false;
  }
  return ctx.isSectionEnabled(entry.section);
}

/** Returns null when the entry must be dropped because of its gating. */
function gateEntry(
  entry: SearchEntry,
  byId: ReadonlyMap<string, SearchEntry>,
  ctx: SearchContext,
): Gate | null {
  if (isReachable(entry, ctx)) {
    return { locked: false, unlockChain: [] };
  }
  if (entry.unlockedBy === undefined) return null;

  const chain: string[] = [];
  const seen = new Set<string>([entry.id]);
  let nextId: string | undefined = entry.unlockedBy;
  while (nextId !== undefined) {
    if (seen.has(nextId)) return null;
    seen.add(nextId);
    const enabler = byId.get(nextId);
    if (enabler === undefined) return null;
    chain.push(nextId);
    if (chain.length > MAX_UNLOCK_DEPTH) return null;
    if (isReachable(enabler, ctx)) {
      return { locked: true, unlockChain: chain };
    }
    nextId = enabler.unlockedBy;
  }
  return null;
}

/** Entry score, or null when some token matches no field. */
function scoreEntry(
  fields: SearchFields,
  tokens: readonly string[],
  normalizedQuery: string,
): number | null {
  let total = 0;
  for (const token of tokens) {
    const tokenScore = bestTokenScore(token, fields);
    if (tokenScore === 0) return null;
    total += tokenScore;
  }
  if (fields.labels.some((label) => label.startsWith(normalizedQuery))) {
    total += SCORE_LABEL_STARTS_WITH_QUERY;
  }
  return total;
}

export function search(
  entries: readonly SearchEntry[],
  query: string,
  resolve: Resolve,
  ctx: SearchContext,
  opts?: { limit?: number },
): SearchHit[] {
  const normalizedQuery = capQuery(query);
  const tokens = tokenize(normalizedQuery);
  if (tokens.length === 0) return [];

  const byId = new Map(entries.map((entry) => [entry.id, entry] as const));
  const limit = Math.max(0, opts?.limit ?? DEFAULT_LIMIT);
  const hits: SearchHit[] = [];

  for (const entry of entries) {
    if (
      entry.platforms !== undefined &&
      !entry.platforms.includes(ctx.platform)
    ) {
      continue;
    }
    const gate = gateEntry(entry, byId, ctx);
    if (gate === null) continue;
    const score = scoreEntry(fieldsOf(entry, resolve), tokens, normalizedQuery);
    if (score === null) continue;
    hits.push({
      entry,
      score,
      locked: gate.locked,
      unlockChain: gate.unlockChain,
    });
  }

  return hits.sort((a, b) => b.score - a.score).slice(0, limit);
}

export function resolveTarget(
  hit: SearchHit,
  entries: readonly SearchEntry[],
  ctx: SearchContext,
): SearchEntry {
  void ctx;
  if (!hit.locked) return hit.entry;
  const enablerId = hit.unlockChain[hit.unlockChain.length - 1];
  return entries.find((entry) => entry.id === enablerId) ?? hit.entry;
}

interface OffsetText {
  text: string;
  /** origin[k] = display code-point index that produced normalized code point k. */
  origin: number[];
}

function mapDisplay(display: string): OffsetText {
  const parts: string[] = [];
  const origin: number[] = [];
  Array.from(display).forEach((char, index) => {
    const folded = WHITESPACE.test(char) ? " " : fold(char);
    for (const normalizedChar of Array.from(folded)) {
      parts.push(normalizedChar);
      origin.push(index);
    }
  });
  return { text: parts.join(""), origin };
}

export function matchRange(display: string, query: string): MatchRange | null {
  const tokens = tokenize(capQuery(query));
  if (tokens.length === 0) return null;

  const mapped = mapDisplay(display);
  if (mapped.text !== normalize(display)) return null;

  for (const token of tokens) {
    const index = mapped.text.indexOf(token);
    if (index === -1) continue;
    const startCp = Array.from(mapped.text.slice(0, index)).length;
    const endCp = startCp + Array.from(token).length;
    const start = mapped.origin[startCp];
    const last = mapped.origin[endCp - 1];
    if (start === undefined || last === undefined) return null;
    return { start, end: last + 1 };
  }
  return null;
}
