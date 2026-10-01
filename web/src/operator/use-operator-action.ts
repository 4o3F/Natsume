import { useEffect, useRef, useState } from "react";

import { ApiError } from "@/api/errors";

export function operatorErrorMessage(error: unknown) {
  if (!(error instanceof ApiError)) return "Request failed. Please try again.";
  switch (error.code) {
    case "OPERATOR_LOGIN_NAME_CONFLICT":
      return "That username is already in use. Choose another and try again.";
    case "OPERATOR_LAST_ADMIN":
      return "The last administrator cannot be deleted or changed to Viewer.";
    case "OPERATOR_CURRENT_PASSWORD_INVALID":
      return "The current password is incorrect.";
    case "OPERATOR_CREDENTIAL_CHANGED":
      return "Your password changed during this request. Try again with your current password.";
    case "OPERATOR_LINK_UNAVAILABLE":
      return "This link is no longer available. Contact an administrator.";
    case "OPERATOR_LOGOUT_REQUIRED":
      return "Sign out before using this link.";
    case "INVALID_REQUEST":
      return "Check the form values and try again.";
    case "SERVICE_UNAVAILABLE":
      return "The server is busy. Please try again shortly.";
    case "RESOURCE_NOT_FOUND":
      return "This user or invitation no longer exists. Refresh the list.";
    case "AUTHORIZATION_DENIED":
      return "Administrator access is required.";
    default:
      return "Request failed. Please try again.";
  }
}

// Operator secrets and responses never enter the Query mutation cache.
// The page owns cancellation, including navigation within one session generation.
export function useOperatorAction() {
  const controller = useRef<AbortController | null>(null);
  const busy = useRef(false);
  const [pending, setPending] = useState(false);
  const [error, setError] = useState<string | null>(null);
  useEffect(() => {
    const current = new AbortController();
    controller.current = current;
    return () => current.abort();
  }, []);

  async function run<T>(
    operation: (signal: AbortSignal) => Promise<T>,
    onSuccess: (result: T) => void,
  ) {
    const signal = controller.current?.signal;
    if (!signal || signal.aborted || busy.current) return;
    busy.current = true;
    setPending(true);
    setError(null);
    try {
      const result = await operation(signal);
      if (!signal.aborted) onSuccess(result);
    } catch (error) {
      if (!signal.aborted) setError(operatorErrorMessage(error));
    } finally {
      if (!signal.aborted) {
        busy.current = false;
        setPending(false);
      }
    }
  }

  return { run, pending, error };
}
