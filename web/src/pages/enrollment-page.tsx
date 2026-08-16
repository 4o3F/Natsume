import { useMemo, useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import type { ColumnDef } from "@tanstack/react-table";

import { api } from "@/api/client";
import { ApiError, unwrap } from "@/api/errors";
import type { components } from "@/api/generated/schema";
import { LIST_POLL_MS } from "@/api/polling";
import { useSession } from "@/auth/use-session";
import { DataState } from "@/components/data-state";
import { DataTable } from "@/components/data-table";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
  AlertDialogTrigger,
} from "@/components/ui/alert-dialog";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@/components/ui/card";

type EnrollmentRequest = components["schemas"]["EnrollmentRequestSummary"];
type EnrollmentActionResponse =
  components["schemas"]["EnrollmentActionResponse"];
type ProvisioningWindow = components["schemas"]["ProvisioningWindowResponse"];

interface Notice {
  tone: "success" | "error";
  title: string;
  detail?: string;
  correlationId?: string;
}

const ENROLLMENT_REQUESTS_KEY = ["enrollment-requests"] as const;
const PROVISIONING_WINDOW_KEY = ["provisioning-window"] as const;

export function EnrollmentPage() {
  const queryClient = useQueryClient();
  const session = useSession().data;
  const isAdmin = session?.role === "admin";
  const [notice, setNotice] = useState<Notice | null>(null);

  const requestsQuery = useQuery({
    queryKey: ENROLLMENT_REQUESTS_KEY,
    queryFn: async () =>
      unwrap<EnrollmentRequest[]>(await api.GET("/api/v2/enrollment-requests")),
    refetchInterval: LIST_POLL_MS,
  });
  const windowQuery = useQuery({
    queryKey: PROVISIONING_WINDOW_KEY,
    queryFn: async () =>
      unwrap<ProvisioningWindow>(await api.GET("/api/v2/provisioning-window")),
    refetchInterval: LIST_POLL_MS,
  });

  function refreshRequests() {
    void queryClient.invalidateQueries({ queryKey: ENROLLMENT_REQUESTS_KEY });
  }

  function refreshWindowAndRequests() {
    void Promise.all([
      queryClient.invalidateQueries({ queryKey: PROVISIONING_WINDOW_KEY }),
      queryClient.invalidateQueries({ queryKey: ENROLLMENT_REQUESTS_KEY }),
    ]);
  }

  function handleMutationError(error: unknown) {
    setNotice(noticeFromError(error));
    if (error instanceof ApiError) {
      switch (error.code) {
        case "ENROLLMENT_REQUEST_INVALID":
        case "ENROLLMENT_REQUEST_REJECTED":
          refreshRequests();
          break;
        case "PROVISIONING_WINDOW_CLOSED":
          refreshWindowAndRequests();
          break;
        default:
          break;
      }
    }
  }

  const approve = useMutation({
    mutationFn: async (requestId: string) =>
      unwrap<EnrollmentActionResponse>(
        await api.POST(
          "/api/v2/enrollment-requests/{request_id}/actions/approve",
          { params: { path: { request_id: requestId } } },
        ),
      ),
    onSuccess: (result) => {
      setNotice({
        tone: "success",
        title: "Enrollment approved",
        detail: `Request ${result.enrollment_request_id} can now be claimed by the device.`,
      });
      refreshRequests();
    },
    onError: handleMutationError,
  });

  const reject = useMutation({
    mutationFn: async (requestId: string) =>
      unwrap<EnrollmentActionResponse>(
        await api.POST(
          "/api/v2/enrollment-requests/{request_id}/actions/reject",
          { params: { path: { request_id: requestId } } },
        ),
      ),
    onSuccess: (result) => {
      setNotice({
        tone: "success",
        title: "Enrollment rejected",
        detail: `Request ${result.enrollment_request_id} is no longer actionable.`,
      });
      refreshRequests();
    },
    onError: handleMutationError,
  });

  const changeWindow = useMutation({
    mutationFn: async (target: "open" | "closed") => {
      if (target === "open") {
        return unwrap<ProvisioningWindow>(
          await api.POST("/api/v2/provisioning-window/actions/open"),
        );
      }
      return unwrap<ProvisioningWindow>(
        await api.POST("/api/v2/provisioning-window/actions/close"),
      );
    },
    onSuccess: (window) => {
      setNotice({
        tone: "success",
        title: `Provisioning window ${window.state}`,
        detail: `Window revision ${window.revision}.`,
      });
      refreshWindowAndRequests();
    },
    onError: handleMutationError,
  });

  const columns = useMemo<ColumnDef<EnrollmentRequest>[]>(() => {
    const base: ColumnDef<EnrollmentRequest>[] = [
      {
        accessorKey: "machine_hardware_id",
        header: "Hardware ID",
        cell: ({ row }) => (
          <span
            className="block max-w-48 truncate font-mono text-xs"
            title={row.original.machine_hardware_id}
          >
            {row.original.machine_hardware_id}
          </span>
        ),
      },
      {
        accessorKey: "hardware_identity_quality",
        header: "Quality",
        cell: ({ row }) => (
          <Badge variant={qualityBadge(row.original.hardware_identity_quality)}>
            {row.original.hardware_identity_quality}
          </Badge>
        ),
      },
      {
        accessorKey: "gateway_spki_sha256",
        header: "Gateway SPKI",
        cell: ({ row }) => (
          <span
            className="font-mono text-xs"
            title={row.original.gateway_spki_sha256}
          >
            {row.original.gateway_spki_sha256.slice(0, 12)}
          </span>
        ),
      },
      {
        id: "client_protocol",
        header: "Client / protocol",
        cell: ({ row }) => (
          <span>
            {row.original.client_version} / v{row.original.protocol_version}
          </span>
        ),
      },
      {
        accessorKey: "state",
        header: "State",
        cell: ({ row }) => (
          <Badge
            variant={
              row.original.state === "approved" ? "default" : "secondary"
            }
          >
            {row.original.state}
          </Badge>
        ),
      },
      {
        accessorKey: "created_at",
        header: "Created",
        cell: ({ row }) => new Date(row.original.created_at).toLocaleString(),
      },
    ];
    if (isAdmin) {
      base.push({
        id: "actions",
        header: "Actions",
        cell: ({ row }) =>
          row.original.state === "pending" ? (
            <EnrollmentActions
              request={row.original}
              disabled={approve.isPending || reject.isPending}
              onApprove={() =>
                approve.mutate(row.original.enrollment_request_id)
              }
              onReject={() => reject.mutate(row.original.enrollment_request_id)}
            />
          ) : null,
      });
    }
    return base;
  }, [approve, isAdmin, reject]);

  return (
    <div className="space-y-6">
      <div>
        <h1 className="text-2xl font-semibold tracking-tight">Enrollment</h1>
        <p className="mt-1 text-sm text-muted-foreground">
          Review credential replacement requests while provisioning is open.
        </p>
      </div>

      {notice && <NoticeAlert notice={notice} />}

      <Card>
        <CardHeader>
          <CardTitle>Provisioning window</CardTitle>
          <CardDescription>
            Device enrollment and credential claims require an open window.
          </CardDescription>
        </CardHeader>
        <CardContent>
          <DataState
            isLoading={windowQuery.isLoading}
            error={windowQuery.data ? null : windowQuery.error}
            isEmpty={false}
            emptyLabel="Provisioning window unavailable."
          >
            {windowQuery.data && (
              <div className="flex flex-wrap items-center gap-3">
                <Badge
                  variant={
                    windowQuery.data.state === "open" ? "default" : "outline"
                  }
                >
                  {windowQuery.data.state}
                </Badge>
                <span className="text-sm text-muted-foreground">
                  Revision {windowQuery.data.revision}
                </span>
                {isAdmin && (
                  <div className="ml-auto flex gap-2">
                    <Button
                      type="button"
                      size="sm"
                      variant="outline"
                      disabled={
                        windowQuery.data.state === "open" ||
                        changeWindow.isPending
                      }
                      onClick={() => {
                        setNotice(null);
                        changeWindow.mutate("open");
                      }}
                    >
                      Open window
                    </Button>
                    <AlertDialog>
                      <AlertDialogTrigger asChild>
                        <Button
                          type="button"
                          size="sm"
                          variant="destructive"
                          disabled={
                            windowQuery.data.state === "closed" ||
                            changeWindow.isPending
                          }
                        >
                          Close window
                        </Button>
                      </AlertDialogTrigger>
                      <AlertDialogContent>
                        <AlertDialogHeader>
                          <AlertDialogTitle>
                            Close the provisioning window?
                          </AlertDialogTitle>
                          <AlertDialogDescription>
                            Closing expires every unclaimed enrollment request.
                          </AlertDialogDescription>
                        </AlertDialogHeader>
                        <AlertDialogFooter>
                          <AlertDialogCancel>Cancel</AlertDialogCancel>
                          <AlertDialogAction
                            onClick={() => {
                              setNotice(null);
                              changeWindow.mutate("closed");
                            }}
                          >
                            Confirm close
                          </AlertDialogAction>
                        </AlertDialogFooter>
                      </AlertDialogContent>
                    </AlertDialog>
                  </div>
                )}
              </div>
            )}
          </DataState>
        </CardContent>
      </Card>

      <section className="space-y-3" aria-labelledby="live-enrollment-heading">
        <div>
          <h2 id="live-enrollment-heading" className="text-lg font-semibold">
            Live requests
          </h2>
          <p className="text-sm text-muted-foreground">
            {requestsQuery.data?.length ?? 0} pending or approved requests
          </p>
        </div>
        <DataState
          isLoading={requestsQuery.isLoading}
          error={requestsQuery.data ? null : requestsQuery.error}
          isEmpty={!requestsQuery.data?.length}
          emptyLabel="No live enrollment requests."
        >
          <DataTable columns={columns} data={requestsQuery.data ?? []} />
        </DataState>
      </section>
    </div>
  );
}

