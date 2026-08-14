#!/usr/bin/env node

import assert from "node:assert/strict";
import { describe, it } from "node:test";
import { visit } from "unist-util-visit";
import {
  collectHeadings,
  detectUnclosedFence,
  parseMarkdown,
} from "./markdown.mjs";

describe("Markdown AST helpers", () => {
  it("collects heading levels, text, positions, and GitHub anchors", () => {
    const headings = collectHeadings(
      parseMarkdown("# Root\n\n## Inline `code` heading\n"),
    );
    assert.deepStrictEqual(headings, [
      {
        text: "Root",
        level: 1,
        line: 1,
        baseAnchor: "root",
        anchor: "root",
      },
      {
        text: "Inline code heading",
        level: 2,
        line: 3,
        baseAnchor: "inline-code-heading",
        anchor: "inline-code-heading",
      },
    ]);
  });

  it("separates duplicate base anchors from headings that already end in numbers", () => {
    const duplicate = collectHeadings(parseMarkdown("# Foo\n\n## Foo\n"));
    assert.deepStrictEqual(
      duplicate.map(({ baseAnchor, anchor }) => ({ baseAnchor, anchor })),
      [
        { baseAnchor: "foo", anchor: "foo" },
        { baseAnchor: "foo", anchor: "foo-1" },
      ],
    );

    const distinct = collectHeadings(parseMarkdown("# Foo\n\n## Foo-1\n"));
    assert.deepStrictEqual(
      distinct.map(({ baseAnchor, anchor }) => ({ baseAnchor, anchor })),
      [
        { baseAnchor: "foo", anchor: "foo" },
        { baseAnchor: "foo-1", anchor: "foo-1" },
      ],
    );
  });

  it("produces an empty base anchor for punctuation-only headings", () => {
    const [heading] = collectHeadings(parseMarkdown("# ???\n"));
    assert.strictEqual(heading.baseAnchor, "");
  });

  it("does not create link nodes for fenced or inline code", () => {
    const tree = parseMarkdown(
      "```text\n[hidden](hidden.md)\n```\n\n`[also hidden](inline.md)`\n\n[visible](visible.md)\n",
    );
    const links = [];
    visit(tree, "link", (node) => links.push(node.url));
    assert.deepStrictEqual(links, ["visible.md"]);
  });

  it("creates link and image nodes with source positions", () => {
    const tree = parseMarkdown("[link](file.md)\n\n![image](image.png)\n");
    const targets = [];
    visit(tree, (node) => {
      if (node.type === "link" || node.type === "image") {
        targets.push({
          type: node.type,
          url: node.url,
          line: node.position.start.line,
        });
      }
    });
    assert.deepStrictEqual(targets, [
      { type: "link", url: "file.md", line: 1 },
      { type: "image", url: "image.png", line: 3 },
    ]);
  });

  it("exposes Mermaid code values and opening lines", () => {
    const tree = parseMarkdown("text\n\n```mermaid\ngraph TD\n  A-->B\n```\n");
    const blocks = [];
    visit(tree, "code", (node) => {
      if (node.lang === "mermaid") {
        blocks.push({ value: node.value, line: node.position.start.line });
      }
    });
    assert.deepStrictEqual(blocks, [{ value: "graph TD\n  A-->B", line: 3 }]);
  });
});

describe("fence validation", () => {
  it("reports an unclosed fence with its line and info string", () => {
    assert.deepStrictEqual(detectUnclosedFence("```mermaid\ngraph TD\n"), {
      character: "`",
      length: 3,
      line: 1,
      info: "mermaid",
    });
  });

  it("accepts a matching fence of equal or greater length", () => {
    assert.strictEqual(detectUnclosedFence("```text\nvalue\n````\n"), null);
  });

  it("does not close a backtick fence with a tilde fence", () => {
    assert.notStrictEqual(detectUnclosedFence("```text\nvalue\n~~~\n"), null);
  });
});
