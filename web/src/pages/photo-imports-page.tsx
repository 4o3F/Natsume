import { useMemo, useState } from "react";
import { useQuery } from "@tanstack/react-query";
import {
  CheckCheck,
  FileArchive,
  LoaderCircle,
  Search,
  Upload,
  X,
} from "lucide-react";
import { Link, useNavigate, useParams } from "react-router";

import { unwrap } from "@/api/errors";
import type {
  PhotoDecision_Serialize as PhotoDecision,
  PhotoPreviewEntry_Serialize as PhotoPreviewEntry,
  ResultImportJob_Serialize as ResultImportJob,
  ResultImportPreview_Serialize as ResultImportPreview,
} from "@/api/generated/types";
import { LIST_POLL_MS } from "@/api/polling";
import { useSessionScope } from "@/auth/session-context";
import { DataState } from "@/components/data-state";
import { DataTable, type DataTableColumnDef } from "@/components/data-table";
import { tones } from "@/components/device-status-style";
import { PageHeader, SectionHeader } from "@/components/page-header";
import { PhotoTeam } from "@/components/photo-team";
import { PhotoThumb } from "@/components/photo-thumb";
import { StatTile } from "@/components/stat-tile";
import { Alert, AlertTitle } from "@/components/ui/alert";
import {
  AlertDialog,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogMedia,
  AlertDialogTitle,
} from "@/components/ui/alert-dialog";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { usePageToast } from "@/components/use-page-toast";
import { cn } from "@/lib/utils";
import { useOperatorAction } from "@/operator/use-operator-action";
import {
  activateDecision,
  importFailures,
  importInFlight,
  importStates,
  needsTarget,
  photoManagementError,
  previewCounts,
  rejectDecision,
  reviewable,
  shortHash,
  submissionStates,
} from "@/pages/photo-management";
import { formatBytes } from "@/pages/print-status";

const JOB_POLL_MS = 2_000;
const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;

type Pending = {
  entry: PhotoPreviewEntry;
  decision: PhotoDecision;
  target: string;
};

export function PhotoImportsPage() {
  const { importId } = useParams();
  return importId ? (
    <ImportDetail key={importId} importId={importId} />
  ) : (
    <ImportStart />
  );
}

function ImportStart() {
  const toast = usePageToast();
  const navigate = useNavigate();
  const { api } = useSessionScope();
  const action = useOperatorAction(photoManagementError);
  const [open, setOpen] = useState("");
  const [file, setFile] = useState<File | null>(null);
  const openId = open.trim().toLowerCase();
  const upload = (chosen: File | undefined) => {
    if (!chosen || action.pending) return;
    setFile(chosen);
    void action.run(
      async (signal) => unwrap(await api.uploadPhotoImport(chosen, { signal })),
      (job) => {
        toast.success("Package uploaded", {
          description: "The Server is verifying signatures and images.",
        });
        void navigate(`/photo/imports/${job.import_id}`);
      },
    );
  };

  return (
    <div className="min-w-0 space-y-6">
      <PageHeader
        title="Photo imports"
        description="Upload the signed result package a Photo Client exported, check it against the current roster, then accept it."
      />
      {action.error && (
        <Alert variant="destructive">
          <AlertTitle>{action.error}</AlertTitle>
        </Alert>
      )}
      <div className="grid items-start gap-6 lg:grid-cols-[minmax(0,1fr)_18rem]">
        <PackageDropzone
          file={file}
          disabled={action.pending}
          onSelect={(files) => upload(files[0])}
        />
        <aside
          aria-label="Open an import"
          className="space-y-3 border bg-muted p-6 text-sm"
        >
          <p className="font-semibold">Open an earlier import</p>
          <p className="text-muted-foreground">
            The Server can’t list imports. Paste the import ID from the page
            address of an upload you started earlier.
          </p>
          <form
            className="flex gap-2"
            onSubmit={(event) => {
              event.preventDefault();
              if (UUID.test(openId)) void navigate(`/photo/imports/${openId}`);
            }}
          >
            <Input
              aria-label="Import ID"
              placeholder="Import ID"
              autoComplete="off"
              value={open}
              onChange={(event) => setOpen(event.target.value)}
            />
            <Button
              type="submit"
              variant="outline"
              disabled={!UUID.test(openId)}
            >
              <Search aria-hidden="true" />
              Open
            </Button>
          </form>
        </aside>
      </div>
    </div>
  );
}

