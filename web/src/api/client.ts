import type {
  AccountResponse,
  BindingResponse,
  DeviceListState,
  DeviceResponse_Serialize as DeviceResponse,
  DeviceUpdateRequest,
  EnrollmentReviewResponse,
  ImportPendingResponse,
  ImportPreviewResponse,
  OperatorAccountResponse,
  OperatorInvitationIssuedResponse,
  OperatorInvitationResponse,
  OperatorPasswordChangeRequest,
  OperatorPasswordResetInspectionResponse,
  OperatorPasswordResetIssuedResponse,
  OperatorPasswordResetRequest,
  OperatorRegistrationInspectionResponse,
  OperatorRegistrationRequest,
  OperatorRoleRequest,
  OperatorTokenRequest,
  OrganizationLogoResponse,
  PrintQueueEntry,
  ProvisioningWindowRequest,
  ProvisioningWindowResponse,
  SeatResponse,
  SessionRequest,
  SessionResponse,
  TargetSubmissionBody,
  TargetSubmissionResponse,
} from "./generated/types";

export interface ApiResult<T> {
  data?: T;
  error?: unknown;
  response: Response;
}

interface RequestOptions {
  signal?: AbortSignal;
  baseUrl?: string;
}

const XLSX_CONTENT_TYPE =
  "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet";

