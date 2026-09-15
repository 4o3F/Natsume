import type { DeviceState, DeviceStateFilterValue } from "./device-state";

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
    <fieldset className="flex flex-wrap items-center gap-x-4 gap-y-2 text-sm">
      <legend className="shrink-0 font-medium">Device state</legend>
      {states.map((state) => (
        <label key={state} className="flex items-center gap-2 capitalize">
          <input
            type="checkbox"
            checked={value[state]}
            onChange={(event) =>
              onChange({ ...value, [state]: event.target.checked })
            }
          />
          {state}
        </label>
      ))}
    </fieldset>
  );
}
