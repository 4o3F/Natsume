import { expect, it } from "vitest";

import type { DeviceSummary } from "../api/generated/types";
import { FLAPPING_DISCONNECTS, isFlapping, lastSeen } from "./device-summaries";

const summary: DeviceSummary = {
  device_id: "01912345-6789-7abc-8def-0123456789ab",
  last_connected_at: 1_000,
  last_disconnected_at: 3_000,
  last_disconnect_reason: "ping_silence",
  disconnects: FLAPPING_DISCONNECTS - 1,
};

it("flags flapping at the documented disconnect threshold", () => {
  expect(isFlapping(undefined)).toBe(false);
  expect(isFlapping(summary)).toBe(false);
  expect(isFlapping({ ...summary, disconnects: FLAPPING_DISCONNECTS })).toBe(
    true,
  );
});

it("reports connected devices as seen now and otherwise the latest fact", () => {
  expect(lastSeen(undefined, "active")).toBe("now");
  expect(lastSeen(summary, "awaiting_fresh_state")).toBe("now");
  expect(lastSeen(summary, "offline")).toBe(3_000);
  expect(lastSeen({ ...summary, last_disconnected_at: null }, "offline")).toBe(
    1_000,
  );
  expect(
    lastSeen(
      { ...summary, last_connected_at: null, last_disconnected_at: null },
      "offline",
    ),
  ).toBeNull();
  expect(lastSeen(undefined, "offline")).toBeNull();
});
