import type {
  AccountResponse_Serialize as AccountResponse,
  BindingResponse,
  DeviceListState,
  DeviceResponse_Serialize as DeviceResponse,
  DeviceSummaries_Serialize as DeviceSummaries,
  DeviceTimeline_Serialize as DeviceTimeline,
  DeviceUpdateRequest,
  EnrollmentReviewResponse,
  EventPage_Serialize as EventPage,
  ImportPendingResponse_Serialize as ImportPendingResponse,
  ImportPreviewResponse_Serialize as ImportPreviewResponse,
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
  OrganizationLogoResponse_Serialize as OrganizationLogoResponse,
  PendingPhotoPage_Serialize as PendingPhotoPage,
  PhotoDecision_Serialize as PhotoDecision,
  PhotoPairingCode,
  PhotoRecord,
  PhotoSource,
  PhotoSourceCreate,
  PhotoSourceStateUpdate,
  PrintQueueEntry_Serialize as PrintQueueEntry,
  PrintExecution_Serialize as PrintExecution,
  PrintRequeue,
  PrintPrinter_Serialize as PrintPrinter,
  PrintPrinterCreate,
  PrintPairingCode,
  PrintSource,
  PrintSourceCreate,
  PrintSourceIssued,
  ProvisioningWindowRequest,
  ProvisioningWindowResponse,
  ResultImportJob_Serialize as ResultImportJob,
  ResultImportPreview_Serialize as ResultImportPreview,
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

/** Admin-only `JSONL` download, linked directly so the browser streams it. */
export function journalExportHref(query: URLSearchParams) {
  const search = query.toString();
  return `/api/v2/journal/export${search ? `?${search}` : ""}`;
}

/** Admin-only Photo current-image read; the browser loads it directly. */
export function photoImageHref(accountId: string) {
  return `/api/v2/photo/photos/${encodeURIComponent(accountId)}/image`;
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
    listPrintExecutions: (options?: RequestOptions) =>
      request<PrintExecution[]>("GET", "/api/v2/print/executions", options),
    requeuePrintExecution: (
      sequence: number,
      body: PrintRequeue,
      options?: RequestOptions,
    ) =>
      json<void>(
        "POST",
        `/api/v2/print/executions/${sequence}/requeue`,
        body,
        options,
      ),
    listPrintPrinters: (options?: RequestOptions) =>
      request<PrintPrinter[]>("GET", "/api/v2/print/printers", options),
    createPrintPrinter: (body: PrintPrinterCreate, options?: RequestOptions) =>
      json<PrintPrinter>("POST", "/api/v2/print/printers", body, options),
    issuePrintPairingCode: (id: string, options?: RequestOptions) =>
      json<PrintPairingCode>(
        "POST",
        `/api/v2/print/printers/${encodeURIComponent(id)}/pairing-code`,
        undefined,
        options,
      ),
    clearPrintPrinterBinding: (id: string, options?: RequestOptions) =>
      request<void>(
        "DELETE",
        `/api/v2/print/printers/${encodeURIComponent(id)}/binding`,
        options,
      ),
    listPrintSources: (options?: RequestOptions) =>
      request<PrintSource[]>("GET", "/api/v2/print/sources", options),
    createPrintSource: (body: PrintSourceCreate, options?: RequestOptions) =>
      json<PrintSourceIssued>("POST", "/api/v2/print/sources", body, options),
    revokePrintSource: (id: string, options?: RequestOptions) =>
      request<void>(
        "DELETE",
        `/api/v2/print/sources/${encodeURIComponent(id)}`,
        options,
      ),
    listPhotoSources: (options?: RequestOptions) =>
      request<PhotoSource[]>("GET", "/api/v2/photo/sources", options),
    createPhotoSource: (body: PhotoSourceCreate, options?: RequestOptions) =>
      json<PhotoSource>("POST", "/api/v2/photo/sources", body, options),
    issuePhotoPairingCode: (id: string, options?: RequestOptions) =>
      json<PhotoPairingCode>(
        "POST",
        `/api/v2/photo/sources/${encodeURIComponent(id)}/pairing-code`,
        {},
        options,
      ),
    resetPhotoEnrollment: (id: string, options?: RequestOptions) =>
      json<PhotoPairingCode>(
        "POST",
        `/api/v2/photo/sources/${encodeURIComponent(id)}/reset-enrollment`,
        undefined,
        options,
      ),
    updatePhotoSourceState: (
      id: string,
      body: PhotoSourceStateUpdate,
      options?: RequestOptions,
    ) =>
      json<void>(
        "POST",
        `/api/v2/photo/sources/${encodeURIComponent(id)}/state`,
        body,
        options,
      ),
    uploadPhotoImport: (file: Blob, options?: RequestOptions) =>
      request<ResultImportJob>("POST", "/api/v2/photo/imports", options, file, {
        "Content-Type": "application/zip",
      }),
    getPhotoImport: (id: string, options?: RequestOptions) =>
      request<ResultImportJob>(
        "GET",
        `/api/v2/photo/imports/${encodeURIComponent(id)}`,
        options,
      ),
    previewPhotoImport: (id: string, options?: RequestOptions) =>
      request<ResultImportPreview>(
        "GET",
        `/api/v2/photo/imports/${encodeURIComponent(id)}/preview`,
        options,
      ),
    commitPhotoImport: (id: string, options?: RequestOptions) =>
      json<ResultImportPreview>(
        "POST",
        `/api/v2/photo/imports/${encodeURIComponent(id)}/commit`,
        undefined,
        options,
      ),
    decidePhotoImport: (
      id: string,
      body: PhotoDecision,
      options?: RequestOptions,
    ) =>
      json<ResultImportPreview>(
        "POST",
        `/api/v2/photo/imports/${encodeURIComponent(id)}/decisions`,
        body,
        options,
      ),
    listPhotoSubmissions: (
      cursor: { source: string; submission: string } | null,
      options?: RequestOptions,
    ) => {
      const query = new URLSearchParams({ limit: "100" });
      if (cursor) {
        query.set("after_source_id", cursor.source);
        query.set("after_submission_id", cursor.submission);
      }
      return request<PendingPhotoPage>(
        "GET",
        `/api/v2/photo/submissions?${query}`,
        options,
      );
    },
    listPhotos: (options?: RequestOptions) =>
      request<PhotoRecord[]>("GET", "/api/v2/photo/photos", options),
    exportPhotos: (options?: RequestOptions) =>
      request<Blob>(
        "POST",
        "/api/v2/photo/exports",
        options,
        undefined,
        undefined,
        true,
      ),
    exportPyrite: (options?: RequestOptions) =>
      request<Blob>(
        "POST",
        "/api/v2/photo/exports/pyrite",
        options,
        undefined,
        undefined,
        true,
      ),
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
    listJournalEvents: (query: URLSearchParams, options?: RequestOptions) => {
      const search = query.toString();
      return request<EventPage>(
        "GET",
        `/api/v2/journal/events${search ? `?${search}` : ""}`,
        options,
      );
    },
    getDeviceTimeline: (
      id: string,
      window: { from: number; to: number },
      options?: RequestOptions,
    ) =>
      request<DeviceTimeline>(
        "GET",
        `/api/v2/journal/devices/${encodeURIComponent(id)}/timeline?${new URLSearchParams(
          { from: String(window.from), to: String(window.to) },
        )}`,
        options,
      ),
    listDeviceSummaries: (windowMs: number, options?: RequestOptions) =>
      request<DeviceSummaries>(
        "GET",
        `/api/v2/journal/device-summaries?${new URLSearchParams({
          window_ms: String(windowMs),
        })}`,
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
