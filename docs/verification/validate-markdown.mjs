#!/usr/bin/env node

import { readFile, readdir, stat } from "node:fs/promises";
import path from "node:path";
import process from "node:process";

const MARKDOWN_EXTENSIONS = new Set([".md", ".markdown"]);
const decoder = new TextDecoder("utf-8", { fatal: true });

async function collect(input) {
  const absolute = path.resolve(input);
  const metadata = await stat(absolute);
  if (metadata.isFile()) {
    return MARKDOWN_EXTENSIONS.has(path.extname(absolute).toLowerCase()) ? [absolute] : [];
  }

  const files = [];
  async function visit(directory) {
    const entries = await readdir(directory, { withFileTypes: true });
    entries.sort((left, right) => left.name.localeCompare(right.name));
    for (const entry of entries) {
      const candidate = path.join(directory, entry.name);
      if (entry.isDirectory()) await visit(candidate);
      else if (entry.isFile() && MARKDOWN_EXTENSIONS.has(path.extname(entry.name).toLowerCase())) files.push(candidate);
    }
  }
  await visit(absolute);
  return files;
}

function slugify(text) {
  return text
    .trim()
    .toLowerCase()
    .replace(/<[^>]+>/g, "")
    .replace(/[^\p{L}\p{N}\s\-_]/gu, "")
    .replace(/\s+/g, "-")
    .replace(/-+/g, "-");
}

function report(failures, file, line, message) {
  const location = line === null ? path.relative(process.cwd(), file) : `${path.relative(process.cwd(), file)}:${line}`;
  failures.push(`${location}: ${message}`);
}

async function main() {
  const inputs = process.argv.slice(2);
  if (inputs.length === 0) {
    throw new Error("usage: node docs/verification/validate-markdown.mjs <file-or-directory> [...]");
  }

  const files = [...new Set((await Promise.all(inputs.map(collect))).flat())].sort();
  const failures = [];

  for (const file of files) {
    let source;
    try {
      source = decoder.decode(await readFile(file));
    } catch {
      report(failures, file, null, "is not valid UTF-8");
      continue;
    }

    if (!source.endsWith("\n")) report(failures, file, null, "must end with a newline");
    if (source.includes("\0")) report(failures, file, null, "contains a NUL byte");

    const lines = source.split(/\r?\n/);
    const slugs = new Map();
    let h1Count = 0;
    let previousHeadingLevel = null;
    let fence = null;

    for (let index = 0; index < lines.length; index += 1) {
      const line = lines[index];
      const lineNumber = index + 1;
      const fenceMatch = line.match(/^ {0,3}(`{3,}|~{3,})(.*)$/);

      if (fenceMatch) {
        const marker = fenceMatch[1];
        if (fence === null) {
          fence = { character: marker[0], length: marker.length, line: lineNumber };
        } else if (marker[0] === fence.character && marker.length >= fence.length && fenceMatch[2].trim() === "") {
          fence = null;
        }
        continue;
      }

      if (fence !== null) continue;

      if (line.includes("\t")) report(failures, file, lineNumber, "contains a tab outside a fenced code block");

      const trailing = line.match(/ +$/)?.[0] ?? "";
      if (trailing.length > 0 && trailing.length !== 2) {
        report(failures, file, lineNumber, "has trailing spaces other than the intentional two-space Markdown line break");
      }

      if (/^\s{0,3}([-+*])(?!\1)(?=\S)/.test(line)) {
        report(failures, file, lineNumber, "unordered-list marker must be followed by a space");
      }
      if (/^\s{0,3}\d+[.)](?=\S)/.test(line)) {
        report(failures, file, lineNumber, "ordered-list marker must be followed by a space");
      }

      const heading = line.match(/^ {0,3}(#{1,6})\s+(.+?)\s*#*\s*$/);
      if (!heading) continue;

      const level = heading[1].length;
      if (level === 1) h1Count += 1;
      if (previousHeadingLevel !== null && level > previousHeadingLevel + 1) {
        report(failures, file, lineNumber, `heading level jumps from H${previousHeadingLevel} to H${level}`);
      }
      previousHeadingLevel = level;

      const slug = slugify(heading[2]);
      if (slug.length === 0) report(failures, file, lineNumber, "heading has an empty anchor");
      const firstLine = slugs.get(slug);
      if (firstLine !== undefined) {
        report(failures, file, lineNumber, `duplicates heading anchor '${slug}' first used on line ${firstLine}`);
      } else {
        slugs.set(slug, lineNumber);
      }
    }

    if (fence !== null) report(failures, file, fence.line, "opens a fenced code block that is never closed");
    if (h1Count !== 1) report(failures, file, null, `must contain exactly one H1 heading; found ${h1Count}`);
  }

  if (failures.length > 0) {
    for (const failure of failures) console.error(`validate-markdown: ${failure}`);
    process.exitCode = 1;
    return;
  }

  console.log(`validate-markdown: validated ${files.length} Markdown file(s)`);
}

main().catch((error) => {
  console.error(`validate-markdown: ${error instanceof Error ? error.message : String(error)}`);
  process.exitCode = 1;
});
