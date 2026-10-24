import * as React from "react";
import { cva, type VariantProps } from "class-variance-authority";

import { cn } from "@/lib/utils";

const alertVariants = cva(
  "relative grid w-full grid-cols-[0_1fr] items-start gap-y-0.5 rounded-none border border-l-4 px-4 py-3 text-sm text-foreground has-[>svg]:grid-cols-[calc(var(--spacing)*4)_1fr] has-[>svg]:gap-x-3 [&>svg]:size-4 [&>svg]:translate-y-0.5",
  {
    variants: {
      variant: {
        // Carbon inline notifications: tinted surface, 4px semantic edge.
        default: "border-l-foreground bg-card",
        destructive:
          "border-destructive/40 border-l-destructive bg-destructive-subtle [&>svg]:text-destructive *:data-[slot=alert-description]:text-foreground",
        warning:
          "border-warning/50 border-l-warning bg-warning-subtle [&>svg]:text-warning-strong *:data-[slot=alert-description]:text-foreground",
        success:
          "border-success/40 border-l-success bg-success-subtle [&>svg]:text-success-strong *:data-[slot=alert-description]:text-foreground",
        info: "border-info/40 border-l-info bg-info-subtle [&>svg]:text-info *:data-[slot=alert-description]:text-foreground",
      },
    },
    defaultVariants: {
      variant: "default",
    },
  },
);

function Alert({
  className,
  variant,
  ...props
}: React.ComponentProps<"div"> & VariantProps<typeof alertVariants>) {
  return (
    <div
      data-slot="alert"
      role="alert"
      className={cn(alertVariants({ variant }), className)}
      {...props}
    />
  );
}

function AlertTitle({ className, ...props }: React.ComponentProps<"div">) {
  return (
    <div
      data-slot="alert-title"
      className={cn("col-start-2 min-h-4 font-semibold", className)}
      {...props}
    />
  );
}

function AlertDescription({
  className,
  ...props
}: React.ComponentProps<"div">) {
  return (
    <div
      data-slot="alert-description"
      className={cn(
        "col-start-2 grid justify-items-start gap-1 text-sm text-muted-foreground [&_p]:leading-relaxed",
        className,
      )}
      {...props}
    />
  );
}

export { Alert, AlertTitle, AlertDescription };
