import type { ReactNode } from "react";
import { X } from "lucide-react";

import { Button } from "@/components/ui/button";

/** Floating action bar shown while table rows are selected. */
export function BulkActionBar({
  count,
  noun = "device",
  onClear,
  children,
}: {
  count: number;
  noun?: string;
  onClear: () => void;
  children: ReactNode;
}) {
  if (count === 0) return null;
  return (
    <div className="pointer-events-none fixed inset-x-0 bottom-6 z-40 flex justify-center px-4">
      <div
        role="toolbar"
        aria-label="Selection actions"
        className="pointer-events-auto flex max-w-full animate-in items-center gap-1 overflow-x-auto border bg-popover p-1.5 pl-4 text-popover-foreground shadow-lg duration-200 fade-in-0 slide-in-from-bottom-4"
      >
        <span className="mr-2 shrink-0 text-sm font-medium tabular-nums">
          {count} {noun}
          {count === 1 ? "" : "s"} selected
        </span>
        <span aria-hidden="true" className="mr-1 h-5 w-px shrink-0 bg-border" />
        {children}
        <span aria-hidden="true" className="mx-1 h-5 w-px shrink-0 bg-border" />
        <Button
          type="button"
          variant="ghost"
          size="icon-sm"
          aria-label="Clear selection"
          onClick={onClear}
        >
          <X aria-hidden="true" />
        </Button>
      </div>
    </div>
  );
}
