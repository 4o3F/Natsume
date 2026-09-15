import type { components } from "@/api/generated/schema";

export type TargetRequest = components["schemas"]["TargetSubmissionBody"];
export type TargetResponse = components["schemas"]["TargetSubmissionResponse"];
export type TargetOperation =
  "waiting" | "contest" | "terminate" | "reset" | "poweroff";

export const targetOperations = {
  waiting: "Show waiting screen",
  contest: "Show contest desktop",
  terminate: "Terminate",
  reset: "Reset home",
  poweroff: "Power off online devices",
} as const;

export function targetAction(
  operation: TargetOperation,
): TargetRequest["action"] {
  switch (operation) {
    case "waiting":
    case "contest":
      return { kind: "set_foreground", foreground_target: operation };
    case "terminate":
      return { kind: "terminate_session" };
    case "reset":
      return { kind: "reset_home" };
    case "poweroff":
      return { kind: "power_off" };
  }
}

export function targetActionLabel(action: TargetRequest["action"]) {
  switch (action.kind) {
    case "set_foreground":
      return targetOperations[action.foreground_target];
    case "terminate_session":
      return targetOperations.terminate;
    case "reset_home":
      return targetOperations.reset;
    case "power_off":
      return targetOperations.poweroff;
  }
}