function EnrollmentActions({
  request,
  disabled,
  onApprove,
  onReject,
}: {
  request: EnrollmentRequest;
  disabled: boolean;
  onApprove: () => void;
  onReject: () => void;
}) {
  return (
    <div className="flex gap-2">
      <AlertDialog>
        <AlertDialogTrigger asChild>
          <Button type="button" size="sm" disabled={disabled}>
            Approve
          </Button>
        </AlertDialogTrigger>
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>Approve this enrollment?</AlertDialogTitle>
            <AlertDialogDescription>
              Approving lets this device claim credentials for that hardware id.
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel>Cancel</AlertDialogCancel>
            <AlertDialogAction onClick={onApprove}>
              Confirm approve
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
      <AlertDialog>
        <AlertDialogTrigger asChild>
          <Button
            type="button"
            size="sm"
            variant="destructive"
            disabled={disabled}
          >
            Reject
          </Button>
        </AlertDialogTrigger>
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>Reject this enrollment?</AlertDialogTitle>
            <AlertDialogDescription>
              Rejecting is terminal for this hardware id until the window
              closes.
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel>Cancel</AlertDialogCancel>
            <AlertDialogAction onClick={onReject}>
              Confirm reject
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
      <span className="sr-only">Request {request.enrollment_request_id}</span>
    </div>
  );
}

