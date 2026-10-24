import { useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import {
  Check,
  DoorClosed,
  DoorOpen,
  Info,
  KeyRound,
  LoaderCircle,
  ShieldCheck,
  ShieldQuestion,
  TriangleAlert,
  X,
} from "lucide-react";

import { usePageToast } from "@/components/use-page-toast";

import { useSessionScope } from "@/auth/session-context";
import { ApiError, unwrap } from "@/api/errors";
import type {
  EnrollmentReviewResponse,
  ProvisioningWindowResponse,
} from "@/api/generated/types";
import { LIST_POLL_MS } from "@/api/polling";
import { useSession } from "@/auth/use-session";
import { ConfirmDialog } from "@/components/confirm-dialog";
import { DataTable, type DataTableColumnDef } from "@/components/data-table";
import { DataState } from "@/components/data-state";
import { PageHeader } from "@/components/page-header";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Card } from "@/components/ui/card";
import {
  Tooltip,
  TooltipContent,
  TooltipTrigger,
} from "@/components/ui/tooltip";
import { cn } from "@/lib/utils";

type EnrollmentReview = EnrollmentReviewResponse;
type ProvisioningWindow = ProvisioningWindowResponse;
type Decision = { kind: "approve" | "deny"; review: EnrollmentReview };

const ENROLLMENT_REVIEWS_KEY = ["enrollment-reviews"] as const;
const PROVISIONING_WINDOW_KEY = ["provisioning-window"] as const;

function shortId(value: string, size = 12) {
  return value.length > size ? `${value.slice(0, size)}…` : value;
}

