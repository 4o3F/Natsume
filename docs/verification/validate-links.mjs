#!/usr/bin/env node

import { readFile, stat } from "node:fs/promises";
import path from "node:path";
import process from "node:process";
import { visit } from "unist-util-visit";
import {
  MARKDOWN_EXTENSIONS,
  collectAllMarkdownFiles,
  collectHeadings,
  parseMarkdown,
} from "./markdown.mjs";

function decodeTarget(value) {
  return decodeURIComponent(value.replace(/&amp;/g, "&"));
}

async function getHeadings(file, cache) {
  if (!cache.has(file)) {
    const tree = parseMarkdown(await readFile(file, "utf8"));
    cache.set(
      file,
      new Set(collectHeadings(tree).map((heading) => heading.anchor)),
    );
  }
  return cache.get(file);
}

async function main() {
  const inputs = process.argv.slice(2);
  if (inputs.length === 0) {
    throw new Error(
      "usage: node docs/verification/validate-links.mjs <file-or-directory> [...]",
    );
  }

  const files = await collectAllMarkdownFiles(inputs);
  const headingCache = new Map();
  const failures = [];
  let linkCount = 0;

  for (const file of files) {
    let tree;
    try {
      tree = parseMarkdown(await readFile(file, "utf8"));
    } catch {
      failures.push(
        `${path.relative(process.cwd(), file)}: failed to parse Markdown`,
      );
      continue;
    }

    const links = [];
    visit(tree, (node) => {
      if (node.type === "link" || node.type === "image") links.push(node.url);
    });

    for (const target of links) {
      const raw = target.replace(/^<|>$/g, "");
      if (/^(https?:|mailto:|tel:|data:)/i.test(raw)) continue;

      linkCount += 1;
      const [rawPath, rawFragment] = raw.split("#", 2);
      const targetPath = rawPath
        ? path.resolve(path.dirname(file), decodeTarget(rawPath))
        : file;

      try {
        const metadata = await stat(targetPath);
        if (metadata.isDirectory()) {
          const readme = path.join(targetPath, "README.md");
          await stat(readme);
          if (rawFragment) {
            const headings = await getHeadings(readme, headingCache);
            if (!headings.has(decodeTarget(rawFragment).toLowerCase())) {
              failures.push(
                `${path.relative(process.cwd(), file)}: missing heading #${rawFragment} in ${path.relative(process.cwd(), readme)}`,
              );
            }
          }
        } else if (
          rawFragment &&
          MARKDOWN_EXTENSIONS.has(path.extname(targetPath).toLowerCase())
        ) {
          const headings = await getHeadings(targetPath, headingCache);
          if (!headings.has(decodeTarget(rawFragment).toLowerCase())) {
            failures.push(
              `${path.relative(process.cwd(), file)}: missing heading #${rawFragment} in ${path.relative(process.cwd(), targetPath)}`,
            );
          }
        }
      } catch {
        failures.push(
          `${path.relative(process.cwd(), file)}: missing target ${raw}`,
        );
      }
    }
  }

  if (failures.length > 0) {
    for (const failure of failures) {
      console.error(`validate-links: ${failure}`);
    }
    process.exitCode = 1;
    return;
  }

  console.log(
    `validate-links: validated ${linkCount} relative link(s) across ${files.length} Markdown file(s)`,
  );
}

main().catch((error) => {
  console.error(
    `validate-links: ${error instanceof Error ? error.message : String(error)}`,
  );
  process.exitCode = 1;
});
