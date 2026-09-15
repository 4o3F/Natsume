import type { ReactNode } from "react";
import { Check } from "lucide-react";

export function CheckboxFilter({
  label,
  children,
}: {
  label: string;
  children: ReactNode;
}) {
  return (
    <fieldset className="flex min-w-0 flex-wrap items-center gap-2">
      <legend className="sr-only">{label}</legend>
      <span aria-hidden="true" className="mr-1 text-sm font-medium">
        {label}
      </span>
      {children}
    </fieldset>
  );
}

export function CheckboxFilterOption({
  label,
  checked,
  onCheckedChange,
}: {
  label: string;
  checked: boolean;
  onCheckedChange: (checked: boolean) => void;
}) {
  return (
    <label className="flex h-9 cursor-pointer items-center gap-2 rounded-md border bg-background px-3 text-sm text-muted-foreground shadow-xs transition-colors select-none hover:bg-accent has-checked:bg-secondary has-checked:text-secondary-foreground has-focus-visible:border-ring has-focus-visible:ring-[3px] has-focus-visible:ring-ring/50">
      <span className="relative flex size-4 shrink-0">
        <input
          type="checkbox"
          className="peer size-4 cursor-pointer appearance-none rounded border border-input bg-background outline-none checked:border-primary checked:bg-primary"
          checked={checked}
          onChange={(event) => onCheckedChange(event.target.checked)}
        />
        <Check
          aria-hidden="true"
          className="pointer-events-none absolute inset-0 m-auto size-3 text-primary-foreground opacity-0 peer-checked:opacity-100"
        />
      </span>
      {label}
    </label>
  );
}
