import { useEffect, useRef } from "react";
import { toast } from "sonner";

// Late callbacks must not create notifications after the page or session changes.
export function usePageToast() {
  const active = useRef(false);
  useEffect(() => {
    active.current = true;
    return () => {
      active.current = false;
    };
  }, []);

  const notify = (
    kind: "success" | "error" | "warning",
    ...args: Parameters<typeof toast.success>
  ) => {
    if (active.current) toast[kind](...args);
  };

  return {
    success: (...args: Parameters<typeof toast.success>) =>
      notify("success", ...args),
    error: (...args: Parameters<typeof toast.error>) =>
      notify("error", ...args),
    warning: (...args: Parameters<typeof toast.warning>) =>
      notify("warning", ...args),
  };
}
