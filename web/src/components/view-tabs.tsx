import { useRef, type KeyboardEvent, type ReactNode } from "react";

import { cn } from "@/lib/utils";

/**
 * Tabs switching views of one resource, following the WAI-ARIA tabs pattern
 * (roving focus, arrow/Home/End keys, automatic activation) and styled as the
 * shared segmented toggle group. `ViewTabPanel` renders the matching panel;
 * both take the same `scope` (from `useId`) to link tabs and panels.
 */
export function ViewTabs<T extends string>({
  scope,
  label,
  value,
  onValueChange,
  items,
  className,
}: {
  scope: string;
  label: string;
  value: T;
  onValueChange: (value: T) => void;
  items: readonly { value: T; label: string; icon?: ReactNode }[];
  className?: string;
}) {
  const list = useRef<HTMLDivElement>(null);

  function onKeyDown(event: KeyboardEvent<HTMLButtonElement>, index: number) {
    const next =
      event.key === "ArrowRight"
        ? (index + 1) % items.length
        : event.key === "ArrowLeft"
          ? (index - 1 + items.length) % items.length
          : event.key === "Home"
            ? 0
            : event.key === "End"
              ? items.length - 1
              : null;
    if (next === null) return;
    event.preventDefault();
    onValueChange(items[next].value);
    const tabs =
      list.current?.querySelectorAll<HTMLButtonElement>('[role="tab"]');
    tabs?.[next]?.focus();
  }

  return (
    <div
      ref={list}
      role="tablist"
      aria-label={label}
      className={cn(
        "inline-flex items-center divide-x divide-border-strong border border-border-strong",
        className,
      )}
    >
      {items.map((item, index) => {
        const selected = item.value === value;
        return (
          <button
            key={item.value}
            type="button"
            role="tab"
            id={`${scope}-${item.value}-tab`}
            aria-selected={selected}
            aria-controls={`${scope}-${item.value}-panel`}
            tabIndex={selected ? 0 : -1}
            data-state={selected ? "on" : "off"}
            onClick={() => onValueChange(item.value)}
            onKeyDown={(event) => onKeyDown(event, index)}
            className="inline-flex h-8 touch:h-12 touch:min-w-12 cursor-pointer items-center justify-center gap-1.5 px-3 text-sm font-normal text-muted-foreground transition-colors outline-none hover:bg-accent hover:text-foreground focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-ring data-[state=on]:bg-inverse data-[state=on]:text-inverse-foreground [&_svg]:size-4"
          >
            {item.icon}
            {item.label}
          </button>
        );
      })}
    </div>
  );
}

/** Panel for the selected `ViewTabs` item. */
export function ViewTabPanel({
  scope,
  value,
  children,
  className,
}: {
  scope: string;
  value: string;
  children: ReactNode;
  className?: string;
}) {
  return (
    <div
      role="tabpanel"
      id={`${scope}-${value}-panel`}
      aria-labelledby={`${scope}-${value}-tab`}
      tabIndex={0}
      className={cn(
        "outline-none focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-ring",
        className,
      )}
    >
      {children}
    </div>
  );
}
