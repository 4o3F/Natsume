#!/usr/bin/env node

import { readFile } from "node:fs/promises";
import path from "node:path";
import process from "node:process";
import { JSDOM } from "jsdom";
import { visit } from "unist-util-visit";
import {
  collectAllMarkdownFiles,
  detectUnclosedFence,
  parseMarkdown,
} from "./verification/markdown.mjs";

function formatError(error) {
  const message = error instanceof Error ? error.message : String(error);
  return message.replace(/\s+/g, " ").trim();
}

async function main() {
  const inputs = process.argv.slice(2);
  if (inputs.length === 0) {
    throw new Error(
      "usage: node docs/validate-mermaid.mjs <file-or-directory> [...]",
    );
  }

  const files = await collectAllMarkdownFiles(inputs);
  if (files.length === 0) {
    throw new Error("no Markdown files found in the requested inputs");
  }

  const dom = new JSDOM("");
  globalThis.window = dom.window;
  globalThis.document = dom.window.document;

  const { default: mermaid } = await import("mermaid");
  mermaid.initialize({ securityLevel: "strict", startOnLoad: false });

  let diagramCount = 0;
  const failures = [];

  for (const file of files) {
    const relativeFile = path.relative(process.cwd(), file) || file;
    const source = await readFile(file, "utf8");
    const unclosed = detectUnclosedFence(source);
    if (unclosed?.info.split(/\s+/, 1)[0]?.toLowerCase() === "mermaid") {
      failures.push(`${relativeFile}:${unclosed.line}: unclosed Mermaid fence`);
      continue;
    }

    let tree;
    try {
      tree = parseMarkdown(source);
    } catch {
      failures.push(`${relativeFile}: failed to parse Markdown`);
      continue;
    }

    const blocks = [];
    visit(tree, "code", (node) => {
      if (node.lang?.toLowerCase() === "mermaid") {
        blocks.push({
          line: node.position?.start.line ?? 1,
          source: node.value,
          firstLine:
            node.value
              .split(/\r?\n/)
              .find((line) => line.trim())
              ?.trim() ?? "",
        });
      }
    });

    for (const [index, block] of blocks.entries()) {
      diagramCount += 1;
      try {
        await mermaid.parse(block.source);
      } catch (error) {
        failures.push(
          `${relativeFile}:${block.line}: block ${index + 1} (${block.firstLine}): ${formatError(error)}`,
        );
      }
    }
  }

  if (diagramCount === 0) {
    failures.push("no Mermaid fences found in the requested inputs");
  }

  if (failures.length > 0) {
    for (const failure of failures) {
      console.error(`validate-mermaid: ${failure}`);
    }
    process.exitCode = 1;
    return;
  }

  console.log(
    `validate-mermaid: validated ${diagramCount} diagram(s) across ${files.length} Markdown file(s)`,
  );
}

main().catch((error) => {
  console.error(`validate-mermaid: ${formatError(error)}`);
  process.exitCode = 1;
});
