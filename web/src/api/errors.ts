import type { components } from "./generated/schema";

type ErrorResponse = components["schemas"]["ErrorResponse"];

export class ApiError extends Error {
  code: string;
  status: number;
  title: string;

  constructor(code: string, status: number, title: string) {
    super(title);
    this.name = "ApiError";
    this.code = code;
    this.status = status;
    this.title = title;
  }
}

export async function unwrap<T>({
  data,
  error,
  response,
}: {
  data?: T;
  error?: unknown;
  response: Response;
}): Promise<T> {
  if (response.ok) {
    return data as T;
  }

  if (error && typeof error === "object") {
    const { code, status, title } = error as Partial<ErrorResponse>;

    if (
      typeof code === "string" &&
      typeof status === "number" &&
      typeof title === "string"
    ) {
      throw new ApiError(code, status, title);
    }
  }

  throw new ApiError(
    "UNPARSEABLE_RESPONSE",
    response.status,
    `HTTP ${response.status}`,
  );
}
