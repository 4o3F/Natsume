import type { ReactNode } from "react";
import { Check } from "lucide-react";

import { cn } from "@/lib/utils";

export function CheckboxFilter({
  label,
  children,
}: {
  label: string;
  children: ReactNode;
}) {
  return (
    <fieldset className="flex min-w-0 flex-wrap items-center gap-1.5">
      <legend className="sr-only">{label}</legend>
      <span
        aria-hidden="true"
        className="mr-1 text-xs font-medium text-muted-foreground"
      >
        {label}
      </span>
      {children}
    </fieldset>
  );
}

/** Toggle chip backed by a native checkbox for form semantics. */
export function CheckboxFilterOption({
  label,
  checked,
  onCheckedChange,
  count,
}: {
  label: string;
  checked: boolean;
  onCheckedChange: (checked: boolean) => void;
  count?: number;
}) {
  return (
    <label
      className={cn(
        // Carbon selectable tag: square, charcoal when selected.
        "flex h-8 cursor-pointer items-center gap-1.5 border border-border-strong bg-transparent px-3 text-sm text-foreground transition-colors select-none hover:bg-accent",
        "has-checked:border-inverse has-checked:bg-inverse has-checked:text-inverse-foreground",
        "has-focus-visible:outline-2 has-focus-visible:outline-offset-1 has-focus-visible:outline-ring",
      )}
    >
      <input
        type="checkbox"
        className="peer sr-only"
        checked={checked}
        onChange={(event) => onCheckedChange(event.target.checked)}
      />
      <Check
        aria-hidden="true"
        className="hidden size-3.5 peer-checked:block"
      />
      {label}
      {count !== undefined && (
        <span className="text-xs tabular-nums opacity-70">{count}</span>
      )}
    </label>
  );
}
