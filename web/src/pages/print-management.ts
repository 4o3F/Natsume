import { ApiError } from "../api/errors";

export function printManagementError(error: unknown) {
  if (!(error instanceof ApiError))
    return "Print request failed. Refresh and verify the result before repeating an operation.";
  if (error.status === 409)
    return "Print state changed or a binding or active attempt prevents this operation. Refresh and verify before trying again.";
  if (error.status === 403) return "Administrator access is required.";
  if (error.status === 404)
    return "This print record no longer exists. Refresh the list.";
  if (error.status === 400) return "Check the print form values and try again.";
  return "Print request failed. Refresh and verify the result before repeating an operation.";
}

export function validPrintName(name: string) {
  return (
    Array.from(name).length >= 1 &&
    Array.from(name).length <= 128 &&
    !/[\p{Cc}]/u.test(name)
  );
}
