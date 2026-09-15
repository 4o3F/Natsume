import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import type { ColumnDef } from "@tanstack/react-table";
import {
  DoorClosed,
  DoorOpen,
  Info,
  LoaderCircle,
  TriangleAlert,
} from "lucide-react";

import { useSessionScope } from "@/auth/session-context";
import { ApiError, unwrap } from "@/api/errors";
import type { components } from "@/api/generated/schema";
import { LIST_POLL_MS } from "@/api/polling";
import { useSession } from "@/auth/use-session";
import { DataTable } from "@/components/data-table";
import { DataState } from "@/components/data-state";
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

type EnrollmentReview = components["schemas"]["EnrollmentReviewResponse"];
type ProvisioningWindow = components["schemas"]["ProvisioningWindowResponse"];

const ENROLLMENT_REVIEWS_KEY = ["enrollment-reviews"] as const;
const PROVISIONING_WINDOW_KEY = ["provisioning-window"] as const;

export function EnrollmentPage() {
  const { api } = useSessionScope();
  const session = useSession().data;
  const queryClient = useQueryClient();
  const reviews = useQuery({
    queryKey: ENROLLMENT_REVIEWS_KEY,
    queryFn: async () =>
      unwrap<EnrollmentReview[]>(await api.GET("/api/v2/enrollment-reviews")),
    refetchInterval: LIST_POLL_MS,
  });
  const approve = useMutation({
    mutationFn: async (reviewId: string) =>
      unwrap<void>(
        await api.POST(
          "/api/v2/enrollment-reviews/{review_id}/actions/approve",
          { params: { path: { review_id: reviewId } } },
        ),
      ),
    onSuccess: async () => {
      await queryClient.invalidateQueries({ queryKey: ENROLLMENT_REVIEWS_KEY });
    },
  });
  const deny = useMutation({
    mutationFn: async (reviewId: string) =>
      unwrap<void>(
        await api.POST("/api/v2/enrollment-reviews/{review_id}/actions/deny", {
          params: { path: { review_id: reviewId } },
        }),
      ),
    onSuccess: async () => {
      await queryClient.invalidateQueries({ queryKey: ENROLLMENT_REVIEWS_KEY });
    },
  });

  const isAdmin = session?.role === "admin";
  const isMutating = approve.isPending || deny.isPending;
  const columns: ColumnDef<EnrollmentReview>[] = [
    { accessorKey: "machine_hardware_id", header: "Machine hardware ID" },
    {
      accessorKey: "evidence_quality",
      header: "Evidence",
      cell: ({ row }) => (
        <Badge variant="outline">{row.original.evidence_quality}</Badge>
      ),
    },
    { accessorKey: "daemon_version", header: "Daemon" },
    { accessorKey: "agent_version", header: "Agent" },
    {
      accessorKey: "candidate_public_key",
      header: "Candidate key",
      cell: ({ row }) => (
        <span
          className="font-mono text-xs"
          title={row.original.candidate_public_key}
        >
          {row.original.candidate_public_key.slice(0, 16)}…
        </span>
      ),
    },
  ];

  if (isAdmin) {
    columns.push({
      id: "actions",
      header: "Actions",
      cell: ({ row }) => (
        <div className="flex gap-2">
          <ReviewAction
            label="Approve"
            title="Approve this device?"
            description={`This commits the candidate control key for ${row.original.machine_hardware_id}.`}
            disabled={isMutating}
            onConfirm={() => approve.mutate(row.original.review_id)}
          />
          <ReviewAction
            label="Deny"
            title="Deny this review?"
            description="The waiting enrollment connection will be rejected."
            disabled={isMutating}
            destructive
            onConfirm={() => deny.mutate(row.original.review_id)}
          />
        </div>
      ),
    });
  }

  const mutationError = approve.error ?? deny.error;

  return (
    <div className="space-y-6">
      <div>
        <h1 className="text-2xl font-semibold tracking-tight">
          Enrollment Reviews
        </h1>
        <p className="mt-1 text-sm text-muted-foreground">
          Pending reviews belong to currently connected enrollment attempts.
        </p>
      </div>

      <EnrollmentWindow isAdmin={isAdmin} />

      {mutationError && (
        <Alert variant="destructive">
          <AlertTitle>
            {mutationError instanceof ApiError
              ? mutationError.title
              : "Enrollment action failed"}
          </AlertTitle>
        </Alert>
      )}

      <DataState
        isLoading={reviews.isLoading}
        error={reviews.data ? null : reviews.error}
        isEmpty={!reviews.data?.length}
        emptyLabel="No enrollment reviews are pending."
      >
        <DataTable columns={columns} data={reviews.data ?? []} />
      </DataState>
    </div>
  );
}