export function EnrollmentPage() {
  const toast = usePageToast();
  const { api } = useSessionScope();
  const session = useSession().data;
  const queryClient = useQueryClient();
  const [decision, setDecision] = useState<Decision | null>(null);
  const reviews = useQuery({
    queryKey: ENROLLMENT_REVIEWS_KEY,
    queryFn: async () =>
      unwrap<EnrollmentReview[]>(await api.listEnrollmentReviews()),
    refetchInterval: LIST_POLL_MS,
  });
  const decide = useMutation({
    mutationFn: async ({ kind, review }: Decision) =>
      unwrap<void>(
        kind === "approve"
          ? await api.approveEnrollmentReview(review.review_id)
          : await api.denyEnrollmentReview(review.review_id),
      ),
    onSuccess: (_, { kind, review }) => {
      toast.success(
        kind === "approve" ? "Device approved" : "Enrollment denied",
        { description: shortId(review.machine_hardware_id, 16) },
      );
    },
    onSettled: async () => {
      setDecision(null);
      await queryClient.invalidateQueries({ queryKey: ENROLLMENT_REVIEWS_KEY });
    },
  });

  const isAdmin = session?.role === "admin";
  const columns: DataTableColumnDef<EnrollmentReview>[] = [
    {
      accessorKey: "machine_hardware_id",
      header: "Machine hardware ID",
      cell: ({ row }) => (
        <code
          className="font-mono text-xs"
          title={row.original.machine_hardware_id}
        >
          {shortId(row.original.machine_hardware_id, 16)}
        </code>
      ),
    },
    {
      accessorKey: "evidence_quality",
      header: "Evidence",
      cell: ({ row }) => <EvidenceBadge review={row.original} />,
    },
    {
      id: "versions",
      header: "Daemon / Agent",
      cell: ({ row }) => {
        const { daemon_version, agent_version } = row.original;
        return (
          <span className="flex items-center gap-1.5 font-mono text-xs">
            {daemon_version}
            <span className="text-muted-foreground">/</span>
            {agent_version}
            {daemon_version !== agent_version && (
              <Badge variant="warning">Mismatch</Badge>
            )}
          </span>
        );
      },
    },
    {
      accessorKey: "candidate_public_key",
      header: "Candidate key",
      cell: ({ row }) => (
        <Tooltip>
          <TooltipTrigger asChild>
            <span className="inline-flex items-center gap-1.5 rounded-md bg-muted px-1.5 py-0.5 font-mono text-xs">
              <KeyRound
                aria-hidden="true"
                className="size-3 text-muted-foreground"
              />
              {row.original.candidate_public_key.slice(0, 20)}…
            </span>
          </TooltipTrigger>
          <TooltipContent className="max-w-96 font-mono break-all">
            {row.original.candidate_public_key}
          </TooltipContent>
        </Tooltip>
      ),
    },
  ];

  if (isAdmin) {
    columns.push({
      id: "actions",
      header: () => <span className="sr-only">Actions</span>,
      cell: ({ row }) => (
        <div className="flex justify-end gap-2">
          <Button
            type="button"
            variant="outline"
            size="sm"
            disabled={decide.isPending}
            onClick={() => setDecision({ kind: "deny", review: row.original })}
          >
            <X aria-hidden="true" />
            Deny
          </Button>
          <Button
            type="button"
            size="sm"
            disabled={decide.isPending}
            onClick={() =>
              setDecision({ kind: "approve", review: row.original })
            }
          >
            <Check aria-hidden="true" />
            Approve
          </Button>
        </div>
      ),
    });
  }

  const pendingCount = reviews.data?.length ?? 0;

  return (
    <div className="min-w-0 space-y-6">
      <PageHeader
        title="Enrollment"
        description="Approve devices that are waiting to join. Reviews belong to enrollment attempts that are connected right now."
      />

      <EnrollmentWindow isAdmin={isAdmin} />

      {decide.isError && (
        <Alert variant="destructive">
          <TriangleAlert aria-hidden="true" />
          <AlertTitle>
            {decide.error instanceof ApiError
              ? decide.error.title
              : "Enrollment action failed"}
          </AlertTitle>
        </Alert>
      )}

      <section aria-label="Pending reviews" className="space-y-3">
        <div className="flex items-center gap-2">
          <p className="text-sm font-semibold">Pending reviews</p>
          {reviews.data && (
            <Badge variant={pendingCount ? "warning" : "muted"}>
              {pendingCount}
            </Badge>
          )}
          {!isAdmin && (
            <p className="ml-auto text-xs text-muted-foreground">
              Only administrators can approve or deny.
            </p>
          )}
        </div>
        <DataState
          isLoading={reviews.isLoading}
          error={reviews.data ? null : reviews.error}
          isEmpty={!pendingCount}
          emptyLabel="No devices waiting"
          emptyDescription="New enrollment attempts appear here while their device stays connected."
        >
          <DataTable
            label="Pending enrollment reviews"
            columns={columns}
            data={reviews.data ?? []}
            getRowId={(review) => review.review_id}
          />
        </DataState>
      </section>

      <ConfirmDialog
        open={decision !== null}
        onOpenChange={(open) => {
          if (!open && !decide.isPending) setDecision(null);
        }}
        tone={decision?.kind === "approve" ? "info" : "destructive"}
        icon={
          decision?.kind === "approve" ? (
            <ShieldCheck aria-hidden="true" />
          ) : undefined
        }
        title={
          decision?.kind === "approve"
            ? "Approve this device?"
            : "Deny this enrollment?"
        }
        description={
          decision?.kind === "approve" ? (
            <>
              This commits the candidate control key for{" "}
              <code className="font-mono text-xs break-all">
                {decision.review.machine_hardware_id}
              </code>
              .
            </>
          ) : (
            "The waiting enrollment connection will be rejected."
          )
        }
        confirmLabel={decision?.kind === "approve" ? "Approve" : "Deny"}
        disabled={decide.isPending}
        onConfirm={() => decision && decide.mutate(decision)}
      />
    </div>
  );
}

function EvidenceBadge({ review }: { review: EnrollmentReview }) {
  return review.evidence_quality === "strong" ? (
    <Badge variant="success">
      <ShieldCheck aria-hidden="true" />
      Strong
    </Badge>
  ) : (
    <Badge variant="warning">
      <ShieldQuestion aria-hidden="true" />
      Medium
    </Badge>
  );
}

