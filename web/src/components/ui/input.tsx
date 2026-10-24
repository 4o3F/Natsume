import * as React from "react";

import { cn } from "@/lib/utils";

function Input({ className, type, ...props }: React.ComponentProps<"input">) {
  return (
    <input
      type={type}
      data-slot="input"
      className={cn(
        // Carbon text input: field surface, square, bottom hairline.
        "flex h-10 w-full min-w-0 rounded-none border-0 border-b border-border-strong bg-input px-4 py-2.5 text-sm transition-[color,outline-color,border-color] outline-none selection:bg-primary selection:text-primary-foreground file:inline-flex file:h-7 file:border-0 file:bg-transparent file:text-sm file:font-medium file:text-foreground placeholder:text-subtle-foreground disabled:pointer-events-none disabled:cursor-not-allowed disabled:border-transparent disabled:text-subtle-foreground",
        "focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-ring",
        "aria-invalid:outline-2 aria-invalid:-outline-offset-2 aria-invalid:outline-destructive",
        "[&::-webkit-search-cancel-button]:appearance-none",
        className,
      )}
      {...props}
    />
  );
}

/** Input with a leading icon and optional trailing slot (e.g. a shortcut hint). */
function InputGroup({
  icon,
  trailing,
  className,
  ...props
}: React.ComponentProps<"input"> & {
  icon?: React.ReactNode;
  trailing?: React.ReactNode;
}) {
  return (
    <div
      data-slot="input-group"
      className={cn("relative flex min-w-0 items-center", className)}
    >
      {icon && (
        <span className="pointer-events-none absolute left-3 flex text-muted-foreground [&_svg]:size-4">
          {icon}
        </span>
      )}
      <Input className={cn(icon && "pl-10", trailing && "pr-16")} {...props} />
      {trailing && (
        <span className="absolute right-2 flex items-center gap-1">
          {trailing}
        </span>
      )}
    </div>
  );
}

export { Input, InputGroup };
