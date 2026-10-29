/**
 * English labels for every closed journal vocabulary. Each map `satisfies` a
 * record over the generated union, so adding a server variant fails the build
 * until it is labelled here. Event text is composed only from these labels and
 * typed identifiers; persisted free text is never rendered.
 */
import type {
  ActualResource,
  ActualState,
  AdmissionStage,
  AuditRoute,
  AuditTarget,
  AuthFailureClass,
  Availability,
  BindingCode,
  Category,
  DisconnectReason,
  Event,
  GapKind,
  HttpMethod,
  LifecycleState,
  PauseReason,
  ReviewCancelCause,
  SessionEnd,
  Severity,
  TargetAction,
  TargetResult,
} from "../api/generated/types";

export type EventKind = Event["kind"];
type AuditTargetKey = keyof AuditTarget;

/** Label shown for any value the generated vocabulary does not contain. */
const UNKNOWN_LABEL = "Unrecognized";

/** Looks up a closed-vocabulary label, falling back for unknown wire values. */
export function labelFor<T extends string>(
  labels: Readonly<Record<T, string>>,
  value: string,
): string {
  return Object.hasOwn(labels, value) ? labels[value as T] : UNKNOWN_LABEL;
}

export const categoryLabels = {
  device: "Device",
  audit: "Operator requests",
  auth: "Authentication",
  journal: "Journal",
} as const satisfies Record<Category, string>;

export const severityLabels = {
  info: "Info",
  warning: "Warning",
  error: "Error",
} as const satisfies Record<Severity, string>;

export const eventKinds = {
  admission_rejected: { label: "Admission rejected", category: "device" },
  lease_attached: { label: "Connected", category: "device" },
  lease_replaced: { label: "Connection replaced", category: "device" },
  disconnected: { label: "Disconnected", category: "device" },
  reconcile_retry: { label: "Reconcile retry", category: "device" },
  actor_stopped: { label: "Device actor stopped", category: "device" },
  actual_changed: { label: "Reported state changed", category: "device" },
  review_created: { label: "Enrollment review created", category: "device" },
  review_cancelled: {
    label: "Enrollment review cancelled",
    category: "device",
  },
  enrollment_approved: { label: "Enrollment approved", category: "device" },
  enrollment_denied: { label: "Enrollment denied", category: "device" },
  activation_committed: { label: "Activation committed", category: "device" },
  admission_completed: { label: "Admission completed", category: "device" },
  lifecycle_changed: { label: "Lifecycle changed", category: "device" },
  binding_bound: { label: "Seat bound", category: "device" },
  binding_rejected: { label: "Seat binding rejected", category: "device" },
  binding_unbound: { label: "Seat unbound", category: "device" },
  target_submitted: { label: "Target submitted", category: "device" },
  operator_request: { label: "Operator request", category: "audit" },
  auth_succeeded: { label: "Signed in", category: "auth" },
  auth_failed: { label: "Sign-in failed", category: "auth" },
  logged_out: { label: "Signed out", category: "auth" },
  password_changed: { label: "Password changed", category: "auth" },
  password_reset_issued: { label: "Password reset issued", category: "auth" },
  password_reset_completed: {
    label: "Password reset completed",
    category: "auth",
  },
  invitation_registered: { label: "Invitation registered", category: "auth" },
  server_started: { label: "Server started", category: "journal" },
  server_stopping: { label: "Server stopping", category: "journal" },
  journal_dropped: { label: "Events dropped", category: "journal" },
  journal_paused: { label: "Journal paused", category: "journal" },
  journal_resumed: { label: "Journal resumed", category: "journal" },
} as const satisfies Record<EventKind, { label: string; category: Category }>;

export const eventKindLabels = Object.fromEntries(
  Object.entries(eventKinds).map(([kind, { label }]) => [kind, label]),
) as Record<EventKind, string>;

export function isEventKind(value: string): value is EventKind {
  return Object.hasOwn(eventKinds, value);
}

export const disconnectReasonLabels = {
  disabled: "Device disabled",
  revoked: "Device revoked",
  control_key_replaced: "Control key replaced",
  ping_silence: "Heartbeat lost",
  client_close: "Closed by client",
  transport_error: "Transport error",
  protocol_violation: "Protocol violation",
  actor_inbox_unavailable: "Device actor unavailable",
  send_failed: "Send failed",
  send_timeout: "Send timed out",
  outgoing_closed: "Outgoing channel closed",
  client_fault: "Client fault",
  server_fault: "Server fault",
  transient_limit: "Transient retry limit",
  outbound_backpressure: "Outbound backpressure",
  outbound_closed: "Outbound channel closed",
  server_shutdown: "Server shutdown",
  actor_failed: "Device actor failed",
} as const satisfies Record<DisconnectReason, string>;