function EnrollmentWindow({ isAdmin }: { isAdmin: boolean }) {
  const { api } = useSessionScope();
  const queryClient = useQueryClient();
  const updateWindow = useMutation({
    mutationFn: async (state: ProvisioningWindow["state"]) =>
      unwrap<ProvisioningWindow>(await api.updateProvisioningWindow({ state })),
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
      unwrap<ProvisioningWindow>(await api.getProvisioningWindow({ signal })),
    enabled: !updateWindow.isPending,
    refetchInterval: LIST_POLL_MS,
  });
  const isOpen = window.data?.state === "open";
  const display = window.isPending
    ? {
        label: "Checking…",
        mode: "Reading the enrollment policy from the server",
        Icon: LoaderCircle,
        accent: "bg-muted text-muted-foreground",
        rail: "bg-border",
      }
    : window.isError
      ? {
          label: "Unavailable",
          mode: "Enrollment policy could not be confirmed. Refresh before making changes.",
          Icon: TriangleAlert,
          accent: "bg-destructive-subtle text-destructive",
          rail: "bg-destructive",
        }
      : isOpen
        ? {
            label: "Open",
            mode: "New and pending requests are approved automatically.",
            Icon: DoorOpen,
            accent: "bg-success-subtle text-success-strong",
            rail: "bg-success",
          }
        : {
            label: "Closed",
            mode: "Requests wait here for an administrator.",
            Icon: DoorClosed,
            accent: "bg-warning-subtle text-warning-strong",
            rail: "bg-warning",
          };

  return (
    <Card
      role="region"
      aria-label="Enrollment window"
      className="relative gap-0 overflow-hidden py-0"
    >
      <span
        aria-hidden="true"
        className={cn("absolute inset-y-0 left-0 w-1", display.rail)}
      />
      <div className="flex flex-col gap-5 p-5 sm:flex-row sm:items-center sm:justify-between sm:p-6">
        <div className="flex items-center gap-4">
          <span
            className={cn(
              "relative flex size-12 shrink-0 items-center justify-center rounded-xl",
              display.accent,
            )}
          >
            {isOpen && (
              <span
                aria-hidden="true"
                className="absolute inset-0 rounded-xl bg-success/20 motion-safe:animate-pulse-ring"
              />
            )}
            <display.Icon
              aria-hidden="true"
              className={cn(
                "size-6",
                window.isPending && "animate-spin motion-reduce:animate-none",
              )}
            />
          </span>
          <div className="min-w-0">
            <p className="text-xs font-medium text-muted-foreground">
              Enrollment window
            </p>
            <p role="status" className="text-headline font-light">
              {display.label}
            </p>
            <p className="text-sm text-muted-foreground">{display.mode}</p>
          </div>
        </div>
        {isAdmin && (
          <Button
            type="button"
            className="shrink-0"
            variant={isOpen ? "outline" : "default"}
            disabled={!window.isSuccess}
            loading={updateWindow.isPending}
            onClick={() => updateWindow.mutate(isOpen ? "closed" : "open")}
          >
            {!updateWindow.isPending &&
              (isOpen ? (
                <DoorClosed aria-hidden="true" />
              ) : (
                <DoorOpen aria-hidden="true" />
              ))}
            {updateWindow.isPending
              ? "Updating…"
              : isOpen
                ? "Close window"
                : "Open window"}
          </Button>
        )}
      </div>
      <div className="space-y-3 border-t bg-muted/40 px-5 py-3 sm:px-6">
        <p className="flex items-start gap-2 text-xs text-muted-foreground">
          <Info aria-hidden="true" className="mt-0.5 size-3.5 shrink-0" />
          The window closes whenever the server restarts. Closing it does not
          revoke enrolled devices.
        </p>
        {window.isError && (
          <Alert variant="destructive">
            <TriangleAlert aria-hidden="true" />
            <AlertTitle>
              Unable to load enrollment window: {window.error.message}
            </AlertTitle>
            <AlertDescription>
              <Button
                type="button"
                variant="outline"
                size="sm"
                loading={window.isFetching}
                onClick={() => void window.refetch()}
              >
                Retry
              </Button>
            </AlertDescription>
          </Alert>
        )}
        {updateWindow.isError && (
          <Alert variant="destructive">
            <TriangleAlert aria-hidden="true" />
            <AlertTitle>
              Unable to update enrollment window: {updateWindow.error.message}
            </AlertTitle>
          </Alert>
        )}
      </div>
    </Card>
  );
}