function EnrollmentWindow({ isAdmin }: { isAdmin: boolean }) {
  const { api } = useSessionScope();
  const queryClient = useQueryClient();
  const updateWindow = useMutation({
    mutationFn: async (state: ProvisioningWindow["state"]) =>
      unwrap<ProvisioningWindow>(
        await api.PUT("/api/v2/provisioning-window", { body: { state } }),
      ),
    onMutate: () =>
      queryClient.cancelQueries({ queryKey: PROVISIONING_WINDOW_KEY }),
    onSuccess: (data) => {
      queryClient.setQueryData(PROVISIONING_WINDOW_KEY, data);
    },
    onSettled: () =>
      queryClient.invalidateQueries({ queryKey: PROVISIONING_WINDOW_KEY }),
  });
  const window = useQuery({
    queryKey: PROVISIONING_WINDOW_KEY,
    queryFn: async ({ signal }) =>
      unwrap<ProvisioningWindow>(
        await api.GET("/api/v2/provisioning-window", { signal }),
      ),
    enabled: !updateWindow.isPending,
    refetchInterval: LIST_POLL_MS,
  });
  const isOpen = window.data?.state === "open";
  const display = window.isPending
    ? {
        label: "Loading...",
        mode: "Checking enrollment policy",
        description:
          "Reading the current enrollment window state from the Server.",
        Icon: LoaderCircle,
        border: "border-border",
        surface: "bg-muted/30",
        accent: "bg-muted text-muted-foreground",
        text: "text-muted-foreground",
      }
    : window.isError
      ? {
          label: "Unavailable",
          mode: "Enrollment policy could not be confirmed",
          description: "Refresh the window state before making changes.",
          Icon: TriangleAlert,
          border: "border-destructive/30",
          surface: "bg-destructive/5",
          accent: "bg-destructive/10 text-destructive",
          text: "text-destructive",
        }
      : isOpen
        ? {
            label: "Open",
            mode: "Automatic approval is enabled",
            description:
              "Open automatically approves new and pending enrollment requests.",
            Icon: DoorOpen,
            border: "border-emerald-600/30",
            surface: "bg-emerald-500/5",
            accent: "bg-emerald-500/15 text-emerald-700",
            text: "text-emerald-700",
          }
        : {
            label: "Closed",
            mode: "Administrator approval is required",
            description:
              "Closed keeps requests here for administrator approval.",
            Icon: DoorClosed,
            border: "border-amber-600/30",
            surface: "bg-amber-500/5",
            accent: "bg-amber-500/15 text-amber-700",
            text: "text-amber-700",
          };

  return (
    <Card
      role="region"
      aria-label="Enrollment window"
      className={`gap-0 overflow-hidden py-0 ${display.border}`}
    >
      <CardHeader className={`gap-4 p-5 sm:p-6 ${display.surface}`}>
        <CardTitle className="text-sm text-muted-foreground">
          Enrollment window
        </CardTitle>
        <div className="flex flex-col gap-5 sm:flex-row sm:items-center sm:justify-between">
          <div className="flex items-start gap-4">
            <div
              className={`flex size-14 shrink-0 items-center justify-center rounded-xl ${display.accent}`}
            >
              <display.Icon
                aria-hidden="true"
                className={`size-7 ${window.isPending ? "animate-spin motion-reduce:animate-none" : ""}`}
              />
            </div>
            <div className="min-w-0 space-y-1">
              <p
                role="status"
                className={`text-3xl font-semibold tracking-tight ${display.text}`}
              >
                {display.label}
              </p>
              <p className="text-sm font-medium">{display.mode}</p>
              <CardDescription>{display.description}</CardDescription>
            </div>
          </div>
          {isAdmin && (
            <Button
              type="button"
              className="shrink-0"
              variant={isOpen ? "outline" : "default"}
              disabled={!window.isSuccess || updateWindow.isPending}
              onClick={() => updateWindow.mutate(isOpen ? "closed" : "open")}
            >
              {updateWindow.isPending ? (
                <LoaderCircle
                  aria-hidden="true"
                  className="animate-spin motion-reduce:animate-none"
                />
              ) : isOpen ? (
                <DoorClosed aria-hidden="true" />
              ) : (
                <DoorOpen aria-hidden="true" />
              )}
              {updateWindow.isPending
                ? "Updating..."
                : isOpen
                  ? "Close window"
                  : "Open window"}
            </Button>
          )}
        </div>
      </CardHeader>
      <CardContent className="space-y-3 border-t px-5 py-4 sm:px-6">
        <p className="flex items-start gap-2 text-xs text-muted-foreground">
          <Info aria-hidden="true" className="mt-0.5 size-3.5 shrink-0" />
          The window closes whenever the server restarts. Closing it does not
          revoke enrolled devices.
        </p>
        {window.isError && (
          <Alert variant="destructive">
            <AlertTitle>
              Unable to load enrollment window: {window.error.message}
            </AlertTitle>
            <AlertDescription>
              <Button
                type="button"
                variant="outline"
                size="sm"
                disabled={window.isFetching}
                onClick={() => void window.refetch()}
              >
                Retry
              </Button>
            </AlertDescription>
          </Alert>
        )}
        {updateWindow.isError && (
          <Alert variant="destructive">
            <AlertTitle>
              Unable to update enrollment window: {updateWindow.error.message}
            </AlertTitle>
          </Alert>
        )}
      </CardContent>
    </Card>
  );
}

function ReviewAction({
  label,
  title,
  description,
  disabled,
  destructive = false,
  onConfirm,
}: {
  label: string;
  title: string;
  description: string;
  disabled: boolean;
  destructive?: boolean;
  onConfirm: () => void;
}) {
  return (
    <AlertDialog>
      <AlertDialogTrigger asChild>
        <Button
          type="button"
          variant={destructive ? "outline" : "default"}
          size="sm"
          disabled={disabled}
        >
          {label}
        </Button>
      </AlertDialogTrigger>
      <AlertDialogContent>
        <AlertDialogHeader>
          <AlertDialogTitle>{title}</AlertDialogTitle>
          <AlertDialogDescription>{description}</AlertDialogDescription>
        </AlertDialogHeader>
        <AlertDialogFooter>
          <AlertDialogCancel>Cancel</AlertDialogCancel>
          <AlertDialogAction
            className={
              destructive
                ? "bg-destructive text-white hover:bg-destructive/90"
                : undefined
            }
            onClick={onConfirm}
          >
            {label}
          </AlertDialogAction>
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  );
}