const admissionStageLabels = {
  server_stopping: "Server stopping",
  capacity: "Capacity",
  protocol_version: "Protocol version",
  challenge: "Challenge",
  proof: "Proof",
  metadata: "Metadata",
  resume_authority: "Resume authority",
  enrollment: "Enrollment",
  activation: "Activation",
  ready_exchange: "Ready exchange",
  attach: "Attach",
} as const satisfies Record<AdmissionStage, string>;

export const gapKindLabels = {
  pruned: "History pruned",
  dropped: "Events dropped",
  paused: "Journal paused",
  unreadable: "Unreadable events",
} as const satisfies Record<GapKind, string>;

export const sessionEndLabels = {
  disconnected: "Disconnected",
  unknown: "End unknown",
  ongoing: "Ongoing",
} as const satisfies Record<SessionEnd["state"], string>;

const actualResourceLabels = {
  gateway: "Gateway",
  binding_access: "Binding access",
  runtime_config: "Runtime config",
  foreground: "Foreground",
  session: "Session",
  home: "Home",
  power: "Power",
} as const satisfies Record<ActualResource, string>;

const actualStateLabels = {
  unknown: "Unknown",
  none: "None",
  starting: "Starting",
  running: "Running",
  terminating: "Terminating",
  ambiguous: "Ambiguous",
  error: "Error",
  waiting: "Waiting",
  contest: "Contest",
  greeter: "Greeter",
  other: "Other",
  steady: "Steady",
  resetting: "Resetting",
  recovery_required: "Recovery required",
  idle: "Idle",
  pending: "Pending",
  accepted: "Accepted",
  expired: "Expired",
  absent: "Absent",
  applied: "Applied",
  failed: "Failed",
  blocked: "Blocked",
  restoring: "Restoring",
  ready: "Ready",
  upstream_unhealthy: "Upstream unhealthy",
} as const satisfies Record<ActualState, string>;

const targetActionLabels = {
  set_foreground: "Set foreground",
  terminate_session: "Terminate session",
  reset_home: "Reset home",
  power_off: "Power off",
} as const satisfies Record<TargetAction, string>;

const targetResultLabels = {
  accepted: "Accepted",
  device_not_found: "Device not found",
  device_not_enabled: "Device not enabled",
  invalid_device_state: "Invalid device state",
  invalid_target: "Invalid target",
  epoch_exhausted: "Epoch exhausted",
} as const satisfies Record<TargetResult, string>;

const auditRouteLabels = {
  session: "Session",
  device: "Device",
  device_binding: "Device binding",
  review_approve: "Approve enrollment",
  review_deny: "Deny enrollment",
  imports: "Imports",
  import: "Import",
  import_commit: "Commit import",
  operator_account: "Operator account",
  password_reset_issue: "Issue password reset",
  invitations: "Invitations",
  invitation: "Invitation",
  invitation_regenerate: "Regenerate invitation",
  password_change: "Change password",
  password_reset: "Reset password",
  password_reset_inspect: "Inspect password reset",
  registration: "Registration",
  registration_inspect: "Inspect registration",
  print_requeue: "Requeue print",
  printers: "Printers",
  printer_binding: "Printer binding",
  printer_pairing_code: "Printer pairing code",
  print_sources: "Print sources",
  print_source: "Print source",
  photo_sources: "Photo sources",
  photo_pairing_code: "Photo pairing code",
  photo_source_state: "Photo source state",
  provisioning: "Provisioning window",
  target_submission: "Target submission",
} as const satisfies Record<AuditRoute, string>;

const httpMethodLabels = {
  post: "POST",
  put: "PUT",
  patch: "PATCH",
  delete: "DELETE",
  options: "OPTIONS",
  connect: "CONNECT",
  trace: "TRACE",
  other: "Other method",
} as const satisfies Record<HttpMethod, string>;

const auditTargetLabels = {
  account: "Account",
  binding: "Binding",
  device: "Device",
  import: "Import",
  invitation: "Invitation",
  operator: "Operator",
  password_reset: "Password reset",
  print_execution: "Print execution",
  print_source: "Print source",
  photo_source: "Photo source",
  printer: "Printer",
  review: "Review",
  seat: "Seat",
} as const satisfies Record<AuditTargetKey, string>;

const reviewCancelCauseLabels = {
  client_close: "Closed by client",
  timeout: "Timed out",
  replaced: "Replaced",
  server_stopping: "Server stopping",
  transport_error: "Transport error",
} as const satisfies Record<ReviewCancelCause, string>;

const bindingCodeLabels = {
  not_found: "Seat not found",
  unmapped: "Seat not mapped",
  occupied: "Seat occupied",
} as const satisfies Record<BindingCode, string>;

const lifecycleStateLabels = {
  enabled: "Enabled",
  disabled: "Disabled",
  revoked: "Revoked",
} as const satisfies Record<LifecycleState, string>;

