import type { components } from "@/api/generated/schema";

export type DeviceStateFilterValue = components["schemas"]["DeviceListState"];

export function DeviceStateFilter({
  value,
  onChange,
}: {
  value: DeviceStateFilterValue;
  onChange: (value: DeviceStateFilterValue) => void;
}) {
  return (
    <div className="flex items-center gap-2 text-sm">
      <label htmlFor="device-state-filter" className="shrink-0 font-medium">
        Device state
      </label>
      <select
        id="device-state-filter"
        className="h-9 w-52 rounded-md border bg-background px-3 text-sm shadow-xs"
        value={value}
        onChange={(event) =>
          onChange(event.target.value as DeviceStateFilterValue)
        }
      >
        <option value="non_revoked">Enabled + disabled</option>
        <option value="enabled">Enabled only</option>
        <option value="disabled">Disabled only</option>
        <option value="revoked">Revoked only</option>
        <option value="all">All devices</option>
      </select>
    </div>
  );
}
