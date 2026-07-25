#!/usr/bin/env node
import { readFile, writeFile } from "node:fs/promises";
import path from "node:path";
import process from "node:process";
import { fileURLToPath } from "node:url";

const scriptDir = path.dirname(fileURLToPath(import.meta.url));
const docsDir = path.resolve(scriptDir, "..");
const repoRoot = path.resolve(docsDir, "..");
const registryPath = path.join(scriptDir, "registry.json");

function cell(value) {
  return String(value).replaceAll("|", "\\|").replace(/\r?\n/g, "<br>");
}

function list(values) {
  return values && values.length > 0 ? values.map((value) => `\`${cell(value)}\``).join("、") : "—";
}

function evidence(values) {
  return values && values.length > 0 ? values.map(cell).join("<br>") : "未产生";
}

function checkbox(status) {
  return status === "PASS" || status === "SATISFIED" ? "[x]" : "[ ]";
}

function generatedHeader(sourceRelative) {
  return [
    "<!-- GENERATED FILE: DO NOT EDIT DIRECTLY -->",
    `<!-- Source: ${sourceRelative}; regenerate with node docs/verification/render.mjs --write -->`,
    "",
  ].join("\n");
}

function renderRequirements(registry) {
  const lines = [
    generatedHeader("docs/verification/registry.json"),
    "# Phase 0 需求与追踪",
    "",
    `> Registry 更新：${registry.updated_at}  `,
    `> Phase 窗口：${registry.phase_window.start} 至 ${registry.phase_window.end}  `,
    "> 当前所有 requirement 状态以 registry 为准；文档生成不代表实现或验收完成。",
    "",
    "权威规则：",
    "",
    "- [Verification Registry](../verification/README.md)",
    "- [架构](../architecture.md)",
    "- [契约](../contracts.md)",
    "- [安全不变量](../security-recovery.md)",
    "- [G0 检查清单](../gates/g0-checklist.md)",
    "",
    "## 状态定义",
    "",
    list(registry.requirement_statuses),
    "",
  ];
  const groups = new Map();
  for (const req of registry.requirements) {
    if (!groups.has(req.work_package)) groups.set(req.work_package, []);
    groups.get(req.work_package).push(req);
  }
  for (const [group, reqs] of groups) {
    lines.push(`## ${group}`, "");
    lines.push("| ID | 需求 | Probe | Gate | 不变量 | 状态 | Evidence |");
    lines.push("|---|---|---|---|---|---|---|");
    for (const req of reqs) {
      lines.push(`| \`${req.id}\` | ${cell(req.statement)} | ${list(req.probes)} | ${list(req.gates)} | ${list(req.invariants)} | \`${req.status}\` | ${evidence(req.evidence)} |`);
    }
    lines.push("");
  }
  lines.push(
    "## 非目标",
    "",
    "- 完整领域 CRUD、Auth/RBAC、SSE；",
    "- 生产 CSV、Preparation Center 和业务 Web 页面；",
    "- 真实 fleet Command executor 和生产 Caddy generator；",
    "- 完整 Session/Home 状态机；",
    "- 将 Gateway certificate 加入 Enrollment；",
    "- 以文档或 scaffold 代替目标环境证据。",
    "",
    "## 变更控制",
    "",
    "1. 新 requirement 只追加 ID，不复用已发布 ID；",
    "2. 修改 registry 后运行 renderer；",
    "3. `SATISFIED` 必须有可定位 evidence；",
    "4. 总体 Gate 结论只存在于独立签署 decision；",
    "5. 证书、安全和特权边界不得通过 requirement waiver 放宽。",
    "",
  );
  return lines.join("\n");
}

