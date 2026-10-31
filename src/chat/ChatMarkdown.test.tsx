// Static markup for the reply body (UI enterprise T2, DESIGN B7). renderToStaticMarkup runs no
// effects; it proves what reaches the DOM: text nodes only, no raw HTML, no links, no images.
import { describe, expect, it } from "vitest";
import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import ChatMarkdown from "./ChatMarkdown";

function html(id: number, body: string): string {
  return renderToStaticMarkup(createElement(ChatMarkdown, { id, text: body }));
}

describe("ChatMarkdown static markup", () => {
  it("renders bold, lists, a code block and inline code as elements", () => {
    const out = html(
      1,
      "**Bold** and `code`\n\n- one\n- two\n\n```js\nlet a;\n```",
    );
    expect(out).toContain("<strong>Bold</strong>");
    expect(out).toContain('<code class="chat-code-inline">code</code>');
    expect(out).toMatch(/<ul[^>]*>.*<li>.*one.*<\/li>.*<\/ul>/);
    expect(out).toMatch(/<pre class="chat-code-block"[^>]*>.*let a;.*<\/pre>/);
  });

  it("keeps raw HTML and an img onerror as escaped text, never an element", () => {
    const out = html(
      2,
      '<img src=x onerror="alert(1)"> <script>alert(2)</script>',
    );
    expect(out).not.toMatch(/<img\b/i);
    expect(out).not.toMatch(/<script\b/i);
    expect(out).toContain("&lt;img");
  });

  it("renders URLs and markdown links as inert text, not anchors", () => {
    const out = html(3, "https://example.com [x](https://evil.test)");
    expect(out).not.toMatch(/<a\b/i);
    expect(out).toContain("https://example.com");
  });

  it("renders nothing for an empty or whitespace-only body", () => {
    expect(html(4, "")).not.toMatch(/<(p|ul|ol|pre)\b/);
    expect(html(5, "   \n  ")).not.toMatch(/<(p|ul|ol|pre)\b/);
  });

  it("strips bidi overrides and zero-width characters before parsing, keeping ZWJ emoji", () => {
    const out = html(
      6,
      "pay\u202Eevil\u202C \u2066x\u2069 zero\u200Bwidth \u{1F468}\u200D\u{1F469}",
    );
    expect(out).not.toMatch(/[\u202A-\u202E\u2066-\u2069\u200B]/);
    expect(out).toContain("payevil");
    expect(out).toContain("\u{1F468}\u200D\u{1F469}");
  });

  it("renders inline bold, italic and code in a single-line reply without inventing a list or fence", () => {
    // The Rust log collapses a spoken reply to one line, so this is the common real shape.
    const out = html(7, "Run `npm test` for **all** tests, then *check* it.");
    expect(out).toContain('<code class="chat-code-inline">npm test</code>');
    expect(out).toContain("<strong>all</strong>");
    expect(out).toContain("<em>check</em>");
    expect(out).not.toMatch(/<(ul|ol|li|pre)\b/);
  });

  it("parses and renders 1000 messages under the budget", () => {
    const messages = Array.from(
      { length: 1000 },
      (_, i) => `**Reply ${i}**\n- a\n- b\n\`\`\`\nx ${i}\n\`\`\``,
    );
    html(9000, messages[0] ?? "");
    const started = performance.now();
    messages.forEach((body, i) => html(9001 + i, body));
    expect(performance.now() - started).toBeLessThan(500);
  });
});
