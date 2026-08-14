#!/usr/bin/env node

import { readFile, readdir, stat } from "node:fs/promises";
import path from "node:path";
import GithubSlugger, { slug } from "github-slugger";
import { toString } from "mdast-util-to-string";
import remarkGfm from "remark-gfm";
import remarkParse from "remark-parse";
import { unified } from "unified";
import { visit } from "unist-util-visit";

export const MARKDOWN_EXTENSIONS = new Set([".md", ".markdown"]);

const decoder = new TextDecoder("utf-8", { fatal: true });
const markdown = unified().use(remarkParse).use(remarkGfm).freeze();

export function isLocalAdrArchive(directory) {
  return (
    path.basename(directory) === "archive" &&
    path.basename(path.dirname(directory)) === "adr"
  );
}

export function compareNames(left, right) {
  return left < right ? -1 : left > right ? 1 : 0;
}

export async function collectMarkdownFiles(input) {
  const absolute = path.resolve(input);
  const metadata = await stat(absolute);

  if (metadata.isFile()) {
    return MARKDOWN_EXTENSIONS.has(path.extname(absolute).toLowerCase())
      ? [absolute]
      : [];
  }
  if (!metadata.isDirectory()) return [];

  const files = [];
  async function collect(directory) {
    const entries = await readdir(directory, { withFileTypes: true });
    entries.sort((left, right) => compareNames(left.name, right.name));
    for (const entry of entries) {
      const entryPath = path.join(directory, entry.name);
      if (entry.isDirectory() && !isLocalAdrArchive(entryPath)) {
        await collect(entryPath);
      } else if (
        entry.isFile() &&
        MARKDOWN_EXTENSIONS.has(path.extname(entry.name).toLowerCase())
      ) {
        files.push(entryPath);
      }
    }
  }

  await collect(absolute);
  return files;
}

export async function collectAllMarkdownFiles(inputs) {
  const discovered = [];
  for (const input of inputs) {
    discovered.push(...(await collectMarkdownFiles(input)));
  }
  return [...new Set(discovered)].sort(compareNames);
}

export function parseMarkdown(source) {
  return markdown.parse(source);
}

export async function tryReadUtf8(file) {
  try {
    return { source: decoder.decode(await readFile(file)), valid: true };
  } catch {
    return { source: null, valid: false };
  }
}

export function collectHeadings(tree) {
  const slugger = new GithubSlugger();
  const headings = [];
  visit(tree, "heading", (node) => {
    const text = toString(node);
    headings.push({
      text,
      level: node.depth,
      line: node.position?.start.line ?? null,
      baseAnchor: slug(text),
      anchor: slugger.slug(text),
    });
  });
  return headings;
}

export function detectUnclosedFence(source) {
  const lines = source.split(/\r?\n/);
  let fence = null;

  for (let index = 0; index < lines.length; index += 1) {
    const match = lines[index].match(/^ {0,3}(`{3,}|~{3,})(.*)$/);
    if (!match) continue;

    const marker = match[1];
    if (fence === null) {
      fence = {
        character: marker[0],
        length: marker.length,
        line: index + 1,
        info: match[2].trim(),
      };
    } else if (
      marker[0] === fence.character &&
      marker.length >= fence.length &&
      match[2].trim() === ""
    ) {
      fence = null;
    }
  }

  return fence;
}
