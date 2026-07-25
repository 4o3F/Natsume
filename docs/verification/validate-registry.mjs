#!/usr/bin/env node

import { access, readFile } from "node:fs/promises";
import path from "node:path";
import process from "node:process";
import { fileURLToPath } from "node:url";

const scriptDir = path.dirname(fileURLToPath(import.meta.url));
const docsDir = path.resolve(scriptDir, "..");
const repoRoot = path.resolve(docsDir, "..");
const registryPath = path.join(scriptDir, "registry.json");
const invariantPath = path.join(docsDir, "security-recovery.md");
const PROBE_STATUSES = new Set(["NOT-RUN", "RUNNING", "PASS", "FAIL", "BLOCKED-INPUT"]);
const SYNTHETIC_PROBES = new Set(["CI", "DOC"]);

function fail(failures, message) {
  failures.push(message);
}

function nonEmptyString(value) {
  return typeof value === "string" && value.trim().length > 0;
}

function assertArray(failures, owner, field, value) {
  if (!Array.isArray(value)) {
    fail(failures, `${owner}.${field} must be an array`);
    return [];
  }
  return value;
}

function assertUnique(failures, label, values) {
  const seen = new Set();
  for (const value of values) {
    if (seen.has(value)) fail(failures, `${label} contains duplicate ${value}`);
    seen.add(value);
  }
}

function assertSortedUnique(failures, owner, field, values) {
  assertUnique(failures, `${owner}.${field}`, values);
  const expected = [...values].sort();
  if (JSON.stringify(values) !== JSON.stringify(expected)) {
    fail(failures, `${owner}.${field} must be sorted`);
  }
}

function requireEvidence(failures, owner, status, evidence, successfulStatuses) {
  if (successfulStatuses.has(status) && evidence.length === 0) {
    fail(failures, `${owner} is ${status} but has no evidence locator`);
  }
}

function parseDate(failures, owner, value) {
  if (!/^\d{4}-\d{2}-\d{2}$/.test(value ?? "")) {
    fail(failures, `${owner} must use YYYY-MM-DD`);
    return null;
  }
  const date = new Date(`${value}T00:00:00Z`);
  if (Number.isNaN(date.valueOf()) || date.toISOString().slice(0, 10) !== value) {
    fail(failures, `${owner} is not a valid calendar date`);
    return null;
  }
  return date;
}

