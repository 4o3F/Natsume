import { expect, it } from "vitest";

import type { Event } from "../api/generated/types";
import {
  categoryLabels,
  describeEvent,
  disconnectReasonLabels,
  eventKindLabels,
  eventKinds,
  isEventKind,
  labelFor,
} from "./event-labels";

it("labels every event kind with its server category", () => {
  expect(Object.keys(eventKinds)).toHaveLength(31);
  for (const [kind, { label, category }] of Object.entries(eventKinds)) {
    expect(label, kind).not.toBe("");
    expect(Object.keys(categoryLabels)).toContain(category);
    expect(eventKindLabels[kind as keyof typeof eventKindLabels]).toBe(label);
  }
  expect(eventKinds.operator_request.category).toBe("audit");
  expect(eventKinds.auth_failed.category).toBe("auth");
  expect(eventKinds.journal_dropped.category).toBe("journal");
});

it("falls back for unknown or prototype values without echoing them", () => {
  expect(labelFor(disconnectReasonLabels, "ping_silence")).toBe(
    "Heartbeat lost",
  );
  expect(labelFor(disconnectReasonLabels, "<script>")).toBe("Unrecognized");
  expect(labelFor(disconnectReasonLabels, "constructor")).toBe("Unrecognized");
  expect(labelFor(eventKindLabels, "unknown")).toBe("Unrecognized");
  expect(isEventKind("disconnected")).toBe(true);
  expect(isEventKind("toString")).toBe(false);
});

it("describes events from labels and shortened typed identifiers", () => {
  const session = "01912345-6789-7abc-8def-0123456789ab";
  expect(
    describeEvent({
      kind: "disconnected",
      session_id: session,
      reason: "ping_silence",
    }),
  ).toBe("Heartbeat lost · Session 01912345");
  expect(
    describeEvent({
      kind: "actual_changed",
      resource: "session",
      from: null,
      to: "running",
    }),
  ).toBe("Session: first report → Running");
  expect(describeEvent({ kind: "auth_succeeded" })).toBe("");
});

it("omits malformed API codes and bounds audit targets", () => {
  const event: Event = {
    kind: "operator_request",
    route: "device",
    method: "patch",
    status: 403,
    api_code: "not a token",
    cause: null,
    targets: [
      { device: "01912345-6789-7abc-8def-0123456789ab" },
      { seat: "A-01" },
      { print_execution: 7 },
      { operator: "01912345-6789-7abc-8def-0123456789ac" },
    ],
  };
  expect(describeEvent(event)).toBe(
    "PATCH Device · HTTP 403 · Device 01912345 · Seat A-01 · Print execution #7 · +1 more",
  );
  expect(describeEvent({ ...event, api_code: "FORBIDDEN" })).toContain(
    "HTTP 403 · FORBIDDEN ·",
  );
  expect(describeEvent({ ...event, targets: [{ review: "abcdef0123" }] })).toBe(
    "PATCH Device · HTTP 403 · Review abcdef01",
  );
});

it("labels Photo administrator audit routes and source targets", () => {
  for (const [route, label] of [
    ["photo_sources", "Photo sources"],
    ["photo_pairing_code", "Photo pairing code"],
    ["photo_enrollment_reset", "Photo enrollment reset"],
    ["photo_source_state", "Photo source state"],
  ] as const) {
    expect(
      describeEvent({
        kind: "operator_request",
        route,
        method: "post",
        status: 200,
        api_code: null,
        cause: null,
        targets: [{ photo_source: "01912345-6789-7abc-8def-0123456789ab" }],
      }),
    ).toBe(`POST ${label} · HTTP 200 · Photo source 01912345`);
  }
});