export function createApiClient(
  signal: AbortSignal,
  onUnauthorized: () => void,
) {
  async function request<T>(
    method: string,
    path: string,
    options: RequestOptions = {},
    body?: BodyInit,
    headers?: HeadersInit,
    binaryResponse = false,
  ): Promise<ApiResult<T>> {
    signal.throwIfAborted();
    const request = new Request(
      new URL(path, options.baseUrl ?? window.location.origin),
      {
        method,
        body,
        headers,
        signal: options.signal
          ? AbortSignal.any([signal, options.signal])
          : signal,
      },
    );
    request.signal.throwIfAborted();
    const response = await fetch(request);
    signal.throwIfAborted();
    if (
      response.status === 401 &&
      !(method === "POST" && path === "/api/v2/session")
    ) {
      onUnauthorized();
    }
    if (
      response.status === 204 ||
      response.headers.get("content-length") === "0"
    )
      return { data: undefined, response };
    if (response.ok && binaryResponse)
      return { data: (await response.blob()) as T, response };

    const text = await response.text();
    if (response.ok)
      return { data: (text ? JSON.parse(text) : undefined) as T, response };
    let error: unknown = text;
    try {
      error = JSON.parse(text);
    } catch {
      // Non-JSON errors retain the HTTP status for the shared error handler.
    }
    return { error, response };
  }

  function json<T>(
    method: string,
    path: string,
    body: unknown,
    options?: RequestOptions,
  ) {
    return request<T>(
      method,
      path,
      options,
      body === undefined ? undefined : JSON.stringify(body),
      body === undefined ? undefined : { "Content-Type": "application/json" },
    );
  }

  return {
    getSession: (options?: RequestOptions) =>
      request<SessionResponse>("GET", "/api/v2/session", options),
    createSession: (body: SessionRequest, options?: RequestOptions) =>
      json<SessionResponse>("POST", "/api/v2/session", body, options),
    deleteSession: (options?: RequestOptions) =>
      request<void>("DELETE", "/api/v2/session", options),
    listPrintQueue: (options?: RequestOptions) =>
      request<PrintQueueEntry[]>("GET", "/api/v2/print/queue", options),
    listSeats: (options?: RequestOptions) =>
      request<SeatResponse[]>("GET", "/api/v2/seats", options),
    listAccounts: (options?: RequestOptions) =>
      request<AccountResponse[]>("GET", "/api/v2/accounts", options),
    listBindings: (options?: RequestOptions) =>
      request<BindingResponse[]>("GET", "/api/v2/bindings", options),
    listDevices: (states?: DeviceListState[], options?: RequestOptions) => {
      const query = states
        ? `?${new URLSearchParams({ state: states.join(",") })}`
        : "";
      return request<DeviceResponse[]>(
        "GET",
        `/api/v2/devices${query}`,
        options,
      );
    },
    updateDevice: (
      id: string,
      body: DeviceUpdateRequest,
      options?: RequestOptions,
    ) =>
      json<void>(
        "PATCH",
        `/api/v2/devices/${encodeURIComponent(id)}`,
        body,
        options,
      ),
    deleteDeviceBinding: (id: string, options?: RequestOptions) =>
      request<void>(
        "DELETE",
        `/api/v2/devices/${encodeURIComponent(id)}/binding`,
        options,
      ),
    submitTargets: (body: TargetSubmissionBody, options?: RequestOptions) =>
      json<TargetSubmissionResponse>(
        "POST",
        "/api/v2/target-submissions",
        body,
        options,
      ),
    getProvisioningWindow: (options?: RequestOptions) =>
      request<ProvisioningWindowResponse>(
        "GET",
        "/api/v2/provisioning-window",
        options,
      ),
    updateProvisioningWindow: (
      body: ProvisioningWindowRequest,
      options?: RequestOptions,
    ) =>
      json<ProvisioningWindowResponse>(
        "PUT",
        "/api/v2/provisioning-window",
        body,
        options,
      ),
    listEnrollmentReviews: (options?: RequestOptions) =>
      request<EnrollmentReviewResponse[]>(
        "GET",
        "/api/v2/enrollment-reviews",
        options,
      ),
    approveEnrollmentReview: (id: string, options?: RequestOptions) =>
      request<void>(
        "POST",
        `/api/v2/enrollment-reviews/${encodeURIComponent(id)}/actions/approve`,
        options,
      ),
    denyEnrollmentReview: (id: string, options?: RequestOptions) =>
      request<void>(
        "POST",
        `/api/v2/enrollment-reviews/${encodeURIComponent(id)}/actions/deny`,
        options,
      ),
    getImport: (options?: RequestOptions) =>
      request<ImportPendingResponse>("GET", "/api/v2/imports", options),
    createImport: (file: Blob, options?: RequestOptions) =>
      request<ImportPreviewResponse>("POST", "/api/v2/imports", options, file, {
        "Content-Type": XLSX_CONTENT_TYPE,
      }),
    commitImport: (
      id: string,
      token: string,
      file: Blob,
      options?: RequestOptions,
    ) =>
      request<void>(
        "POST",
        `/api/v2/imports/${encodeURIComponent(id)}/actions/commit`,
        options,
        file,
        {
          "Content-Type": XLSX_CONTENT_TYPE,
          "x-natsume-preview-token": token,
        },
      ),
    deleteImport: (id: string, options?: RequestOptions) =>
      request<void>(
        "DELETE",
        `/api/v2/imports/${encodeURIComponent(id)}`,
        options,
      ),
    listOrganizations: (options?: RequestOptions) =>
      request<OrganizationLogoResponse[]>(
        "GET",
        "/api/v2/organizations",
        options,
      ),
    listCandidateOrganizations: (id: string, options?: RequestOptions) =>
      request<OrganizationLogoResponse[]>(
        "GET",
        `/api/v2/imports/${encodeURIComponent(id)}/organizations`,
        options,
      ),
    exportDomjudge: (options?: RequestOptions) =>
      request<Blob>(
        "GET",
        "/api/v2/exports/domjudge",
        options,
        undefined,
        undefined,
        true,
      ),
    listOperatorAccounts: (options?: RequestOptions) =>
      request<OperatorAccountResponse[]>(
        "GET",
        "/api/v2/operator/accounts",
        options,
      ),
    updateOperatorAccount: (
      id: string,
      body: OperatorRoleRequest,
      options?: RequestOptions,
    ) =>
      json<void>(
        "PATCH",
        `/api/v2/operator/accounts/${encodeURIComponent(id)}`,
        body,
        options,
      ),
    deleteOperatorAccount: (id: string, options?: RequestOptions) =>
      request<void>(
        "DELETE",
        `/api/v2/operator/accounts/${encodeURIComponent(id)}`,
        options,
      ),
    listOperatorInvitations: (options?: RequestOptions) =>
      request<OperatorInvitationResponse[]>(
        "GET",
        "/api/v2/operator/invitations",
        options,
      ),
    createOperatorInvitation: (
      body: OperatorRoleRequest,
      options?: RequestOptions,
    ) =>
      json<OperatorInvitationIssuedResponse>(
        "POST",
        "/api/v2/operator/invitations",
        body,
        options,
      ),
    revokeOperatorInvitation: (id: string, options?: RequestOptions) =>
      request<void>(
        "DELETE",
        `/api/v2/operator/invitations/${encodeURIComponent(id)}`,
        options,
      ),
    regenerateOperatorInvitation: (id: string, options?: RequestOptions) =>
      request<OperatorInvitationIssuedResponse>(
        "POST",
        `/api/v2/operator/invitations/${encodeURIComponent(id)}/actions/regenerate`,
        options,
      ),
    createOperatorPasswordReset: (id: string, options?: RequestOptions) =>
      request<OperatorPasswordResetIssuedResponse>(
        "POST",
        `/api/v2/operator/accounts/${encodeURIComponent(id)}/password-resets`,
        options,
      ),
    inspectOperatorRegistration: (
      body: OperatorTokenRequest,
      options?: RequestOptions,
    ) =>
      json<OperatorRegistrationInspectionResponse>(
        "POST",
        "/api/v2/operator/register/inspect",
        body,
        options,
      ),
    registerOperator: (
      body: OperatorRegistrationRequest,
      options?: RequestOptions,
    ) =>
      json<OperatorAccountResponse>(
        "POST",
        "/api/v2/operator/register",
        body,
        options,
      ),
    inspectOperatorPasswordReset: (
      body: OperatorTokenRequest,
      options?: RequestOptions,
    ) =>
      json<OperatorPasswordResetInspectionResponse>(
        "POST",
        "/api/v2/operator/password/reset/inspect",
        body,
        options,
      ),
    resetOperatorPassword: (
      body: OperatorPasswordResetRequest,
      options?: RequestOptions,
    ) => json<void>("POST", "/api/v2/operator/password/reset", body, options),
    changeOperatorPassword: (
      body: OperatorPasswordChangeRequest,
      options?: RequestOptions,
    ) => json<void>("POST", "/api/v2/operator/password/change", body, options),
  };
}