function renderGate(registry) {
  const passCount = registry.gates.filter((gate) => gate.status === "PASS").length;
  const lines = [
    generatedHeader("docs/verification/registry.json"),
    "# G0 Gate 检查清单",
    "",
    `> 总体：\`${registry.overall_gate.status}\`，通过计数 \`${passCount} / ${registry.gates.length}\`  `,
    `> Phase 0 窗口：${registry.phase_window.start} 至 ${registry.phase_window.end}  `,
    "> 缺少证据等同未通过；禁止先通过后补证据。",
    "",
    "关联：",
    "",
    "- [Phase 0 requirements](../requirements/phase-0.md)",
    "- [支持平台](../supported-platform.md)",
    "- [实验室清单](../lab/phase-0-inventory.md)",
    "- [Probe reports](../probes/README.md)",
    "- [Verification Registry](../verification/README.md)",
    "",
    "## Gate 条目",
    "",
    "| 完成 | ID | 检查项 | Requirements | Probe | 判定 | Evidence | Owner |",
    "|---|---|---|---|---|---|---|---|",
  ];
  for (const gate of registry.gates) {
    const requirements = registry.requirements
      .filter((requirement) => requirement.gates.includes(gate.id))
      .map((requirement) => requirement.id);
    lines.push(`| ${checkbox(gate.status)} | \`${gate.id}\` | ${cell(gate.statement)} | ${list(requirements)} | ${list(gate.probes)} | \`${gate.status}\` | ${evidence(gate.evidence)} | \`${gate.owner}\` |`);
  }
  lines.push(
    "",
    "## 输入门禁",
    "",
    "`BLOCKED-INPUT` 不计作 Gate PASS。",
    "",
    "| 完成 | ID | 输入 | 截止 | 阻塞 | 判定 | Evidence | Owner |",
    "|---|---|---|---|---|---|---|---|",
  );
  for (const item of registry.inputs) {
    lines.push(`| ${checkbox(item.status)} | \`${item.id}\` | ${cell(item.statement)} | ${cell(item.deadline)} | ${list(item.blocks)} | \`${item.status}\` | ${evidence(item.evidence)} | \`${item.owner}\` |`);
  }
  lines.push(
    "",
    "## Probe 齐套性",
    "",
    "| Probe | 主题 | 报告 | 状态 | Evidence |",
    "|---|---|---|---|---|",
  );
  for (const probe of registry.probes) {
    const reportFromGate = path.relative(path.join(docsDir, "gates"), path.join(repoRoot, probe.report)).replaceAll("\\", "/");
    lines.push(`| \`PROBE-${probe.id}\` | ${cell(probe.title)} | [${path.basename(probe.report)}](${reportFromGate}) | \`${probe.status}\` | ${evidence(probe.evidence)} |`);
  }
  lines.push(
    "",
    "## 不可豁免复核",
    "",
    "- Enrollment request、DB 和 response 无 Gateway material；",
    "- Gateway CSR 只在 mandatory-mTLS QUIC 的 active `SYNC_STATE`；",
    "- anonymous QUIC 未进入 Protobuf decoder，0-RTT 关闭；",
    "- CSR SAN 不授予权限；",
    "- 无 TOFU、Identity Guard、systemd credentials 或 runtime download；",
    "- Session lock/unlock 不调用或改变 Caddy；",
    "- secret 不进入 API、日志、Observed、UI storage 或状态页。",
    "",
    "失败时必须把关联 Gate 标记为 `FAIL`，不能用 waiver 关闭。",
    "",
    "## Gate decision",
    "",
    `总体规则：${registry.overall_gate.rule}`,
    "",
    `预期 decision 路径：\`${registry.overall_gate.decision_file}\`。该文件在正式签署前不应创建为 PASS decision；可使用 [decision template](g0-decision-template.md)。`,
    "",
  );
  return lines.join("\n");
}

