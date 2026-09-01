import { type FormEvent, useEffect, useMemo, useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import type { ColumnDef } from "@tanstack/react-table";

import { api } from "@/api/client";
import { ApiError, unwrap } from "@/api/errors";
import type { components } from "@/api/generated/schema";
import { LIST_POLL_MS } from "@/api/polling";
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
  CardFooter,
  CardHeader,
  CardTitle,
} from "@/components/ui/card";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import {
  clearPreparationPreview,
  getPreparationPreview,
  type PreparationPreview,
  setPreparationPreview,
} from "@/pages/preparation-store";

type ImportMappingChange = components["schemas"]["ImportMappingChangeResponse"];
type ImportPendingResponse = components["schemas"]["ImportPendingResponse"];
type ImportPendingSummary = components["schemas"]["ImportPendingSummary"];
type ImportPreviewResponse = components["schemas"]["ImportPreviewResponse"];
type ImportBindingImpact = components["schemas"]["ImportBindingImpactResponse"];

type SeatChange = {
  seat_code: string;
  change: "Added" | "Removed";
};

interface Notice {
  tone: "success" | "error";
  title: string;
  detail?: string;
}

const PENDING_IMPORT_KEY = ["imports", "pending"] as const;
const CSV_IMPORT_BODY_LIMIT_BYTES = 4_194_304;

const seatChangeColumns: ColumnDef<SeatChange>[] = [
  { accessorKey: "seat_code", header: "Seat code" },
  { accessorKey: "change", header: "Change" },
];

const mappingColumns: ColumnDef<ImportMappingChange>[] = [
  { accessorKey: "seat_code", header: "Seat code" },
  {
    accessorKey: "current_domjudge_username",
    header: "Current username",
    cell: ({ row }) => row.original.current_domjudge_username ?? "Unmapped",
  },
  {
    accessorKey: "candidate_domjudge_username",
    header: "Candidate username",
  },
];

const bindingColumns: ColumnDef<ImportBindingImpact>[] = [
  { accessorKey: "seat_code", header: "Seat code" },
  { accessorKey: "device_id", header: "Device ID" },
];

