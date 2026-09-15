import type { DeviceState, DeviceStateFilterValue } from "./device-state";
import { CheckboxFilter, CheckboxFilterOption } from "./checkbox-filter";

export type { DeviceStateFilterValue } from "./device-state";

const states: readonly DeviceState[] = ["enabled", "disabled", "revoked"];

export function DeviceStateFilter({
  value,
  onChange,
}: {
  value: DeviceStateFilterValue;
  onChange: (value: DeviceStateFilterValue) => void;
}) {
  return (
    <CheckboxFilter label="Device state">
      {states.map((state) => (
        <CheckboxFilterOption
          key={state}
          label={state[0].toUpperCase() + state.slice(1)}
          checked={value[state]}
          onCheckedChange={(checked) =>
            onChange({ ...value, [state]: checked })
          }
        />
      ))}
    </CheckboxFilter>
  );
}
