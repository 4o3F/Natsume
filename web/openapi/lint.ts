import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";
import spectralCore, { type RulesetDefinition } from "@stoplight/spectral-core";
import spectralParsers from "@stoplight/spectral-parsers";
import spectralRulesets from "@stoplight/spectral-rulesets";

const { Document, Spectral } = spectralCore;
const spectral = new Spectral();
spectral.setRuleset({
  extends: spectralRulesets.oas as RulesetDefinition,
  rules: {
    "info-contact": "off",
    "oas3-api-servers": "off",
    "operation-description": "off",
    "operation-tags": "off",
  },
});

const path = resolve(
  process.argv[2] ??
    fileURLToPath(new URL("natsume.openapi.json", import.meta.url)),
);
const document = new Document(
  readFileSync(path, "utf8"),
  spectralParsers.Json,
  path,
);
const diagnostics = await spectral.run(document);
for (const diagnostic of diagnostics) {
  const { line, character } = diagnostic.range.start;
  console.log(
    `${path}:${line + 1}:${character + 1} ${diagnostic.code}: ${diagnostic.message}`,
  );
}
// Match the former CLI's --fail-severity warn behavior.
process.exitCode = diagnostics.some((diagnostic) => diagnostic.severity <= 1)
  ? 1
  : 0;
