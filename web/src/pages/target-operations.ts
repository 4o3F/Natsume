import { unwrap } from "@/api/errors";
import type { components } from "@/api/generated/schema";
import type { SessionScope } from "@/auth/session";

export type TargetOperation =
  | components["schemas"]["SessionForegroundRequest"]["foreground_target"]
  | "terminate"
  | "reset";

export type TargetSubmission = {
  deviceId: string;
  error: string | null;
};

export const targetOperations = {
  waiting: "Show waiting screen",
  contest: "Show contest desktop",
  terminate: "Terminate",
  reset: "Reset home",
} as const;

export async function submitTarget(
  api: SessionScope["api"],
  deviceId: string,
  operation: TargetOperation,
) {
  const params = { path: { device_id: deviceId } };
  switch (operation) {
    case "waiting":
    case "contest":
      return unwrap<components["schemas"]["SessionControlResponse"]>(
        await api.PUT("/api/v2/devices/{device_id}/session-control", {
          params,
          body: { foreground_target: operation },
        }),
      );
    case "terminate":
      return unwrap<components["schemas"]["SessionControlResponse"]>(
        await api.POST(
          "/api/v2/devices/{device_id}/session-control/actions/terminate",
          { params },
        ),
      );
    case "reset":
      return unwrap<components["schemas"]["HomeResponse"]>(
        await api.POST("/api/v2/devices/{device_id}/home/actions/reset", {
          params,
        }),
      );
  }
}
