import { type FormEvent, useEffect, useMemo, useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import type { ColumnDef } from "@tanstack/react-table";

import { useSessionScope } from "@/auth/session-context";
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
import { PreparationLogos } from "@/pages/preparation-logos";
import { RosterDiff } from "@/pages/preparation-diff";
import type { PreparationPreview } from "@/pages/preparation-store";

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

const XLSX_CONTENT_TYPE =
  "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet";
const MAX_WORKBOOK_BYTES = 8 * 1024 * 1024;

const PENDING_IMPORT_KEY = ["imports", "pending"] as const;

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
  {
    id: "impact",
    header: "Impact",
    cell: ({ row }) =>
      row.original.blocks_commit
        ? "Unbind before removing seat"
        : "Bound team data will change",
  },
];

export function PreparationPage() {
  const { api, preparation } = useSessionScope();
  const queryClient = useQueryClient();
  const [selectedFile, setSelectedFile] = useState<File | null>(null);
  const [localPreview, setLocalPreview] = useState<PreparationPreview | null>(
    () => preparation.get(),
  );
  const [notice, setNotice] = useState<Notice | null>(null);

  const pendingQuery = useQuery({
    queryKey: PENDING_IMPORT_KEY,
    queryFn: async () =>
      unwrap<ImportPendingResponse>(await api.GET("/api/v2/imports")),
    refetchInterval: LIST_POLL_MS,
  });

  function forgetPreview() {
    preparation.clear();
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
      const preview = await unwrap<ImportPreviewResponse>(
        await api.POST("/api/v2/imports", {
          // OpenAPI models binary as a string; the serializer sends the File bytes.
          body: "",
          bodySerializer: () => file,
          headers: { "Content-Type": XLSX_CONTENT_TYPE },
        }),
      );
      preparation.set({
        candidate_id: preview.candidate_id,
        preview_token: preview.preview_token,
        file,
      });
      return { candidate_id: preview.candidate_id };
    },
    onSuccess: async ({ candidate_id }) => {
      setSelectedFile(null);
      const preview = preparation.get();
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
      const preview = preparation.get();
      if (!preview || preview.candidate_id !== candidateId) {
        throw new Error("the preview token is unavailable");
      }
      return unwrap<void>(
        await api.POST("/api/v2/imports/{import_id}/actions/commit", {
          params: {
            path: { import_id: candidateId },
            header: { "x-natsume-preview-token": preview.preview_token },
          },
          body: "",
          bodySerializer: () => preview.file,
          headers: { "Content-Type": XLSX_CONTENT_TYPE },
        }),
      );
    },
    onSuccess: async () => {
      forgetPreview();
      setSelectedFile(null);
      setNotice({
        tone: "success",
        title: "Import committed",
        detail:
          "The complete roster has been applied. Only changed passwords advance credential revisions.",
      });
      await Promise.all([
        queryClient.invalidateQueries({ queryKey: PENDING_IMPORT_KEY }),
        queryClient.invalidateQueries({ queryKey: ["seats"] }),
        queryClient.invalidateQueries({ queryKey: ["accounts"] }),
        queryClient.invalidateQueries({ queryKey: ["bindings"] }),
        queryClient.invalidateQueries({ queryKey: ["organization-logos"] }),
      ]);
    },
    onError: handleMutationError,
  });

  const discard = useMutation({
    mutationFn: async (candidateId: string) =>
      unwrap<void>(
        await api.DELETE("/api/v2/imports/{import_id}", {
          params: { path: { import_id: candidateId } },
        }),
      ),
    onSuccess: async () => {
      forgetPreview();
      setSelectedFile(null);
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
    pending !== null && localPreview?.candidate_id === pending.candidate_id;

  function submitUpload(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    if (!selectedFile) {
      return;
    }
    if (selectedFile.size > MAX_WORKBOOK_BYTES) {
      setNotice({
        tone: "error",
        title: "Workbook too large",
        detail: "Choose an XLSX file no larger than 8 MiB.",
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
            <CardTitle>Upload complete roster</CardTitle>
            <CardDescription>
              Use the Teams sheet in the template. Include every team, school,
              seat and account, even when only passwords change. Maximum 8 MiB.
            </CardDescription>
          </CardHeader>
          <form onSubmit={submitUpload}>
            <CardContent className="space-y-3">
              <Label htmlFor="contest-xlsx">XLSX file</Label>
              <Input
                id="contest-xlsx"
                type="file"
                accept=".xlsx"
                disabled={upload.isPending}
                onChange={(event) =>
                  setSelectedFile(event.target.files?.[0] ?? null)
                }
              />
              <Button asChild variant="outline">
                <a href="/api/v2/imports/template" download>
                  Download Excel template
                </a>
              </Button>
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
      <PreparationLogos />
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
  const blocked = pending.diff.binding_impacts.some(
    (impact) => impact.blocks_commit,
  );
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
            Unchanged teams {pending.diff.unchanged_count}
          </Badge>
          <Badge variant="secondary">
            Affected accounts {pending.diff.affected_account_count}
          </Badge>
        </div>

        <RosterDiff diff={pending.diff} />
        <PreparationLogos
          key={pending.candidate_id}
          candidateId={pending.candidate_id}
        />

        <section className="space-y-2" aria-labelledby="seat-changes-heading">
          <h2 id="seat-changes-heading" className="font-medium">
            Seat changes
          </h2>
          <DataTable
            scrollable
            columns={seatChangeColumns}
            data={seatChanges}
          />
        </section>

        <section
          className="space-y-2"
          aria-labelledby="mapping-changes-heading"
        >
          <h2 id="mapping-changes-heading" className="font-medium">
            Mapping changes
          </h2>
          <DataTable
            scrollable
            columns={mappingColumns}
            data={pending.diff.mappings_changed}
          />
        </section>

        {pending.diff.binding_impacts.length > 0 ? (
          <Alert
            variant={blocked ? "destructive" : "default"}
            className="grid-cols-1"
          >
            <AlertTitle className="col-start-1">Binding impacts</AlertTitle>
            <AlertDescription className="col-start-1 w-full">
              <p>
                {blocked
                  ? "Removing an occupied seat requires releasing its binding first."
                  : "These devices will receive updated team data. Their seat bindings and session targets are preserved."}
              </p>
              <div className="w-full text-foreground">
                <DataTable
                  scrollable
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
            Preview authorization and the reviewed XLSX are unavailable after a
            reload or session change; discard and re-upload to commit.
          </p>
        )}
      </CardContent>
      <CardFooter className="sticky bottom-0 gap-3 border-t bg-card py-4">
        <AlertDialog>
          <AlertDialogTrigger asChild>
            <Button
              type="button"
              disabled={
                blocked ||
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
                Applies the complete roster, including removals. Only changed
                passwords advance credential revisions. Bindings remain on their
                seats and session targets stay unchanged.
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
      detail: "The request exceeded the server's size limit.",
    };
  }

  switch (error.code) {
    case "IMPORT_CANDIDATE_INVALID":
      return {
        tone: "error",
        title: error.title,
        detail: "The XLSX did not satisfy the import contract.",
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
        detail:
          "Discard this preview and re-upload the XLSX before committing.",
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
