import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import type { ColumnDef } from "@tanstack/react-table";

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

  return (
    <Card role="region" aria-label="Enrollment window">
      <CardHeader>
        <CardTitle>Enrollment window</CardTitle>
        <CardDescription>
          Open the window to allow device enrollment. It closes whenever the
          server restarts. Closing it does not revoke enrolled devices.
        </CardDescription>
      </CardHeader>
      <CardContent className="space-y-3">
        <div className="flex items-center gap-3">
          <Badge
            role="status"
            variant={window.isSuccess && isOpen ? "default" : "outline"}
          >
            {window.isPending
              ? "Loading..."
              : window.isError
                ? "Unavailable"
                : isOpen
                  ? "Open"
                  : "Closed"}
          </Badge>
          {isAdmin && (
            <Button
              type="button"
              variant={isOpen ? "outline" : "default"}
              disabled={!window.isSuccess || updateWindow.isPending}
              onClick={() => updateWindow.mutate(isOpen ? "closed" : "open")}
            >
              {updateWindow.isPending
                ? "Updating..."
                : isOpen
                  ? "Close window"
                  : "Open window"}
            </Button>
          )}
        </div>
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
