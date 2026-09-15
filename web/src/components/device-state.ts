import type { components } from "@/api/generated/schema";

export type DeviceState = components["schemas"]["DeviceResponse"]["state"];
export type DeviceStateFilterValue = Record<DeviceState, boolean>;

export const defaultDeviceStateFilter: DeviceStateFilterValue = {
  enabled: true,
  disabled: true,
  revoked: false,
};

export function matchesDeviceState(
  state: DeviceState,
  selection: DeviceStateFilterValue,
) {
  return selection[state];
}

export function selectedDeviceStates(
  selection: DeviceStateFilterValue,
): DeviceState[] {
  return (Object.keys(selection) as DeviceState[]).filter(
    (state) => selection[state],
  );
}
