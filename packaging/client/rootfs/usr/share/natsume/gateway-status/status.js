"use strict";

const endpoint = "/.well-known/natsume/gateway-status.json";
const intervalMs = 2000;
const language = navigator.language.toLowerCase().startsWith("zh") ? "zh" : "en";

const copy = {
  zh: {
    brand: "本地比赛网关",
    machine: "设备",
    seat: "赛位",
    updated: "更新时间",
    operation: "操作",
    progress: "处理进度",
    footer: "此页面只显示本机非敏感状态，不会展示比赛账号或密码。",
    unavailable: "本地状态暂时不可用；安全网关仍保持阻断。",
    states: {
      restoring: ["RESTORING", "正在恢复比赛网关", "正在校验证书、配置和本地安全状态。"],
      transition_blocked: ["TRANSITION", "正在切换赛位配置", "旧会话已阻断，目标配置尚未完成验证。"],
      secret_missing: ["SECRET", "等待账号密码同步", "赛位与配置已应用，但管理员尚未执行机密同步。"],
      upstream_unhealthy: ["UPSTREAM", "比赛服务暂不可用", "本机网关正常，但中心服务健康检查未通过。"],
      recovery_required: ["RECOVERY", "工作站需要恢复", "当前状态无法安全确认，已停止访问比赛服务。"],
      unassigned: ["UNASSIGNED", "工作站尚未分配", "等待管理员完成赛位绑定和配置同步。"],
    },
    actions: {
      wait: "请保持此页面打开，工作站会自动重试。",
      contact_operator: "请联系现场管理员，并提供设备短码。",
      check_network: "请检查比赛网络连接；不要绕过本地网关。",
      request_secret_sync: "请联系管理员执行机密同步；页面不会显示密码。",
    },
  },
  en: {
    brand: "Local contest gateway",
    machine: "Device",
    seat: "Seat",
    updated: "Updated",
    operation: "Operation",
    progress: "Progress",
    footer: "This page shows non-sensitive local state only. It never displays contest credentials.",
    unavailable: "Local status is temporarily unavailable; the secure gateway remains blocked.",
    states: {
      restoring: [
        "RESTORING",
        "Restoring the contest gateway",
        "Certificates, configuration and local safety state are being verified.",
      ],
      transition_blocked: [
        "TRANSITION",
        "Switching workstation assignment",
        "The previous session is blocked while the target configuration is verified.",
      ],
      secret_missing: [
        "SECRET",
        "Waiting for credential synchronization",
        "The assignment is applied, but an operator has not synchronized the credential.",
      ],
      upstream_unhealthy: [
        "UPSTREAM",
        "Contest service unavailable",
        "The local gateway is healthy, but the central service failed its health check.",
      ],
      recovery_required: [
        "RECOVERY",
        "Workstation recovery required",
        "The current state cannot be proven safe, so contest access remains blocked.",
      ],
      unassigned: [
        "UNASSIGNED",
        "Workstation not assigned",
        "Waiting for an operator to bind a seat and synchronize configuration.",
      ],
    },
    actions: {
      wait: "Keep this page open. The workstation will retry automatically.",
      contact_operator: "Contact an on-site operator and provide the device short code.",
      check_network: "Check the contest network connection. Do not bypass the local gateway.",
      request_secret_sync: "Ask an operator to synchronize the credential. This page never displays it.",
    },
  },
};

const ui = copy[language];
const byId = (id) => document.getElementById(id);
const setText = (id, value) => { byId(id).textContent = value ?? "—"; };

function applyStaticCopy() {
  setText("brand-subtitle", ui.brand);
  setText("machine-label", ui.machine);
  setText("seat-label-title", ui.seat);
  setText("updated-label", ui.updated);
  setText("operation-label", ui.operation);
  setText("progress-label", ui.progress);
  setText("footer-note", ui.footer);
}

function render(snapshot) {
  const state = Object.prototype.hasOwnProperty.call(ui.states, snapshot.state)
    ? snapshot.state
    : "recovery_required";
  const [badge, title, detail] = ui.states[state];
  document.body.dataset.state = state;
  document.querySelector("main").setAttribute("aria-busy", state === "restoring" ? "true" : "false");
  setText("state-badge", badge);
  setText("state-title", title);
  setText("state-detail", detail);
  setText("machine-id", snapshot.machine_short_id);
  setText("seat-label", snapshot.seat_label);
  setText("operation-id", snapshot.operation_short_id);

  const updatedAt = Number.isFinite(snapshot.updated_at_unix_ms)
    ? new Date(snapshot.updated_at_unix_ms)
    : null;
  setText("updated-at", updatedAt && !Number.isNaN(updatedAt.valueOf()) ? updatedAt.toLocaleString() : "—");

  const current = Number(snapshot.progress_current);
  const total = Number(snapshot.progress_total);
  const hasProgress = Number.isFinite(current) && Number.isFinite(total) && total > 0 && current >= 0;
  byId("progress-panel").hidden = !hasProgress;
  if (hasProgress) {
    byId("progress").max = total;
    byId("progress").value = Math.min(current, total);
    setText("progress-value", `${current} / ${total}`);
  }

  const action = Object.prototype.hasOwnProperty.call(ui.actions, snapshot.suggested_action)
    ? snapshot.suggested_action
    : "contact_operator";
  setText("suggested-action", ui.actions[action]);
}

function renderUnavailable() {
  document.body.dataset.state = "recovery_required";
  setText("state-badge", "BLOCKED");
  setText("state-title", language === "zh" ? "安全网关保持阻断" : "Secure gateway remains blocked");
  setText("state-detail", ui.unavailable);
  setText("suggested-action", ui.actions.contact_operator);
  byId("progress-panel").hidden = true;
}

async function refresh() {
  try {
    const response = await fetch(endpoint, { cache: "no-store", credentials: "omit" });
    if (!response.ok) throw new Error(`status ${response.status}`);
    const snapshot = await response.json();
    render(snapshot);
  } catch (_error) {
    renderUnavailable();
  }
}

applyStaticCopy();
void refresh();
window.setInterval(() => { void refresh(); }, intervalMs);