const authFailureClassLabels = {
  invalid_credentials: "Invalid credentials",
  rate_limited: "Rate limited",
  infrastructure: "Server error",
} as const satisfies Record<AuthFailureClass, string>;

const pauseReasonLabels = {
  disk_budget: "Disk budget reached",
} as const satisfies Record<PauseReason, string>;

export const availabilityLabels = {
  ready: "Recording",
  paused: "Paused",
  degraded: "Degraded",
} as const satisfies Record<Availability, string>;

/** Leading characters of a UUID, enough to tell identifiers apart on screen. */
export function shortId(id: string) {
  return id.slice(0, 8);
}

/** Formats one typed audit target as `<label> <short id>`. */
function formatAuditTarget(target: AuditTarget): string {
  const key = (Object.keys(auditTargetLabels) as AuditTargetKey[]).find(
    (name) => target[name] !== undefined,
  );
  if (!key) return UNKNOWN_LABEL;
  const value = target[key];
  return `${auditTargetLabels[key]} ${typeof value === "number" ? `#${value}` : key === "seat" ? value : shortId(String(value))}`;
}

const ERROR_TOKEN = /^[A-Z][A-Z0-9_]{0,63}$/;

/**
 * One-line event summary composed from closed labels and typed identifiers.
 * Returns an empty string when the kind alone is the whole fact.
 */
export function describeEvent(event: Event): string {
  switch (event.kind) {
    case "admission_rejected":
      return `Stage: ${labelFor(admissionStageLabels, event.stage)} · Peer ${event.peer_ip}`;
    case "lease_attached":
      return `Session ${shortId(event.session_id)}`;
    case "lease_replaced":
      return `Session ${shortId(event.old)} → ${shortId(event.new)}`;
    case "disconnected":
      return `${labelFor(disconnectReasonLabels, event.reason)} · Session ${shortId(event.session_id)}`;
    case "reconcile_retry":
      return `Attempt ${event.attempt}`;
    case "actor_stopped":
      return [
        event.panicked ? "Panicked" : "Exited",
        event.session_id ? `Session ${shortId(event.session_id)}` : null,
      ]
        .filter(Boolean)
        .join(" · ");
    case "actual_changed":
      return `${labelFor(actualResourceLabels, event.resource)}: ${
        event.from === null
          ? "first report"
          : labelFor(actualStateLabels, event.from)
      } → ${labelFor(actualStateLabels, event.to)}`;
    case "review_created":
      return `Review ${shortId(event.review_id)} · Peer ${event.peer_ip}`;
    case "review_cancelled":
      return `Review ${shortId(event.review_id)} · ${labelFor(reviewCancelCauseLabels, event.cause)}`;
    case "enrollment_approved":
      return `Review ${shortId(event.review_id)} · ${event.automatic ? "Automatic" : "By administrator"}`;
    case "enrollment_denied":
      return `Review ${shortId(event.review_id)}`;
    case "lifecycle_changed":
      return `Now ${labelFor(lifecycleStateLabels, event.to).toLowerCase()}`;
    case "binding_bound":
      return `Seat ${event.seat_code}`;
    case "binding_rejected":
      return labelFor(bindingCodeLabels, event.code);
    case "binding_unbound":
      return `Seat ${event.seat_code}`;
    case "target_submitted":
      return [
        labelFor(targetActionLabels, event.action),
        labelFor(targetResultLabels, event.result),
        event.replayed ? "Replayed" : null,
      ]
        .filter(Boolean)
        .join(" · ");
    case "operator_request":
      return [
        `${labelFor(httpMethodLabels, event.method)} ${labelFor(auditRouteLabels, event.route)}`,
        `HTTP ${event.status}`,
        event.api_code && ERROR_TOKEN.test(event.api_code)
          ? event.api_code
          : null,
        ...event.targets.slice(0, 3).map(formatAuditTarget),
        event.targets.length > 3 ? `+${event.targets.length - 3} more` : null,
      ]
        .filter(Boolean)
        .join(" · ");
    case "auth_failed":
      return labelFor(authFailureClassLabels, event.class);
    case "server_started":
      return `Boot ${shortId(event.boot_id)}`;
    case "server_stopping":
      return event.lost_on_flush
        ? `${event.lost_on_flush} events lost on flush`
        : "";
    case "journal_dropped":
      return `${event.count} ${labelFor(categoryLabels, event.category).toLowerCase()} events`;
    case "journal_paused":
      return labelFor(pauseReasonLabels, event.reason);
    case "activation_committed":
    case "admission_completed":
    case "auth_succeeded":
    case "logged_out":
    case "password_changed":
    case "password_reset_issued":
    case "password_reset_completed":
    case "invitation_registered":
    case "journal_resumed":
      return "";
    default:
      return exhaustive(event);
  }
}

function exhaustive(value: never): string {
  void value;
  return "";
}
