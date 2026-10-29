import type { ReactNode } from "react";

import { cn } from "@/lib/utils";
import { tones, type Tone } from "./device-status-style";

const accentBars: Record<Tone, string> = {
  success: "before:bg-success",
  info: "before:bg-info",
  warning: "before:bg-warning",
  danger: "before:bg-destructive",
  muted: "",
};

/**
 * Summary metric. When `onClick` is given it becomes a toggle that filters
 * the list below; `active` reflects that filter.
 */
export function StatTile({
  label,
  value,
  hint,
  tone = "muted",
  icon,
  active,
  onClick,
}: {
  label: string;
  value: ReactNode;
  hint?: ReactNode;
  tone?: Tone;
  icon?: ReactNode;
  active?: boolean;
  onClick?: () => void;
}) {
  const body = (
    <>
      <div className="flex items-center justify-between gap-2">
        <span className="text-caption text-muted-foreground">{label}</span>
        {icon && (
          <span className={cn("[&_svg]:size-4", tones[tone].text)}>{icon}</span>
        )}
      </div>
      <div className="mt-2 flex items-baseline gap-2">
        <span className="text-headline font-light tabular-nums">{value}</span>
        {hint && (
          <span className="truncate text-xs text-muted-foreground">{hint}</span>
        )}
      </div>
    </>
  );
  const className = cn(
    // Carbon tile: hairline, flat; the status edge is functional colour only.
    "relative min-w-0 overflow-hidden border bg-card px-4 py-4 text-left",
    "before:absolute before:inset-y-0 before:left-0 before:w-1",
    accentBars[tone],
  );
  if (!onClick) return <div className={className}>{body}</div>;
  return (
    <button
      type="button"
      aria-pressed={active}
      onClick={onClick}
      className={cn(
        className,
        "cursor-pointer transition-[border-color,background-color] outline-none hover:bg-accent focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-ring",
        active &&
          "border-foreground bg-muted inset-ring-1 inset-ring-foreground",
      )}
    >
      {body}
    </button>
  );
}

export function StatGrid({
  children,
  className,
}: {
  children: ReactNode;
  className?: string;
}) {
  return (
    <div
      className={cn(
        "grid grid-cols-2 gap-3 sm:grid-cols-3 xl:grid-cols-6",
        className,
      )}
    >
      {children}
    </div>
  );
}
