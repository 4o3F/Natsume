import { useEffect, useState, type ReactNode } from "react";
import { EyeOff, KeyRound, Timer } from "lucide-react";

import { CopyButton } from "@/components/copy-button";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { countdown } from "@/pages/print-status";
import { useNow } from "@/lib/use-now";
import { cn } from "@/lib/utils";

/**
 * One-time credential reveal. `code` renders a short code large for reading
 * aloud or typing; `token` renders a long secret in a wrapped mono block.
 */
export function PrintSecret({
  value,
  title,
  description,
  expires,
  kind = "token",
  meta,
  onDismiss,
}: {
  value: string;
  title: string;
  description: ReactNode;
  expires?: number;
  kind?: "code" | "token";
  meta?: ReactNode;
  onDismiss: () => void;
}) {
  const [issuedAt] = useState(Date.now);
  const now = useNow(1_000, expires !== undefined);
  useEffect(() => {
    if (expires === undefined) return;
    const timer = window.setTimeout(
      onDismiss,
      Math.max(0, expires - Date.now()),
    );
    return () => window.clearTimeout(timer);
  }, [expires, onDismiss]);
  const remaining = expires === undefined ? null : countdown(expires, now);
  const fraction =
    expires === undefined
      ? 0
      : Math.max(0, Math.min(1, (expires - now) / (expires - issuedAt || 1)));

  return (
    <section
      aria-label={title}
      className="relative overflow-hidden border border-l-4 border-l-primary bg-card motion-safe:animate-in motion-safe:fade-in motion-safe:slide-in-from-top-2"
    >
      <div className="flex flex-col gap-5 p-5 sm:flex-row sm:items-center sm:p-6">
        <div className="min-w-0 flex-1 space-y-3">
          <div className="flex flex-wrap items-center gap-2">
            <span className="flex size-8 items-center justify-center bg-info-subtle text-primary">
              <KeyRound aria-hidden="true" className="size-4" />
            </span>
            <p className="font-semibold">{title}</p>
            <Badge variant="warning">Shown once</Badge>
          </div>
          <div
            className={cn(
              "border-b border-border-strong bg-muted select-all",
              kind === "code"
                ? "inline-block px-5 py-3 font-mono text-3xl font-light tracking-[0.25em] sm:text-4xl"
                : "block px-4 py-3 font-mono text-sm break-all",
            )}
          >
            {value}
          </div>
          <div className="space-y-1 text-sm text-muted-foreground">
            {description}
          </div>
          {meta && <div className="text-xs text-muted-foreground">{meta}</div>}
        </div>
        <div className="flex shrink-0 flex-row gap-2 sm:w-44 sm:flex-col">
          <CopyButton
            value={value}
            label={kind === "code" ? "Copy code" : "Copy token"}
            variant="default"
            size="default"
            className="flex-1 sm:flex-none"
          >
            {kind === "code" ? "Copy code" : "Copy token"}
          </CopyButton>
          <Button
            type="button"
            variant="outline"
            className="flex-1 sm:flex-none"
            onClick={onDismiss}
          >
            <EyeOff aria-hidden="true" />
            Dismiss
          </Button>
        </div>
      </div>
      {expires !== undefined && (
        <div className="flex items-center gap-3 border-t bg-muted px-5 py-2.5 text-caption text-muted-foreground sm:px-6">
          <Timer aria-hidden="true" className="size-3.5 shrink-0" />
          <span className="tabular-nums">
            {remaining ? `Expires in ${remaining}` : "Expired"} ·{" "}
            {new Date(expires).toLocaleTimeString()}
          </span>
          <span className="sr-only">
            Server validation remains authoritative.
          </span>
          <div
            aria-hidden="true"
            className="ml-auto h-1 w-32 overflow-hidden bg-surface-2"
          >
            <div
              className={cn(
                "h-full transition-[width] duration-1000 ease-linear",
                fraction > 0.25 ? "bg-primary" : "bg-warning",
              )}
              style={{ width: `${fraction * 100}%` }}
            />
          </div>
        </div>
      )}
    </section>
  );
}
