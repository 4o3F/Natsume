import { describe, expect, it } from "vitest";

import { ApiError, unwrap } from "./errors";

describe("unwrap", () => {
  it("passes successful response data through, including an empty body", async () => {
    const data = {
      operator_id: "01912345-6789-7abc-8def-0123456789ab",
      role: "admin",
    };

    await expect(
      unwrap({ data, response: new Response(null, { status: 200 }) }),
    ).resolves.toBe(data);
    await expect(
      unwrap<void>({ response: new Response(null, { status: 204 }) }),
    ).resolves.toBeUndefined();
  });

  it("maps an ErrorResponse body to ApiError fields", async () => {
    const correlationId = "01912345-6789-7abc-8def-0123456789ab";
    const result = unwrap({
      error: {
        title: "Authentication failed",
        status: 401,
        code: "AUTHENTICATION_FAILED",
        correlation_id: correlationId,
      },
      response: new Response(null, { status: 401 }),
    });

    await expect(result).rejects.toBeInstanceOf(ApiError);
    await expect(result).rejects.toMatchObject({
      message: "Authentication failed",
      title: "Authentication failed",
      status: 401,
      code: "AUTHENTICATION_FAILED",
      correlationId,
    });
  });

  it("uses the unparseable response fallback when no typed error body exists", async () => {
    await expect(
      unwrap({
        error: "not a response body",
        response: new Response(null, { status: 502 }),
      }),
    ).rejects.toMatchObject({
      title: "HTTP 502",
      status: 502,
      code: "UNPARSEABLE_RESPONSE",
      correlationId: null,
    });
  });
});
