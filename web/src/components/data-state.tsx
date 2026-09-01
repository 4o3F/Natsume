import type { ReactNode } from "react";

import { ApiError } from "@/api/errors";

interface DataStateProps {
  isLoading: boolean;
  error: unknown;
  isEmpty: boolean;
  emptyLabel: string;
  children: ReactNode;
}

export function DataState({
  isLoading,
  error,
  isEmpty,
  emptyLabel,
  children,
}: DataStateProps) {
  if (isLoading) {
    return <p className="text-sm text-muted-foreground">Loading...</p>;
  }

  if (error instanceof ApiError) {
    return <p className="text-sm text-destructive">{error.title}</p>;
  }

  if (error instanceof Error) {
    return <p className="text-sm text-destructive">{error.message}</p>;
  }

  if (isEmpty) {
    return <p className="text-sm text-muted-foreground">{emptyLabel}</p>;
  }

  return children;
}
