import { Check, ChevronDown, ListFilter } from "lucide-react";

import { Button } from "@/components/ui/button";
import {
  DropdownMenu,
  DropdownMenuCheckboxItem,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { cn } from "@/lib/utils";

/** Chip-styled multi-select. Narrowed selections are highlighted. */
export function MultiSelectFilter<T extends string>({
  label,
  options,
  value,
  onChange,
}: {
  label: string;
  options: readonly { value: T; label: string }[];
  value: Record<T, boolean>;
  onChange: (value: Record<T, boolean>) => void;
}) {
  const selected = options.filter((option) => value[option.value]);
  const all = selected.length === options.length;
  const summary = all
    ? "All"
    : selected.length === 0
      ? "None"
      : selected.map((option) => option.label).join(", ");

  return (
    <DropdownMenu modal={false}>
      <DropdownMenuTrigger asChild>
        <Button
          type="button"
          variant="outline"
          size="sm"
          className={cn(
            "max-w-full min-w-0 justify-between",
            !all &&
              "border-primary bg-primary text-primary-foreground hover:bg-primary-hover dark:border-primary dark:text-primary-foreground",
          )}
          aria-label={`${label}: ${summary}`}
          title={`${label}: ${summary}`}
        >
          <ListFilter aria-hidden="true" />
          <span className="shrink-0">{label}</span>
          {!all && (
            <>
              <span
                aria-hidden="true"
                className="h-3.5 w-px bg-current opacity-40"
              />
              <span className="max-w-40 truncate font-normal">{summary}</span>
            </>
          )}
          <ChevronDown aria-hidden="true" />
        </Button>
      </DropdownMenuTrigger>
      <DropdownMenuContent
        align="start"
        aria-label={label}
        aria-labelledby={undefined}
        className="w-60"
      >
        <DropdownMenuLabel>{label}</DropdownMenuLabel>
        {options.map((option) => (
          <DropdownMenuCheckboxItem
            key={option.value}
            checked={value[option.value]}
            onSelect={(event) => event.preventDefault()}
            onCheckedChange={(checked) =>
              onChange({ ...value, [option.value]: checked === true })
            }
          >
            {option.label}
          </DropdownMenuCheckboxItem>
        ))}
        {!all && (
          <>
            <DropdownMenuSeparator />
            <DropdownMenuItem
              onSelect={() =>
                onChange(
                  Object.fromEntries(
                    options.map((option) => [option.value, true]),
                  ) as Record<T, boolean>,
                )
              }
            >
              <Check aria-hidden="true" />
              Select all
            </DropdownMenuItem>
          </>
        )}
      </DropdownMenuContent>
    </DropdownMenu>
  );
}