function qualityBadge(
  quality: EnrollmentRequest["hardware_identity_quality"],
): "default" | "secondary" | "outline" {
  switch (quality) {
    case "strong":
      return "default";
    case "medium":
      return "secondary";
    case "weak":
      return "outline";
  }
}

function noticeFromError(error: unknown): Notice {
  if (!(error instanceof ApiError)) {
    return {
      tone: "error",
      title: "Request failed",
      detail: "Please try again.",
    };
  }
  switch (error.code) {
    case "ENROLLMENT_REQUEST_INVALID":
      return {
        tone: "error",
        title: "Enrollment request is no longer actionable",
        detail: "The live request list has been refreshed.",
        correlationId: error.correlationId ?? undefined,
      };
    case "ENROLLMENT_REQUEST_REJECTED":
      return {
        tone: "error",
        title: "Enrollment request was rejected",
        detail: "The live request list has been refreshed.",
        correlationId: error.correlationId ?? undefined,
      };
    case "PROVISIONING_WINDOW_CLOSED":
      return {
        tone: "error",
        title: "Provisioning window is closed",
        detail: "Window and request facts have been refreshed.",
        correlationId: error.correlationId ?? undefined,
      };
    case "AUTHORIZATION_DENIED":
      return {
        tone: "error",
        title: "Administrator role required",
        correlationId: error.correlationId ?? undefined,
      };
    default:
      return {
        tone: "error",
        title: error.title,
        detail: "The request could not be completed.",
        correlationId: error.correlationId ?? undefined,
      };
  }
}

function NoticeAlert({ notice }: { notice: Notice }) {
  return (
    <Alert variant={notice.tone === "error" ? "destructive" : "default"}>
      <AlertTitle>{notice.title}</AlertTitle>
      <AlertDescription>
        {notice.detail && <p>{notice.detail}</p>}
        {notice.correlationId && <p>Correlation ID: {notice.correlationId}</p>}
      </AlertDescription>
    </Alert>
  );
}