export function PreparationPage() {
  const queryClient = useQueryClient();
  const [selectedFile, setSelectedFile] = useState<File | null>(null);
  const [localPreview, setLocalPreview] = useState<PreparationPreview | null>(
    () => getPreparationPreview(),
  );
  const [notice, setNotice] = useState<Notice | null>(null);

  const pendingQuery = useQuery({
    queryKey: PENDING_IMPORT_KEY,
    queryFn: async () =>
      unwrap<ImportPendingResponse>(await api.GET("/api/v2/imports")),
    refetchInterval: LIST_POLL_MS,
  });

  function forgetPreview() {
    clearPreparationPreview();
    setLocalPreview(null);
  }

  function handleMutationError(error: unknown) {
    setNotice(noticeFromError(error));
    if (!(error instanceof ApiError)) {
      return;
    }
    switch (error.code) {
      case "IMPORT_CANDIDATE_PENDING":
        void pendingQuery.refetch();
        break;
      case "IMPORT_CANDIDATE_UNAVAILABLE":
        forgetPreview();
        void pendingQuery.refetch();
        break;
      case "IMPORT_CANDIDATE_INVALID":
      case "IMPORT_PREVIEW_STALE":
      default:
        break;
    }
  }

  const upload = useMutation({
    mutationFn: async (file: File) => {
      const csv = await file.text();
      const preview = await unwrap<ImportPreviewResponse>(
        await api.POST("/api/v2/imports", {
          body: csv,
          bodySerializer: (body) => body,
          headers: { "Content-Type": "text/csv" },
        }),
      );
      setPreparationPreview({
        candidate_id: preview.candidate_id,
        preview_token: preview.preview_token,
      });
      return { candidate_id: preview.candidate_id };
    },
    onSuccess: async ({ candidate_id }) => {
      const preview = getPreparationPreview();
      if (preview?.candidate_id === candidate_id) {
        setLocalPreview(preview);
      }
      setNotice({
        tone: "success",
        title: "Preview created",
        detail: "Review the redacted diff before committing.",
      });
      await queryClient.invalidateQueries({ queryKey: PENDING_IMPORT_KEY });
    },
    onError: handleMutationError,
  });

  const commit = useMutation({
    mutationFn: async (candidateId: string): Promise<void> => {
      const preview = getPreparationPreview();
      if (!preview || preview.candidate_id !== candidateId) {
        throw new Error("the preview token is unavailable");
      }
      if (!selectedFile) {
        throw new Error("the reviewed CSV is unavailable");
      }
      const csv = await selectedFile.text();
      return unwrap<void>(
        await api.POST("/api/v2/imports/{import_id}/actions/commit", {
          params: { path: { import_id: candidateId } },
          body: { csv, preview_token: preview.preview_token },
        }),
      );
    },
    onSuccess: async () => {
      forgetPreview();
      setSelectedFile(null);
      setNotice({
        tone: "success",
        title: "Import committed",
        detail: "The confirmed contest configuration has been replaced.",
      });
      await Promise.all([
        queryClient.invalidateQueries({ queryKey: PENDING_IMPORT_KEY }),
        queryClient.invalidateQueries({ queryKey: ["seats"] }),
        queryClient.invalidateQueries({ queryKey: ["accounts"] }),
        queryClient.invalidateQueries({ queryKey: ["bindings"] }),
      ]);
    },
    onError: handleMutationError,
  });

  const discard = useMutation({
    mutationFn: async (candidateId: string) =>
      unwrap<void>(
        await api.POST("/api/v2/imports/{import_id}/actions/discard", {
          params: { path: { import_id: candidateId } },
        }),
      ),
    onSuccess: async () => {
      forgetPreview();
      setNotice({
        tone: "success",
        title: "Preview discarded",
        detail: "The confirmed configuration was not changed.",
      });
      await queryClient.invalidateQueries({ queryKey: PENDING_IMPORT_KEY });
    },
    onError: handleMutationError,
  });

  const pending = pendingQuery.data?.pending ?? null;
  const commitTokenAvailable =
    pending !== null &&
    localPreview?.candidate_id === pending.candidate_id &&
    selectedFile !== null;

  function submitUpload(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    if (!selectedFile) {
      return;
    }
    if (selectedFile.size > CSV_IMPORT_BODY_LIMIT_BYTES) {
      setNotice({
        tone: "error",
        title: "CSV file is too large",
        detail: "CSV imports are limited to 4 MiB.",
      });
      return;
    }
    setNotice(null);
    upload.mutate(selectedFile);
  }

  return (
    <div className="space-y-6">
      <div>
        <h1 className="text-2xl font-semibold tracking-tight">
          Preparation Center
        </h1>
        <p className="mt-1 text-sm text-muted-foreground">
          Stage and review a complete contest configuration before committing
          it.
        </p>
      </div>

      {notice && <NoticeAlert notice={notice} />}
      {pendingQuery.error && (
        <NoticeAlert notice={noticeFromError(pendingQuery.error)} />
      )}

      {pendingQuery.isLoading && (
        <p className="text-sm text-muted-foreground">
          Loading pending import...
        </p>
      )}

      {!pendingQuery.isLoading && !pending && (
        <Card>
          <CardHeader>
            <CardTitle>Upload contest CSV</CardTitle>
            <CardDescription>
              Select a CSV with the exact header seat,account,password.
            </CardDescription>
          </CardHeader>
          <form onSubmit={submitUpload}>
            <CardContent className="space-y-3">
              <Label htmlFor="contest-csv">CSV file</Label>
              <Input
                id="contest-csv"
                type="file"
                accept=".csv,text/csv"
                onChange={(event) =>
                  setSelectedFile(event.target.files?.[0] ?? null)
                }
              />
            </CardContent>
            <CardFooter className="mt-6">
              <Button
                type="submit"
                disabled={!selectedFile || upload.isPending}
              >
                {upload.isPending ? "Uploading..." : "Create preview"}
              </Button>
            </CardFooter>
          </form>
        </Card>
      )}

      {pending && (
        <PendingImportCard
          pending={pending}
          commitTokenAvailable={commitTokenAvailable}
          commitPending={commit.isPending}
          discardPending={discard.isPending}
          onCommit={() => {
            setNotice(null);
            commit.mutate(pending.candidate_id);
          }}
          onDiscard={() => {
            setNotice(null);
            discard.mutate(pending.candidate_id);
          }}
        />
      )}
    </div>
  );
}

