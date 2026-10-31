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
