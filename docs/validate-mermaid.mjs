#!/usr/bin/env node

import { readFile, readdir, stat } from "node:fs/promises";
import path from "node:path";
import process from "node:process";
import { JSDOM } from "jsdom";

const MARKDOWN_EXTENSIONS = new Set([".md", ".markdown"]);

function isLocalAdrArchive(directory) {
  return (
    path.basename(directory) === "archive" &&
    path.basename(path.dirname(directory)) === "adr"
  );
}

function compareNames(left, right) {
  return left < right ? -1 : left > right ? 1 : 0;
}

async function collectMarkdownFiles(input) {
  const absolute = path.resolve(input);
  let metadata;
  try {
    metadata = await stat(absolute);
  } catch (error) {
    throw new Error(`cannot access ${input}: ${error.message}`);
  }

  if (metadata.isFile()) {
    if (!MARKDOWN_EXTENSIONS.has(path.extname(absolute).toLowerCase())) {
      throw new Error(`not a Markdown file: ${input}`);
    }
    return [absolute];
  }

  if (!metadata.isDirectory()) {
    throw new Error(`unsupported input type: ${input}`);
  }

  const files = [];
  async function visit(directory) {
    const entries = await readdir(directory, { withFileTypes: true });
    entries.sort((left, right) => compareNames(left.name, right.name));

    for (const entry of entries) {
      const entryPath = path.join(directory, entry.name);
      if (entry.isDirectory() && !isLocalAdrArchive(entryPath)) {
        await visit(entryPath);
      } else if (
        entry.isFile()
        && MARKDOWN_EXTENSIONS.has(path.extname(entry.name).toLowerCase())
      ) {
        files.push(entryPath);
      }
    }
  }

  await visit(absolute);
  return files;
}

function isClosingFence(line, marker, minimumLength) {
  const candidate = line.replace(/^ {0,3}/, "").trimEnd();
  return (
    candidate.length >= minimumLength
    && [...candidate].every((character) => character === marker)
  );
}

function extractMermaidBlocks(source, file) {
  const lines = source.split(/\r?\n/);
  const blocks = [];

  for (let lineIndex = 0; lineIndex < lines.length; lineIndex += 1) {
    const opening = lines[lineIndex].match(
      /^ {0,3}(`{3,}|~{3,})[ \t]*mermaid(?:[ \t]+.*)?$/i,
    );
    if (!opening) continue;

    const marker = opening[1][0];
    const minimumLength = opening[1].length;
    const openingLine = lineIndex + 1;
    const body = [];
    let closed = false;

    for (lineIndex += 1; lineIndex < lines.length; lineIndex += 1) {
      if (isClosingFence(lines[lineIndex], marker, minimumLength)) {
        closed = true;
        break;
      }
      body.push(lines[lineIndex]);
    }

    if (!closed) {
      throw new Error(`${file}:${openingLine}: unclosed Mermaid fence`);
    }

    blocks.push({
      line: openingLine,
      source: body.join("\n"),
      firstLine: body.find((line) => line.trim())?.trim() ?? "",
    });
  }

  return blocks;
}

function formatError(error) {
  const message = error instanceof Error ? error.message : String(error);
  return message.replace(/\s+/g, " ").trim();
}

async function main() {
  const inputs = process.argv.slice(2);
  if (inputs.length === 0) {
    throw new Error("usage: node docs/validate-mermaid.mjs <file-or-directory> [...]");
  }

  const discovered = [];
  for (const input of inputs) {
    discovered.push(...(await collectMarkdownFiles(input)));
  }

  const files = [...new Set(discovered)].sort(compareNames);
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
    let blocks;

    try {
      blocks = extractMermaidBlocks(await readFile(file, "utf8"), relativeFile);
    } catch (error) {
      failures.push(formatError(error));
      continue;
    }

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