function renderStatus(registry) {
  const reqCounts = Object.fromEntries(registry.requirement_statuses.map((status) => [status, 0]));
  for (const req of registry.requirements) reqCounts[req.status] = (reqCounts[req.status] ?? 0) + 1;
  const gatePass = registry.gates.filter((gate) => gate.status === "PASS").length;
  const blockedInputs = registry.inputs.filter((item) => item.status === "BLOCKED-INPUT").length;
  const lines = [
    generatedHeader("docs/verification/registry.json"),
    "# Phase 0 当前状态",
    "",
    `> 数据日期：${registry.updated_at}  `,
    `> Phase 窗口：${registry.phase_window.start} 至 ${registry.phase_window.end}  `,
    `> G0：\`${registry.overall_gate.status}\`，\`${gatePass} / ${registry.gates.length}\` PASS`,
    "",
    "## 结论",
    "",
    "当前仓库仍是 Phase 0 工程基线。文档重构只消除重复和矛盾，不把任何实现、平台或 Gate 条目标记为完成。",
    "",
    "| 指标 | 当前值 |",
    "|---|---:|",
    `| Requirements | ${registry.requirements.length} |`,
    `| OPEN requirements | ${reqCounts["OPEN"] ?? 0} |`,
    `| SATISFIED requirements | ${reqCounts["SATISFIED"] ?? 0} |`,
    `| G0 PASS | ${gatePass} / ${registry.gates.length} |`,
    `| BLOCKED-INPUT inputs | ${blockedInputs} / ${registry.inputs.length} |`,
    `| Probe PASS | ${registry.probes.filter((probe) => probe.status === "PASS").length} / ${registry.probes.length} |`,
    "",
    "## 主要阻塞",
    "",
  ];
  for (const item of registry.inputs) {
    if (item.status !== "PASS") lines.push(`- \`${item.id}\`：${cell(item.statement)}（\`${item.status}\`，截止 ${cell(item.deadline)}）`);
  }
  lines.push(
    "",
    "## 下一步",
    "",
    "1. 冻结目标 OS、Server endpoint、Caddy supply 和 PKI test material；",
    "2. 到位并登记六台物理工作站；",
    "3. 执行 Probe A–F 并提交可复现 evidence；",
    "4. 根据 evidence 更新 registry；",
    "5. 运行 renderer 和链接/契约检查；",
    "6. 15 项 Gate 全部 PASS 后签署独立 G0 decision。",
    "",
    "## 详情",
    "",
    "- [Requirements](../requirements/phase-0.md)",
    "- [G0 checklist](../gates/g0-checklist.md)",
    "- [Supported platform](../supported-platform.md)",
    "- [Lab inventory](../lab/phase-0-inventory.md)",
    "- [Probe reports](../probes/README.md)",
    "",
  );
  return lines.join("\n");
}

async function main() {
  const mode = process.argv[2];
  if (mode !== "--write" && mode !== "--check") {
    throw new Error("usage: node docs/verification/render.mjs --write|--check");
  }
  const registry = JSON.parse(await readFile(registryPath, "utf8"));
  if (registry.schema_version !== 2) {
    throw new Error("registry schema_version must be 2; run validate-registry.mjs for details");
  }
  const outputs = new Map([
    [path.join(docsDir, "requirements", "phase-0.md"), renderRequirements(registry)],
    [path.join(docsDir, "gates", "g0-checklist.md"), renderGate(registry)],
    [path.join(scriptDir, "phase-0-status.md"), renderStatus(registry)],
  ]);
  const failures = [];
  for (const [file, value] of outputs) {
    const expected = `${value.trimEnd()}\n`;
    if (mode === "--write") {
      await writeFile(file, expected, "utf8");
      console.log(`render-verification: wrote ${path.relative(repoRoot, file)}`);
    } else {
      let actual;
      try {
        actual = await readFile(file, "utf8");
      } catch {
        failures.push(`${path.relative(repoRoot, file)} is missing`);
        continue;
      }
      if (actual !== expected) failures.push(`${path.relative(repoRoot, file)} is stale`);
    }
  }
  if (failures.length > 0) {
    for (const failure of failures) console.error(`render-verification: ${failure}`);
    process.exitCode = 1;
  } else if (mode === "--check") {
    console.log(`render-verification: ${outputs.size} generated file(s) are current`);
  }
}

main().catch((error) => {
  console.error(`render-verification: ${error instanceof Error ? error.message : String(error)}`);
  process.exitCode = 1;
});
