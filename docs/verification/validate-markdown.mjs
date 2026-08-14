#!/usr/bin/env node

import path from "node:path";
import process from "node:process";
import {
  collectAllMarkdownFiles,
  collectHeadings,
  detectUnclosedFence,
  parseMarkdown,
  tryReadUtf8,
} from "./markdown.mjs";

function report(failures, file, line, message) {
  const relative = path.relative(process.cwd(), file);
  failures.push(
    line === null
      ? `${relative}: ${message}`
      : `${relative}:${line}: ${message}`,
  );
}

async function main() {
  const inputs = process.argv.slice(2);
  if (inputs.length === 0) {
    throw new Error(
      "usage: node docs/verification/validate-markdown.mjs <file-or-directory> [...]",
    );
  }

  const files = await collectAllMarkdownFiles(inputs);
  const failures = [];

  for (const file of files) {
    const { source, valid } = await tryReadUtf8(file);
    if (!valid) {
      report(failures, file, null, "is not valid UTF-8");
      continue;
    }

    if (!source.endsWith("\n")) {
      report(failures, file, null, "must end with a newline");
    }
    if (source.includes("\0")) {
      report(failures, file, null, "contains a NUL byte");
    }

    const unclosed = detectUnclosedFence(source);
    if (unclosed !== null) {
      report(
        failures,
        file,
        unclosed.line,
        "opens a fenced code block that is never closed",
      );
    }

    const lines = source.split(/\r?\n/);
    let fence = null;
    for (let index = 0; index < lines.length; index += 1) {
      const line = lines[index];
      const lineNumber = index + 1;
      const fenceMatch = line.match(/^ {0,3}(`{3,}|~{3,})(.*)$/);
      if (fenceMatch) {
        const marker = fenceMatch[1];
        if (fence === null) {
          fence = { character: marker[0], length: marker.length };
        } else if (
          marker[0] === fence.character &&
          marker.length >= fence.length &&
          fenceMatch[2].trim() === ""
        ) {
          fence = null;
        }
        continue;
      }
      if (fence !== null) continue;

      if (line.includes("\t")) {
        report(
          failures,
          file,
          lineNumber,
          "contains a tab outside a fenced code block",
        );
      }
      const trailing = line.match(/ +$/)?.[0] ?? "";
      if (trailing.length > 0 && trailing.length !== 2) {
        report(
          failures,
          file,
          lineNumber,
          "has trailing spaces other than the intentional two-space Markdown line break",
        );
      }
      if (/^\s{0,3}([-+*])(?!\1)(?=\S)/.test(line)) {
        report(
          failures,
          file,
          lineNumber,
          "unordered-list marker must be followed by a space",
        );
      }
      if (/^\s{0,3}\d+[.)](?=\S)/.test(line)) {
        report(
          failures,
          file,
          lineNumber,
          "ordered-list marker must be followed by a space",
        );
      }
    }

    let headings;
    try {
      headings = collectHeadings(parseMarkdown(source));
    } catch {
      report(failures, file, null, "failed to parse Markdown");
      continue;
    }

    let h1Count = 0;
    let previousLevel = null;
    const firstAnchors = new Map();
    for (const heading of headings) {
      if (heading.level === 1) h1Count += 1;
      if (previousLevel !== null && heading.level > previousLevel + 1) {
        report(
          failures,
          file,
          heading.line,
          `heading level jumps from H${previousLevel} to H${heading.level}`,
        );
      }
      previousLevel = heading.level;

      if (heading.baseAnchor.length === 0) {
        report(failures, file, heading.line, "heading has an empty anchor");
      }
      const firstLine = firstAnchors.get(heading.baseAnchor);
      if (firstLine !== undefined) {
        report(
          failures,
          file,
          heading.line,
          `duplicates heading anchor '${heading.baseAnchor}' first used on line ${firstLine}`,
        );
      } else {
        firstAnchors.set(heading.baseAnchor, heading.line);
      }
    }

    if (h1Count !== 1) {
      report(
        failures,
        file,
        null,
        `must contain exactly one H1 heading; found ${h1Count}`,
      );
    }
  }

  if (failures.length > 0) {
    for (const failure of failures) {
      console.error(`validate-markdown: ${failure}`);
    }
    process.exitCode = 1;
    return;
  }

  console.log(`validate-markdown: validated ${files.length} Markdown file(s)`);
}

main().catch((error) => {
  console.error(
    `validate-markdown: ${error instanceof Error ? error.message : String(error)}`,
  );
  process.exitCode = 1;
});
