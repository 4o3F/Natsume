#!/usr/bin/env node
import { readFile, readdir, stat } from "node:fs/promises";
import path from "node:path";
import process from "node:process";

const MARKDOWN_EXTENSIONS = new Set([".md", ".markdown"]);

async function collect(input) {
  const absolute = path.resolve(input);
  const metadata = await stat(absolute);
  if (metadata.isFile()) return MARKDOWN_EXTENSIONS.has(path.extname(absolute).toLowerCase()) ? [absolute] : [];
  const files = [];
  async function visit(dir) {
    const entries = await readdir(dir, { withFileTypes: true });
    entries.sort((a, b) => a.name.localeCompare(b.name));
    for (const entry of entries) {
      const p = path.join(dir, entry.name);
      if (entry.isDirectory()) await visit(p);
      else if (entry.isFile() && MARKDOWN_EXTENSIONS.has(path.extname(entry.name).toLowerCase())) files.push(p);
    }
  }
  await visit(absolute);
  return files;
}

function stripCode(source) {
  return source
    .replace(/```[\s\S]*?```/g, "")
    .replace(/~~~[\s\S]*?~~~/g, "")
    .replace(/`[^`\n]*`/g, "");
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

function headings(source) {
  const result = new Set();
  const counts = new Map();
  for (const line of source.split(/\r?\n/)) {
    const match = line.match(/^ {0,3}#{1,6}\s+(.+?)\s*#*\s*$/);
    if (!match) continue;
    const base = slugify(match[1]);
    const count = counts.get(base) ?? 0;
    counts.set(base, count + 1);
    result.add(count === 0 ? base : `${base}-${count}`);
  }
  return result;
}

function decodeTarget(value) {
  return decodeURIComponent(value.replace(/&amp;/g, "&"));
}

async function main() {
  const inputs = process.argv.slice(2);
  if (inputs.length === 0) throw new Error("usage: node docs/verification/validate-links.mjs <file-or-directory> [...]");
  const files = [...new Set((await Promise.all(inputs.map(collect))).flat())].sort();
  const headingCache = new Map();
  const failures = [];
  let linkCount = 0;

  async function getHeadings(file) {
    if (!headingCache.has(file)) headingCache.set(file, headings(await readFile(file, "utf8")));
    return headingCache.get(file);
  }

  for (const file of files) {
    const source = await readFile(file, "utf8");
    const visible = stripCode(source);
    const regex = /!?\[[^\]]*]\(([^)\s]+)(?:\s+["'][^"']*["'])?\)/g;
    for (const match of visible.matchAll(regex)) {
      const raw = match[1].replace(/^<|>$/g, "");
      if (/^(https?:|mailto:|tel:|data:)/i.test(raw)) continue;
      linkCount += 1;
      const [rawPath, rawFragment] = raw.split("#", 2);
      const targetPath = rawPath ? path.resolve(path.dirname(file), decodeTarget(rawPath)) : file;
      try {
        const metadata = await stat(targetPath);
        if (metadata.isDirectory()) {
          const readme = path.join(targetPath, "README.md");
          await stat(readme);
          if (rawFragment) {
            const hs = await getHeadings(readme);
            if (!hs.has(decodeTarget(rawFragment).toLowerCase())) {
              failures.push(`${path.relative(process.cwd(), file)}: missing heading #${rawFragment} in ${path.relative(process.cwd(), readme)}`);
            }
          }
        } else if (rawFragment && MARKDOWN_EXTENSIONS.has(path.extname(targetPath).toLowerCase())) {
          const hs = await getHeadings(targetPath);
          if (!hs.has(decodeTarget(rawFragment).toLowerCase())) {
            failures.push(`${path.relative(process.cwd(), file)}: missing heading #${rawFragment} in ${path.relative(process.cwd(), targetPath)}`);
          }
        }
      } catch {
        failures.push(`${path.relative(process.cwd(), file)}: missing target ${raw}`);
      }
    }
  }
  if (failures.length > 0) {
    for (const failure of failures) console.error(`validate-links: ${failure}`);
    process.exitCode = 1;
  } else {
    console.log(`validate-links: validated ${linkCount} relative link(s) across ${files.length} Markdown file(s)`);
  }
}

main().catch((error) => {
  console.error(`validate-links: ${error instanceof Error ? error.message : String(error)}`);
  process.exitCode = 1;
});
