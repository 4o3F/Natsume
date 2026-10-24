import { useEffect, type CSSProperties } from "react";
import { toast, Toaster as Sonner, type ToasterProps } from "sonner";

import { useTheme } from "@/components/theme-provider";

function Toaster(props: ToasterProps) {
  const { resolvedTheme } = useTheme();
  useEffect(
    () => () => {
      toast.dismiss();
    },
    [],
  );
  return (
    <Sonner
      theme={resolvedTheme}
      position="bottom-right"
      closeButton
      richColors
      className="toaster group"
      style={
        {
          "--normal-bg": "var(--popover)",
          "--normal-text": "var(--popover-foreground)",
          "--normal-border": "var(--border)",
          "--border-radius": "var(--radius)",
        } as CSSProperties
      }
      {...props}
    />
  );
}

export { Toaster };