function PackageDropzone({
  file,
  disabled,
  onSelect,
}: {
  file: File | null;
  disabled: boolean;
  onSelect: (files: File[]) => void;
}) {
  const [dragging, setDragging] = useState(false);
  const Icon = disabled ? LoaderCircle : file ? FileArchive : Upload;
  return (
    <div
      role="group"
      aria-label="Result package upload"
      className="relative"
      onDragEnter={(event) => {
        if (!disabled && event.dataTransfer.types.includes("Files")) {
          event.preventDefault();
          setDragging(true);
        }
      }}
      onDragOver={(event) => {
        event.preventDefault();
        event.dataTransfer.dropEffect = disabled ? "none" : "copy";
      }}
      onDragLeave={() => setDragging(false)}
      onDrop={(event) => {
        event.preventDefault();
        setDragging(false);
        if (!disabled) onSelect(Array.from(event.dataTransfer.files));
      }}
    >
      <input
        aria-label="Result package ZIP"
        aria-describedby="photo-package-hint"
        type="file"
        accept=".zip,application/zip"
        disabled={disabled}
        className="peer absolute inset-0 z-10 h-full w-full cursor-pointer opacity-0 disabled:cursor-not-allowed"
        onChange={(event) => {
          onSelect(Array.from(event.target.files ?? []));
          event.target.value = "";
        }}
      />
      <div
        className={cn(
          "pointer-events-none flex min-h-48 flex-col items-center justify-center gap-3 border-2 border-dashed px-6 py-8 text-center transition-colors peer-focus-visible:outline-2 peer-focus-visible:outline-offset-2 peer-focus-visible:outline-ring",
          disabled
            ? "border-border bg-muted/20 opacity-60"
            : dragging
              ? "border-primary bg-accent"
              : "border-border bg-muted/20 peer-hover:border-muted-foreground/50 peer-hover:bg-muted/40",
        )}
      >
        <div className="flex size-12 items-center justify-center border bg-background text-muted-foreground">
          <Icon
            aria-hidden="true"
            className={cn(
              "size-6",
              disabled && "animate-spin motion-reduce:animate-none",
            )}
          />
        </div>
        <div className="max-w-full space-y-1">
          <p className="text-sm font-semibold break-all">
            {disabled
              ? "Uploading package…"
              : dragging
                ? "Drop the package here"
                : (file?.name ?? "Drag & drop a result ZIP here")}
          </p>
          <p className="text-sm text-muted-foreground">
            {file
              ? `${formatBytes(file.size)} · ${disabled ? "Keep this page open until the upload finishes" : "Click or drop a file to try again"}`
              : "or click to browse files"}
          </p>
        </div>
        <p id="photo-package-hint" className="text-xs text-muted-foreground">
          One signed .zip from the Photo Client · Up to 8 GiB · Nothing is
          accepted until you commit
        </p>
      </div>
    </div>
  );
}

