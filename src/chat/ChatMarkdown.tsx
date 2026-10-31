// Renders one reply body from the safe markdown AST (UI enterprise T2, DESIGN B7). Every piece of text
// is a React text node: there is no raw-HTML injection prop, no link, no image and no raw HTML. The
// parse is memoised per message (renderedBlocksFor), so a re-render never parses the same text again.
import { Fragment, type ReactNode } from "react";
import { renderedBlocksFor, type BlockNode, type InlineNode } from "./markdown";

function renderInline(nodes: InlineNode[]): ReactNode[] {
  return nodes.map((node, index) => {
    if (node.kind === "text")
      return <Fragment key={index}>{node.text}</Fragment>;
    if (node.kind === "code") {
      return (
        <code key={index} className="chat-code-inline">
          {node.text}
        </code>
      );
    }
    if (node.kind === "strong") {
      return <strong key={index}>{renderInline(node.children)}</strong>;
    }
    return <em key={index}>{renderInline(node.children)}</em>;
  });
}

function renderBlock(block: BlockNode, index: number): ReactNode {
  if (block.kind === "code") {
    // Focusable so keyboard users can scroll a long line; wrapping is off on purpose.
    return (
      <pre key={index} className="chat-code-block" tabIndex={0}>
        <code>{block.text}</code>
      </pre>
    );
  }
  if (block.kind === "list") {
    const items = block.items.map((lines, itemIndex) => (
      <li key={itemIndex}>
        {lines.map((line, lineIndex) => (
          <Fragment key={lineIndex}>
            {lineIndex > 0 ? <br /> : null}
            {renderInline(line)}
          </Fragment>
        ))}
      </li>
    ));
    return block.ordered ? (
      <ol key={index}>{items}</ol>
    ) : (
      <ul key={index}>{items}</ul>
    );
  }
  return (
    <p key={index}>
      {block.lines.map((line, lineIndex) => (
        <Fragment key={lineIndex}>
          {lineIndex > 0 ? <br /> : null}
          {renderInline(line)}
        </Fragment>
      ))}
    </p>
  );
}

export default function ChatMarkdown({
  id,
  text,
}: {
  id: number;
  text: string;
}) {
  const blocks = renderedBlocksFor(id, text);
  return <div className="chat-md">{blocks.map(renderBlock)}</div>;
}