function extractInvariantIds(source) {
  const ids = [];
  for (const line of source.split(/\r?\n/)) {
    const match = line.match(/^###\s+`(INV-[A-Z]+-\d{2})`[：:]/);
    if (match) ids.push(match[1]);
  }
  return ids;
}

async function main() {
  const failures = [];
  const registry = JSON.parse(await readFile(registryPath, "utf8"));

  if (registry.schema_version !== 2) fail(failures, "schema_version must be 2");
  if (registry.phase !== 0) fail(failures, "this registry must describe Phase 0");

  const updatedAt = parseDate(failures, "updated_at", registry.updated_at);
  const start = parseDate(failures, "phase_window.start", registry.phase_window?.start);
  const end = parseDate(failures, "phase_window.end", registry.phase_window?.end);
  if (start && end && start > end) fail(failures, "phase_window.start must not be after phase_window.end");
  if (updatedAt && start && updatedAt < start) fail(failures, "updated_at must not predate phase_window.start");

  const requirementStatuses = new Set(assertArray(failures, "registry", "requirement_statuses", registry.requirement_statuses));
  const gateStatuses = new Set(assertArray(failures, "registry", "gate_statuses", registry.gate_statuses));
  const requirements = assertArray(failures, "registry", "requirements", registry.requirements);
  const gates = assertArray(failures, "registry", "gates", registry.gates);
  const inputs = assertArray(failures, "registry", "inputs", registry.inputs);
  const probes = assertArray(failures, "registry", "probes", registry.probes);

  const requirementIds = requirements.map((item) => item.id);
  const gateIds = gates.map((item) => item.id);
  const inputIds = inputs.map((item) => item.id);
  const probeIds = probes.map((item) => item.id);
  assertUnique(failures, "requirements", requirementIds);
  assertUnique(failures, "gates", gateIds);
  assertUnique(failures, "inputs", inputIds);
  assertUnique(failures, "probes", probeIds);

  const allIds = [...requirementIds, ...gateIds, ...inputIds, ...probeIds.map((id) => `PROBE-${id}`)];
  assertUnique(failures, "registry IDs", allIds);

  const requirementIdSet = new Set(requirementIds);
  const gateIdSet = new Set(gateIds);
  const probeIdSet = new Set(probeIds);
  const invariantIds = extractInvariantIds(await readFile(invariantPath, "utf8"));
  assertUnique(failures, "security-recovery invariants", invariantIds);
  const invariantIdSet = new Set(invariantIds);

  for (const requirement of requirements) {
    const owner = requirement.id ?? "requirement<?>";
    if (!/^REQ-P0-\d{3}$/.test(requirement.id ?? "")) fail(failures, `${owner}.id has invalid format`);
    if (!/^P0\.\d+$|^Gate$/.test(requirement.work_package ?? "")) fail(failures, `${owner}.work_package has invalid format`);
    if (!nonEmptyString(requirement.statement)) fail(failures, `${owner}.statement is empty`);
    if (!requirementStatuses.has(requirement.status)) fail(failures, `${owner}.status is not allowed`);

    const requirementGates = assertArray(failures, owner, "gates", requirement.gates);
    const requirementProbes = assertArray(failures, owner, "probes", requirement.probes);
    const requirementInvariants = assertArray(failures, owner, "invariants", requirement.invariants);
    const requirementEvidence = assertArray(failures, owner, "evidence", requirement.evidence);
    assertSortedUnique(failures, owner, "gates", requirementGates);
    assertSortedUnique(failures, owner, "invariants", requirementInvariants);
    assertUnique(failures, `${owner}.probes`, requirementProbes);

    if (requirementGates.length === 0) fail(failures, `${owner} is not linked to a Gate`);
    for (const id of requirementGates) if (!gateIdSet.has(id)) fail(failures, `${owner} references unknown Gate ${id}`);
    for (const id of requirementInvariants) if (!invariantIdSet.has(id)) fail(failures, `${owner} references unknown invariant ${id}`);
    for (const id of requirementProbes) {
      if (!probeIdSet.has(id) && !SYNTHETIC_PROBES.has(id)) fail(failures, `${owner} references unknown probe ${id}`);
    }
    for (const locator of requirementEvidence) if (!nonEmptyString(locator)) fail(failures, `${owner}.evidence contains an empty locator`);
    requireEvidence(failures, owner, requirement.status, requirementEvidence, new Set(["SATISFIED"]));
  }

  for (const gate of gates) {
    const owner = gate.id ?? "gate<?>";
    if (!/^G0-\d{3}$/.test(gate.id ?? "")) fail(failures, `${owner}.id has invalid format`);
    if (!nonEmptyString(gate.statement)) fail(failures, `${owner}.statement is empty`);
    if (!gateStatuses.has(gate.status)) fail(failures, `${owner}.status is not allowed`);
    if (!/^ROLE_[A-Z0-9_]+$/.test(gate.owner ?? "")) fail(failures, `${owner}.owner has invalid format`);
    if (Object.hasOwn(gate, "requirements")) fail(failures, `${owner}.requirements duplicates requirement-side traceability and must be removed`);

    const gateProbes = assertArray(failures, owner, "probes", gate.probes);
    const gateEvidence = assertArray(failures, owner, "evidence", gate.evidence);
    assertUnique(failures, `${owner}.probes`, gateProbes);
    for (const id of gateProbes) {
      if (!probeIdSet.has(id) && !SYNTHETIC_PROBES.has(id)) fail(failures, `${owner} references unknown probe ${id}`);
    }
    for (const locator of gateEvidence) if (!nonEmptyString(locator)) fail(failures, `${owner}.evidence contains an empty locator`);
    requireEvidence(failures, owner, gate.status, gateEvidence, new Set(["PASS"]));

    const linkedRequirements = requirements.filter((requirement) => requirement.gates.includes(gate.id));
    if (linkedRequirements.length === 0) fail(failures, `${owner} has no linked requirement`);
  }

  for (const input of inputs) {
    const owner = input.id ?? "input<?>";
    if (!/^G0-IN-\d{3}$/.test(input.id ?? "")) fail(failures, `${owner}.id has invalid format`);
    if (!nonEmptyString(input.statement)) fail(failures, `${owner}.statement is empty`);
    if (!gateStatuses.has(input.status)) fail(failures, `${owner}.status is not allowed`);
    if (!/^ROLE_[A-Z0-9_]+$/.test(input.owner ?? "")) fail(failures, `${owner}.owner has invalid format`);
    if (!nonEmptyString(input.deadline)) fail(failures, `${owner}.deadline is empty`);

    const blocks = assertArray(failures, owner, "blocks", input.blocks);
    const inputEvidence = assertArray(failures, owner, "evidence", input.evidence);
    assertSortedUnique(failures, owner, "blocks", blocks);
    for (const id of blocks) if (!gateIdSet.has(id)) fail(failures, `${owner} blocks unknown Gate ${id}`);
    for (const locator of inputEvidence) if (!nonEmptyString(locator)) fail(failures, `${owner}.evidence contains an empty locator`);
    requireEvidence(failures, owner, input.status, inputEvidence, new Set(["PASS"]));
  }

  for (const probe of probes) {
    const owner = `PROBE-${probe.id ?? "<?>"}`;
    if (!/^[A-F]$/.test(probe.id ?? "")) fail(failures, `${owner}.id has invalid format`);
    if (!nonEmptyString(probe.title)) fail(failures, `${owner}.title is empty`);
    if (!PROBE_STATUSES.has(probe.status)) fail(failures, `${owner}.status is not allowed`);
    if (!/^docs\/probes\/[a-z0-9-]+\.md$/.test(probe.report ?? "")) fail(failures, `${owner}.report has invalid path`);
    if (Object.hasOwn(probe, "requirements")) fail(failures, `${owner}.requirements duplicates requirement-side traceability and must be removed`);

    const probeEvidence = assertArray(failures, owner, "evidence", probe.evidence);
    for (const locator of probeEvidence) if (!nonEmptyString(locator)) fail(failures, `${owner}.evidence contains an empty locator`);
    requireEvidence(failures, owner, probe.status, probeEvidence, new Set(["PASS"]));
    try {
      await access(path.resolve(repoRoot, probe.report));
    } catch {
      fail(failures, `${owner}.report does not exist: ${probe.report}`);
    }
    if (!requirements.some((requirement) => requirement.probes.includes(probe.id))) {
      fail(failures, `${owner} has no linked requirement`);
    }
  }

  const overall = registry.overall_gate ?? {};
  if (overall.id !== "G0") fail(failures, "overall_gate.id must be G0");
  if (!gateStatuses.has(overall.status)) fail(failures, "overall_gate.status is not allowed");
  if (!nonEmptyString(overall.rule)) fail(failures, "overall_gate.rule is empty");
  if (!/^docs\/gates\/g0-decision\.md$/.test(overall.decision_file ?? "")) {
    fail(failures, "overall_gate.decision_file must be docs/gates/g0-decision.md");
  }
  if (overall.status === "PASS") {
    if (gates.some((gate) => gate.status !== "PASS")) fail(failures, "overall G0 is PASS but not every Gate is PASS");
    try {
      await access(path.resolve(repoRoot, overall.decision_file));
    } catch {
      fail(failures, `overall G0 is PASS but decision file is missing: ${overall.decision_file}`);
    }
  }

  if (failures.length > 0) {
    for (const message of failures) console.error(`validate-registry: ${message}`);
    process.exitCode = 1;
    return;
  }

  const relationCount = requirements.reduce((total, requirement) => total + requirement.gates.length, 0);
  console.log(
    `validate-registry: ${requirements.length} requirement(s), ${gates.length} Gate(s), ${inputs.length} input(s), ${probes.length} probe(s), ${invariantIds.length} invariant(s), ${relationCount} trace link(s)`,
  );
}

main().catch((error) => {
  console.error(`validate-registry: ${error instanceof Error ? error.message : String(error)}`);
  process.exitCode = 1;
});
