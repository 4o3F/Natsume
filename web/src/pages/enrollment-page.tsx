import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import type { ColumnDef } from "@tanstack/react-table";

import { api } from "@/api/client";
import { ApiError, unwrap } from "@/api/errors";
import type { components } from "@/api/generated/schema";
import { LIST_POLL_MS } from "@/api/polling";
import { useSession } from "@/auth/use-session";
import { DataTable } from "@/components/data-table";
import { DataState } from "@/components/data-state";
import { Alert, AlertTitle } from "@/components/ui/alert";
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

type EnrollmentReview = components["schemas"]["EnrollmentReviewResponse"];

const ENROLLMENT_REVIEWS_KEY = ["enrollment-reviews"] as const;

export function EnrollmentPage() {
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
