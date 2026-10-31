// Safe markdown parser (UI enterprise T2, DESIGN B7). Pure: no DOM, no React.
import { describe, expect, it } from "vitest";
import {
  parseInline,
  parseMarkdown,
  renderedBlocksFor,
  type BlockNode,
  type InlineNode,
} from "./markdown";

const text = (value: string): InlineNode => ({ kind: "text", text: value });

function plain(nodes: InlineNode[]): string {
  return nodes
    .map((n) => {
      if (n.kind === "text" || n.kind === "code") return n.text;
      return plain(n.children);
    })
    .join("");
}

function allText(blocks: BlockNode[]): string {
  return blocks
    .map((b) => {
      if (b.kind === "code") return b.text;
      if (b.kind === "paragraph") return b.lines.map(plain).join("\n");
      return b.items
        .map((item) => item.map((l) => plain(l)).join("\n"))
        .join("\n");
    })
    .join("\n");
}

describe("parseMarkdown: blocks", () => {
  it("returns no blocks for an empty string or only whitespace", () => {
    expect(parseMarkdown("")).toEqual([]);
    expect(parseMarkdown("   \n\t\n  ")).toEqual([]);
  });

  it("makes plain text one paragraph and keeps single newlines as lines", () => {
    expect(parseMarkdown("hello\nworld")).toEqual([
      { kind: "paragraph", lines: [[text("hello")], [text("world")]] },
    ]);
  });

  it("splits paragraphs on blank lines", () => {
    const blocks = parseMarkdown("one\n\ntwo");
    expect(blocks).toHaveLength(2);
    expect(blocks.map((b) => b.kind)).toEqual(["paragraph", "paragraph"]);
  });

  it("parses bullet lists with -, * and + markers", () => {
    const blocks = parseMarkdown("- a\n* b\n+ c");
    expect(blocks).toEqual([
      {
        kind: "list",
        ordered: false,
        items: [[[text("a")]], [[text("b")]], [[text("c")]]],
      },
    ]);
  });

  it("parses numbered lists with 1. and 1) markers", () => {
    const blocks = parseMarkdown("1. first\n2) second");
    expect(blocks).toEqual([
      {
        kind: "list",
        ordered: true,
        items: [[[text("first")]], [[text("second")]]],
      },
    ]);
  });

  it("does not read **bold** at the start of a line as a bullet", () => {
    const blocks = parseMarkdown("**bold** text");
    expect(blocks[0]?.kind).toBe("paragraph");
  });

  it("parses a fenced code block with a language and keeps its text literal", () => {
    const blocks = parseMarkdown(
      "```ts\nconst a = <b>*x*</b>;\n- not a list\n```",
    );
    expect(blocks).toEqual([
      {
        kind: "code",
        lang: "ts",
        text: "const a = <b>*x*</b>;\n- not a list",
      },
    ]);
  });

  it("runs an unclosed fence to the end of the message", () => {
    const blocks = parseMarkdown("intro\n```\nline one\nline two");
    expect(blocks[0]?.kind).toBe("paragraph");
    expect(blocks[1]).toEqual({
      kind: "code",
      lang: "",
      text: "line one\nline two",
    });
  });

  it("treats a fence line with a backtick in its info string as plain text", () => {
    const blocks = parseMarkdown("```a`b\nbody");
    expect(blocks.some((b) => b.kind === "code")).toBe(false);
  });

  it("a list inside a code fence stays literal", () => {
    const blocks = parseMarkdown("```\n- one\n- two\n```");
    expect(blocks).toEqual([{ kind: "code", lang: "", text: "- one\n- two" }]);
  });

  it("keeps a closing fence with more backticks as the close", () => {
    const blocks = parseMarkdown("```\ncode\n````");
    expect(blocks).toEqual([{ kind: "code", lang: "", text: "code" }]);
  });

  it("does not parse HTML tags into any node", () => {
    const blocks = parseMarkdown("<script>alert(1)</script>");
    expect(allText(blocks)).toBe("<script>alert(1)</script>");
    expect(JSON.stringify(blocks)).not.toMatch(/"kind":"(html|link|image)"/);
  });
});

describe("parseInline: spans", () => {
  it("parses inline code, bold and italic", () => {
    expect(parseInline("a `x` **b** *c*")).toEqual([
      text("a "),
      { kind: "code", text: "x" },
      text(" "),
      { kind: "strong", children: [text("b")] },
      text(" "),
      { kind: "em", children: [text("c")] },
    ]);
  });

  it("handles nested backticks: a longer run wraps a shorter one", () => {
    expect(parseInline("``a`b``")).toEqual([{ kind: "code", text: "a`b" }]);
  });

  it("keeps an unmatched backtick run as literal text", () => {
    expect(parseInline("a ` b")).toEqual([text("a ` b")]);
  });

  it("keeps unbalanced ** and * as literal text", () => {
    expect(plain(parseInline("**open only"))).toBe("**open only");
    expect(plain(parseInline("a * b"))).toBe("a * b");
    expect(plain(parseInline("2 * 3 * 4"))).toBe("2 * 3 * 4");
  });

  it("does not parse links: URLs and markdown link syntax stay inert text", () => {
    const nodes = parseInline(
      "see https://example.com and [x](https://evil.test)",
    );
    expect(nodes.every((n) => n.kind === "text")).toBe(true);
    expect(plain(nodes)).toBe(
      "see https://example.com and [x](https://evil.test)",
    );
  });

  it("keeps an img onerror tag as text", () => {
    const nodes = parseInline('<img src=x onerror="alert(1)">');
    expect(nodes).toEqual([text('<img src=x onerror="alert(1)">')]);
  });

  it("returns no nodes for an empty line", () => {
    expect(parseInline("")).toEqual([]);
  });
});

describe("parseMarkdown: robustness and budget", () => {
  it("parses a 100k-character single line without error", () => {
    const line = "word ".repeat(20_000);
    const started = performance.now();
    const blocks = parseMarkdown(line);
    expect(performance.now() - started).toBeLessThan(500);
    expect(blocks).toHaveLength(1);
  });

  it("parses a 100k-character line of backticks and stars without blowing up", () => {
    const line = "`*".repeat(50_000);
    const started = performance.now();
    parseMarkdown(line);
    expect(performance.now() - started).toBeLessThan(500);
  });

  it("parses a 5k-line message", () => {
    const body = Array.from({ length: 5000 }, (_, i) => `line ${i} **x**`).join(
      "\n",
    );
    const started = performance.now();
    const blocks = parseMarkdown(body);
    expect(performance.now() - started).toBeLessThan(500);
    expect(blocks).toHaveLength(1);
  });

  it("parses 1000 messages in under 200 ms", () => {
    const messages = Array.from(
      { length: 1000 },
      (_, i) =>
        `**Message ${i}**\n- item one\n- item two\n\`\`\`\ncode ${i}\n\`\`\``,
    );
    parseMarkdown(messages[0] ?? "");
    const started = performance.now();
    for (const message of messages) parseMarkdown(message);
    expect(performance.now() - started).toBeLessThan(200);
  });

  it("memoises per id and text: the same object comes back for the same message", () => {
    const first = renderedBlocksFor(7001, "**hi**");
    expect(renderedBlocksFor(7001, "**hi**")).toBe(first);
    expect(renderedBlocksFor(7001, "**changed**")).not.toBe(first);
  });
});
