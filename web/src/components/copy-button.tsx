import { useEffect, useState, type ReactNode } from "react";
import { Check, Copy } from "lucide-react";

import { usePageToast } from "@/components/use-page-toast";

import { Button } from "@/components/ui/button";
import {
  Tooltip,
  TooltipContent,
  TooltipTrigger,
} from "@/components/ui/tooltip";

/** Copies `value`; icon-only unless `children` is given. */
export function CopyButton({
  value,
  label,
  children,
  variant = "ghost",
  size,
  className,
}: {
  value: string;
  label: string;
  children?: ReactNode;
  variant?: "ghost" | "outline" | "secondary" | "default";
  size?: "xs" | "sm" | "default" | "icon-xs" | "icon-sm";
  className?: string;
}) {
  const toast = usePageToast();
  const [copied, setCopied] = useState(false);
  useEffect(() => {
    if (!copied) return;
    const timer = window.setTimeout(() => setCopied(false), 1_600);
    return () => window.clearTimeout(timer);
  }, [copied]);
  const copy = async () => {
    try {
      if (!navigator.clipboard) throw new Error("unavailable");
      await navigator.clipboard.writeText(value);
      setCopied(true);
    } catch {
      toast.error("Copy failed", {
        description: "Select the value and copy it manually.",
      });
    }
  };
  const icon = copied ? (
    <Check aria-hidden="true" className="text-success-strong" />
  ) : (
    <Copy aria-hidden="true" />
  );
  if (children)
    return (
      <Button
        type="button"
        variant={variant}
        size={size ?? "sm"}
        className={className}
        onClick={() => void copy()}
      >
        {icon}
        {copied ? "Copied" : children}
      </Button>
    );
  return (
    <Tooltip>
      <TooltipTrigger asChild>
        <Button
          type="button"
          variant={variant}
          size={size ?? "icon-xs"}
          aria-label={label}
          className={className}
          onClick={() => void copy()}
        >
          {icon}
        </Button>
      </TooltipTrigger>
      <TooltipContent>{copied ? "Copied" : label}</TooltipContent>
    </Tooltip>
  );
}
