import { ChevronDown } from "lucide-react";

import { Button } from "@/components/ui/button";
import {
  DropdownMenu,
  DropdownMenuCheckboxItem,
  DropdownMenuContent,
  DropdownMenuLabel,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";

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
  const summary =
    selected.length === options.length
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
          className="min-w-0 max-w-full justify-between"
          aria-label={`${label}: ${summary}`}
          title={`${label}: ${summary}`}
        >
          <span className="shrink-0">{label}:</span>
          <span className="max-w-48 truncate font-normal">{summary}</span>
          <ChevronDown aria-hidden="true" />
        </Button>
      </DropdownMenuTrigger>
      <DropdownMenuContent
        align="start"
        aria-label={label}
        aria-labelledby={undefined}
      >
        <DropdownMenuLabel>{label}</DropdownMenuLabel>
        <DropdownMenuSeparator />
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
      </DropdownMenuContent>
    </DropdownMenu>
  );
}
