import type { components } from "./generated/schema";

type ErrorResponse = components["schemas"]["ErrorResponse"];

export class ApiError extends Error {
  code: string;
  status: number;
  title: string;
  correlationId: string | null;

  constructor(
    code: string,
    status: number,
    title: string,
    correlationId: string | null,
  ) {
    super(title);
    this.name = "ApiError";
    this.code = code;
    this.status = status;
    this.title = title;
    this.correlationId = correlationId;
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
    const {
      code,
      status,
      title,
      correlation_id: correlationId,
    } = error as Partial<ErrorResponse>;

    if (
      typeof code === "string" &&
      typeof status === "number" &&
      typeof title === "string" &&
      typeof correlationId === "string"
    ) {
      throw new ApiError(code, status, title, correlationId);
    }
  }

  throw new ApiError(
    "UNPARSEABLE_RESPONSE",
    response.status,
    `HTTP ${response.status}`,
    response.headers.get("X-Correlation-Id"),
  );
}