function PendingImportCard({
  pending,
  commitTokenAvailable,
  commitPending,
  discardPending,
  onCommit,
  onDiscard,
}: {
  pending: ImportPendingSummary;
  commitTokenAvailable: boolean;
  commitPending: boolean;
  discardPending: boolean;
  onCommit: () => void;
  onDiscard: () => void;
}) {
  const remainingSeconds = useRemainingSeconds(pending.expires_at_unix_ms);
  const seatChanges = useMemo(
    () =>
      [
        ...pending.diff.seats_added.map((seat_code) => ({
          seat_code,
          change: "Added" as const,
        })),
        ...pending.diff.seats_removed.map((seat_code) => ({
          seat_code,
          change: "Removed" as const,
        })),
      ].sort((left, right) =>
        left.seat_code < right.seat_code
          ? -1
          : left.seat_code > right.seat_code
            ? 1
            : 0,
      ),
    [pending.diff.seats_added, pending.diff.seats_removed],
  );

  return (
    <Card>
      <CardHeader>
        <CardTitle>Pending import</CardTitle>
        <CardDescription className="space-y-1">
          <span className="block font-mono">{pending.candidate_id}</span>
          <span className="block">
            Expires {new Date(pending.expires_at_unix_ms).toLocaleString()} (
            {formatRemainingTime(remainingSeconds)} remaining)
          </span>
        </CardDescription>
      </CardHeader>
      <CardContent className="space-y-6">
        <div className="flex flex-wrap gap-2">
          <Badge variant="secondary">
            Unchanged seats {pending.diff.unchanged_count}
          </Badge>
          <Badge variant="secondary">
            Affected accounts {pending.diff.affected_account_count}
          </Badge>
        </div>

        <section className="space-y-2" aria-labelledby="seat-changes-heading">
          <h2 id="seat-changes-heading" className="font-medium">
            Seat changes
          </h2>
          <DataTable columns={seatChangeColumns} data={seatChanges} />
        </section>

        <section
          className="space-y-2"
          aria-labelledby="mapping-changes-heading"
        >
          <h2 id="mapping-changes-heading" className="font-medium">
            Mapping changes
          </h2>
          <DataTable
            columns={mappingColumns}
            data={pending.diff.mappings_changed}
          />
        </section>

        {pending.diff.binding_impacts.length > 0 ? (
          <Alert variant="destructive" className="grid-cols-1">
            <AlertTitle className="col-start-1">Binding impacts</AlertTitle>
            <AlertDescription className="col-start-1 w-full">
              <p>
                These occupied seats block the import until their bindings are
                released.
              </p>
              <div className="w-full text-foreground">
                <DataTable
                  columns={bindingColumns}
                  data={pending.diff.binding_impacts}
                />
              </div>
            </AlertDescription>
          </Alert>
        ) : (
          <p className="text-sm text-muted-foreground">No binding impacts.</p>
        )}

        {!commitTokenAvailable && (
          <p className="text-sm text-muted-foreground">
            Preview authorization and the reviewed CSV are unavailable after a
            reload; discard and re-upload to commit.
          </p>
        )}
      </CardContent>
      <CardFooter className="gap-3">
        <AlertDialog>
          <AlertDialogTrigger asChild>
            <Button
              type="button"
              disabled={
                !commitTokenAvailable ||
                remainingSeconds <= 0 ||
                commitPending ||
                discardPending
              }
            >
              Commit import
            </Button>
          </AlertDialogTrigger>
          <AlertDialogContent>
            <AlertDialogHeader>
              <AlertDialogTitle>Commit this import?</AlertDialogTitle>
              <AlertDialogDescription>
                Replaces the entire confirmed configuration and advances every
                account credential revision.
              </AlertDialogDescription>
            </AlertDialogHeader>
            <AlertDialogFooter>
              <AlertDialogCancel>Cancel</AlertDialogCancel>
              <AlertDialogAction onClick={onCommit}>
                Confirm commit
              </AlertDialogAction>
            </AlertDialogFooter>
          </AlertDialogContent>
        </AlertDialog>
        <Button
          type="button"
          variant="destructive"
          disabled={commitPending || discardPending}
          onClick={onDiscard}
        >
          {discardPending ? "Discarding..." : "Discard preview"}
        </Button>
      </CardFooter>
    </Card>
  );
}

function useRemainingSeconds(expiresAtUnixMs: number): number {
  const [now, setNow] = useState(() => Date.now());

  useEffect(() => {
    const interval = window.setInterval(() => setNow(Date.now()), 1_000);
    return () => window.clearInterval(interval);
  }, []);

  return Math.max(0, Math.floor((expiresAtUnixMs - now) / 1_000));
}

function formatRemainingTime(totalSeconds: number): string {
  const minutes = Math.floor(totalSeconds / 60);
  const seconds = totalSeconds % 60;
  return `${String(minutes).padStart(2, "0")}:${String(seconds).padStart(2, "0")}`;
}

function noticeFromError(error: unknown): Notice {
  if (!(error instanceof ApiError)) {
    return {
      tone: "error",
      title: "Request failed",
      detail: "Please try again.",
    };
  }
  if (error.status === 413) {
    return {
      tone: "error",
      title: "Request too large",
      detail: "The request exceeded the route's size limit.",
    };
  }

  switch (error.code) {
    case "IMPORT_CANDIDATE_INVALID":
      return {
        tone: "error",
        title: error.title,
        detail: "The CSV did not satisfy the import contract.",
      };
    case "IMPORT_CANDIDATE_PENDING":
      return {
        tone: "error",
        title: "A preview is already pending",
        detail: "The pending candidate has been refreshed.",
      };
    case "IMPORT_PREVIEW_STALE":
      return {
        tone: "error",
        title: "Import preview is stale",
        detail: "Discard this preview and re-upload the CSV before committing.",
      };
    case "IMPORT_CANDIDATE_UNAVAILABLE":
      return {
        tone: "error",
        title: "Import candidate unavailable",
        detail: "The pending candidate has been refreshed.",
      };
    default:
      return {
        tone: "error",
        title: error.title,
        detail: "The request could not be completed.",
      };
  }
}

function NoticeAlert({ notice }: { notice: Notice }) {
  return (
    <Alert variant={notice.tone === "error" ? "destructive" : "default"}>
      <AlertTitle>{notice.title}</AlertTitle>
      <AlertDescription>
        {notice.detail && <p>{notice.detail}</p>}
      </AlertDescription>
    </Alert>
  );
}
