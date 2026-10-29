import * as React from "react";
import { Slot } from "@radix-ui/react-slot";
import { cva, type VariantProps } from "class-variance-authority";
import { LoaderCircle } from "lucide-react";

import { cn } from "@/lib/utils";

const buttonVariants = cva(
  // Carbon buttons: square, 14px/400 labels, 2px inset focus outline.
  "inline-flex shrink-0 cursor-pointer items-center justify-center gap-2 rounded-none border border-transparent text-sm font-normal whitespace-nowrap transition-[color,background-color,border-color] outline-none select-none focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-ring disabled:pointer-events-none disabled:opacity-50 aria-invalid:border-destructive [&_svg]:pointer-events-none [&_svg]:shrink-0 [&_svg:not([class*='size-'])]:size-4",
  {
    variants: {
      variant: {
        /** Carbon primary: IBM Blue. */
        default:
          "bg-primary text-primary-foreground hover:bg-primary-hover active:bg-primary-active",
        /** Carbon danger primary. */
        destructive:
          "bg-destructive text-destructive-foreground hover:bg-destructive-hover",
        /** Carbon danger ghost. */
        "destructive-soft":
          "text-destructive hover:bg-destructive hover:text-destructive-foreground",
        /** Carbon danger tertiary. */
        "destructive-outline":
          "border-destructive text-destructive hover:bg-destructive hover:text-destructive-foreground",
        success: "bg-success text-success-foreground hover:bg-success/90",
        /** Carbon tertiary: blue outline that fills on hover. */
        outline:
          "border-primary bg-transparent text-primary hover:bg-primary-hover hover:text-primary-foreground active:bg-primary-active dark:border-foreground dark:text-foreground dark:hover:border-primary-hover dark:hover:text-primary-foreground",
        /** Carbon secondary: charcoal. */
        secondary: "bg-inverse text-inverse-foreground hover:bg-inverse-hover",
        /** Carbon ghost: blue label, neutral hover layer. */
        ghost: "text-link hover:bg-accent active:bg-surface-2",
        link: "h-auto border-0 px-0 text-link underline-offset-4 hover:underline",
      },
      size: {
        default: "h-10 px-4",
        xs: "h-6 gap-1.5 px-2 text-xs touch:h-12 [&_svg:not([class*='size-'])]:size-3.5",
        sm: "h-8 gap-1.5 px-3 touch:h-12",
        lg: "h-12 px-4",
        icon: "size-10",
        "icon-sm": "size-8",
        "icon-xs": "size-6 [&_svg:not([class*='size-'])]:size-3.5",
      },
    },
    compoundVariants: [
      // Carbon icon-only ghost buttons use the neutral icon colour.
      {
        variant: "ghost",
        size: ["icon", "icon-sm", "icon-xs"],
        className: "text-foreground",
      },
    ],
    defaultVariants: {
      variant: "default",
      size: "default",
    },
  },
);

function Button({
  className,
  variant = "default",
  size = "default",
  asChild = false,
  loading = false,
  disabled,
  children,
  ...props
}: React.ComponentProps<"button"> &
  VariantProps<typeof buttonVariants> & {
    asChild?: boolean;
    /** Shows a spinner in place of the leading icon and disables the button. */
    loading?: boolean;
  }) {
  const Comp = asChild ? Slot : "button";

  return (
    <Comp
      data-slot="button"
      className={cn(buttonVariants({ variant, size, className }))}
      disabled={disabled || loading}
      aria-busy={loading || undefined}
      {...props}
    >
      {asChild ? (
        children
      ) : (
        <>
          {loading && (
            <LoaderCircle aria-hidden="true" className="animate-spin" />
          )}
          {children}
        </>
      )}
    </Comp>
  );
}

// eslint-disable-next-line react-refresh/only-export-components
export { Button, buttonVariants };
