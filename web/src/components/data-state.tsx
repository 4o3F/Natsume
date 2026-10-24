import type { ReactNode } from "react";
import { CircleAlert, Inbox } from "lucide-react";

import { ApiError } from "@/api/errors";
import { Skeleton } from "@/components/ui/skeleton";
import { cn } from "@/lib/utils";

interface DataStateProps {
  isLoading: boolean;
  error: unknown;
  isEmpty: boolean;
  emptyLabel: string;
  emptyDescription?: ReactNode;
  emptyAction?: ReactNode;
  children: ReactNode;
}

export function DataState({
  isLoading,
  error,
  isEmpty,
  emptyLabel,
  emptyDescription,
  emptyAction,
  children,
}: DataStateProps) {
  if (isLoading) {
    return (
      <div
        role="status"
        aria-label="Loading"
        className="overflow-hidden border bg-card"
      >
        <div className="h-12 bg-surface-2" />
        {Array.from({ length: 6 }, (_, index) => (
          <div
            key={index}
            className="flex h-12 items-center gap-4 border-b px-4 last:border-0"
          >
            <Skeleton className="h-3.5 w-14" />
            <Skeleton className="h-3.5 w-40" />
            <Skeleton className="ml-auto h-3.5 w-24" />
          </div>
        ))}
        <span className="sr-only">Loading...</span>
      </div>
    );
  }

  if (error instanceof ApiError || error instanceof Error) {
    return (
      <EmptyState
        tone="danger"
        icon={<CircleAlert aria-hidden="true" />}
        title={error instanceof ApiError ? error.title : error.message}
        description="The last request failed. The list will retry automatically."
      />
    );
  }

  if (isEmpty) {
    return (
      <EmptyState
        icon={<Inbox aria-hidden="true" />}
        title={emptyLabel}
        description={emptyDescription}
        action={emptyAction}
      />
    );
  }

  return children;
}

export function EmptyState({
  icon,
  title,
  description,
  action,
  tone = "muted",
  className,
}: {
  icon?: ReactNode;
  title: ReactNode;
  description?: ReactNode;
  action?: ReactNode;
  tone?: "muted" | "danger";
  className?: string;
}) {
  return (
    <div
      role={tone === "danger" ? "alert" : undefined}
      className={cn(
        "flex flex-col items-center justify-center gap-2 border bg-card px-6 py-12 text-center",
        className,
      )}
    >
      {icon && (
        <span
          className={cn(
            "mb-1 flex size-10 items-center justify-center [&_svg]:size-5",
            tone === "danger"
              ? "bg-destructive-subtle text-destructive"
              : "bg-muted text-muted-foreground",
          )}
        >
          {icon}
        </span>
      )}
      <p
        className={cn(
          "text-sm font-medium",
          tone === "danger" && "text-destructive",
        )}
      >
        {title}
      </p>
      {description && (
        <p className="max-w-sm text-sm text-muted-foreground">{description}</p>
      )}
      {action && <div className="mt-2">{action}</div>}
    </div>
  );
}