function ImportDetail({ importId }: { importId: string }) {
  const toast = usePageToast();
  const { api, queryClient } = useSessionScope();
  const action = useOperatorAction(photoManagementError);
  const [commitOpen, setCommitOpen] = useState(false);
  const [pending, setPending] = useState<Pending | null>(null);
  const [targets, setTargets] = useState<Record<string, string>>({});

  const job = useQuery({
    queryKey: ["photo-import", importId],
    queryFn: async ({ signal }) =>
      unwrap(await api.getPhotoImport(importId, { signal })),
    refetchInterval: (query) =>
      query.state.data && !importInFlight(query.state.data.state)
        ? false
        : JOB_POLL_MS,
  });
  const state = job.data?.state;
  const previewKey = ["photo-import-preview", importId];
  const preview = useQuery({
    queryKey: previewKey,
    queryFn: async ({ signal }) =>
      unwrap(await api.previewPhotoImport(importId, { signal })),
    enabled: state === "ready" || state === "committed",
  });
  const accounts = useQuery({
    queryKey: ["accounts"],
    queryFn: async ({ signal }) => unwrap(await api.listAccounts({ signal })),
    refetchInterval: LIST_POLL_MS,
  });
  const photos = useQuery({
    queryKey: ["photo-photos"],
    queryFn: async ({ signal }) => unwrap(await api.listPhotos({ signal })),
    refetchInterval: LIST_POLL_MS,
  });
  const byAccount = useMemo(
    () =>
      new Map(
        (accounts.data ?? []).map((account) => [account.account_id, account]),
      ),
    [accounts.data],
  );
  const committed = preview.data?.committed ?? state === "committed";
  const entries = preview.data?.entries ?? [];
  const counts = previewCounts(entries);

  const refreshAll = (next?: ResultImportPreview) => {
    if (next) queryClient.setQueryData(previewKey, next);
    for (const key of [
      ["photo-import", importId],
      ["photo-photos"],
      ["photo-submissions"],
    ])
      void queryClient.invalidateQueries({ queryKey: key });
  };

  const columns = useMemo<DataTableColumnDef<PhotoPreviewEntry>[]>(
    () => [
      {
        id: "team",
        header: "Team",
        cell: ({ row }) => (
          <PhotoTeam
            account={byAccount.get(row.original.account_id)}
            externalAccount={row.original.account}
          />
        ),
      },
      {
        id: "candidate",
        header: "Candidate",
        cell: ({ row }) => {
          const meta = submissionStates[row.original.state];
          return (
            <div className="space-y-1">
              <Badge variant={tones[meta.tone].badge} dot>
                {meta.label}
              </Badge>
              <p
                className="font-mono text-xs text-muted-foreground"
                title={row.original.image_sha256}
              >
                {shortHash(row.original.image_sha256)}
              </p>
            </div>
          );
        },
      },
      {
        id: "current",
        header: "Photo now",
        cell: ({ row }) =>
          row.original.current ? (
            <div className="flex items-center gap-2">
              <PhotoThumb
                accountId={row.original.account_id}
                label={row.original.account}
                className="size-12"
              />
              <span
                className="font-mono text-xs text-muted-foreground"
                title={row.original.current.image_sha256}
              >
                {shortHash(row.original.current.image_sha256)}
              </span>
            </div>
          ) : (
            <span className="text-muted-foreground">None</span>
          ),
      },
      {
        id: "action",
        header: "Decision",
        cell: ({ row }) => {
          const entry = row.original;
          if (!reviewable(entry))
            return <span className="text-muted-foreground">—</span>;
          if (!committed)
            return (
              <span className="text-xs text-muted-foreground">
                Commit the package first
              </span>
            );
          const orphan = needsTarget(entry);
          const target = orphan
            ? (targets[entry.submission_id] ?? "")
            : entry.account_id;
          return (
            <div className="flex flex-wrap items-center gap-2">
              {orphan && (
                <select
                  aria-label={`Assign ${entry.account} to team`}
                  value={target}
                  disabled={action.pending}
                  className="h-9 max-w-56 rounded-md border bg-background px-3 text-sm disabled:opacity-50"
                  onChange={(event) =>
                    setTargets({
                      ...targets,
                      [entry.submission_id]: event.target.value,
                    })
                  }
                >
                  <option value="">Choose team…</option>
                  {(accounts.data ?? [])
                    .filter((account) => account.team)
                    .map((account) => (
                      <option
                        key={account.account_id}
                        value={account.account_id}
                      >
                        {account.domjudge_username} ·{" "}
                        {account.team?.team_name_zh ||
                          account.team?.team_name_en}
                      </option>
                    ))}
                </select>
              )}
              <Button
                type="button"
                size="sm"
                disabled={action.pending || !target || !photos.data}
                onClick={() =>
                  setPending({
                    entry,
                    target,
                    decision: activateDecision(
                      entry,
                      target,
                      photos.data ?? [],
                    ),
                  })
                }
              >
                {entry.current && !orphan ? "Replace photo" : "Use photo"}
              </Button>
              <Button
                type="button"
                size="sm"
                variant="destructive-outline"
                disabled={action.pending}
                onClick={() =>
                  setPending({ entry, target, decision: rejectDecision(entry) })
                }
              >
                Reject
              </Button>
            </div>
          );
        },
      },
    ],
    [accounts.data, action.pending, byAccount, committed, photos.data, targets],
  );

  const failureNote = job.data?.failure_code
    ? importFailures[job.data.failure_code]
    : null;
  const pendingTeam = pending ? byAccount.get(pending.target) : undefined;
  const pendingLabel =
    pendingTeam?.domjudge_username ?? pending?.entry.account ?? "";

  return (
    <div className="min-w-0 space-y-6">
      <PageHeader
        title="Photo import"
        description={<span className="font-mono break-all">{importId}</span>}
        actions={
          <>
            <Button asChild variant="outline">
              <Link to="/photo/imports">
                <Upload aria-hidden="true" />
                Upload another package
              </Link>
            </Button>
            {state === "ready" && (
              <Button
                type="button"
                disabled={action.pending || !preview.data}
                onClick={() => setCommitOpen(true)}
              >
                <CheckCheck aria-hidden="true" />
                Commit package
              </Button>
            )}
          </>
        }
      />

      {action.error && !commitOpen && !pending && (
        <Alert variant="destructive">
          <AlertTitle>{action.error}</AlertTitle>
        </Alert>
      )}

      <DataState
        isLoading={job.isLoading}
        error={job.data ? null : job.error}
        isEmpty={!job.data}
        emptyLabel="Import not found"
      >
        {job.data && <JobSummary job={job.data} note={failureNote} />}
      </DataState>

      {(state === "ready" || state === "committed") && (
        <section aria-label="Package contents" className="space-y-3">
          <SectionHeader
            title="Package contents"
            description={
              committed
                ? "Accepted. Photos that need a decision stay here until you accept or reject them."
                : "Preview against the current roster. Nothing is accepted until you commit."
            }
          />
          {preview.data && (
            <div className="grid gap-3 sm:grid-cols-3">
              <StatTile label="Images" value={entries.length} tone="muted" />
              <StatTile
                label="Current photos"
                value={counts.current}
                tone="success"
              />
              <StatTile
                label="Need a decision"
                value={counts.reviewable}
                tone={counts.reviewable ? "warning" : "muted"}
                hint={
                  counts.reviewable && !committed
                    ? "Available after commit"
                    : undefined
                }
              />
            </div>
          )}
          {preview.isError && preview.data && (
            <Alert variant="destructive">
              <AlertTitle>
                Preview refresh failed. Showing potentially outdated data.
              </AlertTitle>
            </Alert>
          )}
          <DataState
            isLoading={preview.isLoading}
            error={preview.data ? null : preview.error}
            isEmpty={!entries.length}
            emptyLabel="This package has no images"
          >
            <DataTable
              label="Package images"
              columns={columns}
              data={entries}
              getRowId={(entry) => entry.submission_id}
            />
          </DataState>
        </section>
      )}

      <AlertDialog
        open={commitOpen}
        onOpenChange={(next) => {
          if (!next && !action.pending) setCommitOpen(false);
        }}
      >
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogMedia>
              <CheckCheck aria-hidden="true" />
            </AlertDialogMedia>
            <AlertDialogTitle>Commit this package?</AlertDialogTitle>
            <AlertDialogDescription>
              First photos for a team become current. A different new photo for
              a team that already has one waits for your decision, and teams no
              longer on the roster need a new assignment. Committing the same
              package again doesn’t duplicate anything.
            </AlertDialogDescription>
          </AlertDialogHeader>
          {action.error && (
            <Alert variant="destructive">
              <AlertTitle>{action.error}</AlertTitle>
            </Alert>
          )}
          <AlertDialogFooter>
            <AlertDialogCancel disabled={action.pending}>
              Not yet
            </AlertDialogCancel>
            <Button
              type="button"
              loading={action.pending}
              onClick={() =>
                void action.run(
                  async (signal) =>
                    unwrap(await api.commitPhotoImport(importId, { signal })),
                  (next) => {
                    toast.success("Package committed");
                    setCommitOpen(false);
                    refreshAll(next);
                  },
                )
              }
            >
              Commit package
            </Button>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>

      <AlertDialog
        open={pending !== null}
        onOpenChange={(next) => {
          if (!next && !action.pending) setPending(null);
        }}
      >
        <AlertDialogContent>
          {pending && (
            <>
              <AlertDialogHeader>
                <AlertDialogMedia>
                  {pending.decision.action.action === "reject" ? (
                    <X aria-hidden="true" />
                  ) : (
                    <CheckCheck aria-hidden="true" />
                  )}
                </AlertDialogMedia>
                <AlertDialogTitle>
                  {pending.decision.action.action === "reject"
                    ? `Reject the photo for ${pending.entry.account}?`
                    : `Use this photo for ${pendingLabel}?`}
                </AlertDialogTitle>
                <AlertDialogDescription>
                  {pending.decision.action.action === "reject"
                    ? "The candidate is discarded and the team keeps its current photo."
                    : pending.decision.action.expected_current
                      ? "It replaces the team’s current photo, which is kept as superseded. The decision fails if that photo changed in the meantime."
                      : "It becomes the team’s current photo."}
                </AlertDialogDescription>
              </AlertDialogHeader>
              {action.error && (
                <Alert variant="destructive">
                  <AlertTitle>{action.error}</AlertTitle>
                </Alert>
              )}
              <AlertDialogFooter>
                <AlertDialogCancel disabled={action.pending}>
                  Cancel
                </AlertDialogCancel>
                <Button
                  type="button"
                  variant={
                    pending.decision.action.action === "reject"
                      ? "destructive"
                      : "default"
                  }
                  loading={action.pending}
                  onClick={() =>
                    void action.run(
                      async (signal) =>
                        unwrap(
                          await api.decidePhotoImport(
                            importId,
                            pending.decision,
                            { signal },
                          ),
                        ),
                      (next) => {
                        toast.success(
                          pending.decision.action.action === "reject"
                            ? "Photo rejected"
                            : "Photo accepted",
                          { description: pending.entry.account },
                        );
                        setPending(null);
                        refreshAll(next);
                      },
                    )
                  }
                >
                  {pending.decision.action.action === "reject"
                    ? "Reject photo"
                    : "Use photo"}
                </Button>
              </AlertDialogFooter>
            </>
          )}
        </AlertDialogContent>
      </AlertDialog>
    </div>
  );
}

function JobSummary({
  job,
  note,
}: {
  job: ResultImportJob;
  note: string | null;
}) {
  const meta = importStates[job.state];
  return (
    <section
      aria-label="Import status"
      className="flex flex-wrap items-center gap-x-6 gap-y-3 border bg-card px-4 py-3.5"
    >
      <Badge variant={tones[meta.tone].badge} dot>
        {meta.label}
      </Badge>
      <p className="text-sm text-muted-foreground">
        <span className="tabular-nums">{formatBytes(job.uploaded_bytes)}</span>{" "}
        received
        {job.state !== "uploading" && (
          <>
            {" · "}
            <span className="tabular-nums">{job.verified_images}</span> images
            verified
          </>
        )}
      </p>
      {importInFlight(job.state) && (
        <p className="text-sm text-muted-foreground">
          This page updates automatically.
        </p>
      )}
      {note && (
        <Alert variant="destructive" className="basis-full">
          <AlertTitle>{note}</AlertTitle>
        </Alert>
      )}
    </section>
  );
}
