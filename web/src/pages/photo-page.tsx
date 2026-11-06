import { useId, useMemo, useState } from "react";
import { useInfiniteQuery, useMutation, useQuery } from "@tanstack/react-query";
import { Download, FileArchive, Images } from "lucide-react";
import { Link } from "react-router";

import { unwrap } from "@/api/errors";
import type {
  AccountResponse_Serialize as AccountResponse,
  PendingPhoto_Serialize as PendingPhoto,
  PhotoRecord,
} from "@/api/generated/types";
import { LIST_POLL_MS } from "@/api/polling";
import { useSessionScope } from "@/auth/session-context";
import { DataState } from "@/components/data-state";
import { DataTable, type DataTableColumnDef } from "@/components/data-table";
import { RefreshCountdown } from "@/components/device-status";
import { tones } from "@/components/device-status-style";
import { PageHeader } from "@/components/page-header";
import { PhotoTeam } from "@/components/photo-team";
import { PhotoThumb } from "@/components/photo-thumb";
import { TeamName, TeamSchool } from "@/components/roster-team";
import { StatTile } from "@/components/stat-tile";
import { Alert, AlertTitle } from "@/components/ui/alert";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { ViewTabPanel, ViewTabs } from "@/components/view-tabs";
import {
  photoManagementError,
  shortHash,
  submissionStates,
} from "@/pages/photo-management";
import { formatBytes } from "@/pages/print-status";

type View = "photos" | "missing" | "attention";

function download(blob: Blob, filename: string) {
  const url = URL.createObjectURL(blob);
  const link = document.createElement("a");
  link.href = url;
  link.download = filename;
  link.click();
  // Let the browser take ownership of the download before releasing the URL.
  window.setTimeout(() => URL.revokeObjectURL(url), 1000);
}

export function PhotoPage() {
  const { api, queryClient } = useSessionScope();
  const scope = useId();
  const [view, setView] = useState<View>("photos");
  const [search, setSearch] = useState("");
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
  const submissions = useInfiniteQuery({
    queryKey: ["photo-submissions"],
    queryFn: async ({ pageParam, signal }) =>
      unwrap(await api.listPhotoSubmissions(pageParam, { signal })),
    initialPageParam: null as { source: string; submission: string } | null,
    getNextPageParam: (page) =>
      page.next
        ? {
            source: page.next.after_source_id,
            submission: page.next.after_submission_id,
          }
        : undefined,
    refetchInterval: LIST_POLL_MS,
  });
  const exportPhotos = useMutation({
    mutationFn: async () =>
      download(await unwrap<Blob>(await api.exportPhotos()), "team-photos.zip"),
    gcTime: 0,
  });
  const exportPyrite = useMutation({
    mutationFn: async () =>
      download(
        await unwrap<Blob>(await api.exportPyrite()),
        "pyrite-assets.zip",
      ),
    gcTime: 0,
  });
  const exporting = exportPhotos.isPending || exportPyrite.isPending;
  const exportError = exportPhotos.error ?? exportPyrite.error;
  const refresh = () => {
    for (const key of [["accounts"], ["photo-photos"], ["photo-submissions"]])
      void queryClient.invalidateQueries({ queryKey: key });
  };

  const byAccount = useMemo(
    () =>
      new Map(
        (accounts.data ?? []).map((account) => [account.account_id, account]),
      ),
    [accounts.data],
  );
  const attention = useMemo(
    () => submissions.data?.pages.flatMap((page) => page.entries) ?? [],
    [submissions.data],
  );
  const photoList = photos.data;
  const missing = useMemo(() => {
    const have = new Set((photoList ?? []).map((photo) => photo.account_id));
    return (accounts.data ?? []).filter(
      (account) => account.team && !have.has(account.account_id),
    );
  }, [accounts.data, photoList]);
  const query = search.trim().toLocaleLowerCase();
  const matches = (account: AccountResponse | undefined, external: string) =>
    !query ||
    [
      external,
      account?.team?.team_name_zh,
      account?.team?.team_name_en,
      account?.team?.school_name_zh,
      account?.team?.school_name_en,
    ].some((value) => value?.toLocaleLowerCase().includes(query));
  const shownPhotos = (photoList ?? []).filter((photo) =>
    matches(byAccount.get(photo.account_id), photo.account),
  );
  const shownMissing = missing.filter((account) =>
    matches(account, account.domjudge_username),
  );
  const shownAttention = attention.filter((entry) =>
    matches(byAccount.get(entry.account_id), entry.account),
  );

  const photoColumns = useMemo<DataTableColumnDef<PhotoRecord>[]>(
    () => [
      {
        id: "photo",
        header: "Photo",
        cell: ({ row }) => (
          <PhotoThumb
            accountId={row.original.account_id}
            label={row.original.account}
          />
        ),
      },
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
        id: "school",
        header: "School",
        cell: ({ row }) => (
          <TeamSchool team={byAccount.get(row.original.account_id)?.team} />
        ),
      },
      {
        id: "file",
        header: "Image",
        cell: ({ row }) => (
          <div className="text-xs text-muted-foreground">
            <p className="uppercase">
              {row.original.image_format} ·{" "}
              {formatBytes(row.original.image_bytes)}
            </p>
            <p className="font-mono" title={row.original.image_sha256}>
              {shortHash(row.original.image_sha256)}
            </p>
          </div>
        ),
      },
    ],
    [byAccount],
  );
  const missingColumns = useMemo<DataTableColumnDef<AccountResponse>[]>(
    () => [
      { accessorKey: "domjudge_username", header: "DOMjudge username" },
      {
        id: "team",
        header: "Team",
        cell: ({ row }) => <TeamName team={row.original.team} />,
      },
      {
        id: "school",
        header: "School",
        cell: ({ row }) => <TeamSchool team={row.original.team} />,
      },
    ],
    [],
  );
  const attentionColumns = useMemo<DataTableColumnDef<PendingPhoto>[]>(
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
        id: "state",
        header: "State",
        cell: ({ row }) => {
          const meta = submissionStates[row.original.state];
          return (
            <Badge variant={tones[meta.tone].badge} dot>
              {meta.label}
            </Badge>
          );
        },
      },
      {
        id: "image",
        header: "Candidate",
        cell: ({ row }) => (
          <span
            className="font-mono text-xs text-muted-foreground"
            title={row.original.image_sha256}
          >
            {shortHash(row.original.image_sha256)}
          </span>
        ),
      },
      {
        id: "review",
        header: "Review",
        cell: ({ row }) => (
          <Button asChild size="sm" variant="outline">
            <Link to={`/photo/imports/${row.original.import_id}`}>
              Open import
            </Link>
          </Button>
        ),
      },
    ],
    [byAccount],
  );

  const teams = (accounts.data ?? []).filter((account) => account.team).length;

  return (
    <div className="min-w-0 space-y-6">
      <PageHeader
        title="Team photos"
        description="Current photo of each team, photos waiting for a decision, and exports for printing or the scoreboard."
        actions={
          <>
            <RefreshCountdown
              resource="photo"
              updatedAt={Math.max(photos.dataUpdatedAt, photos.errorUpdatedAt)}
              isFetching={photos.isFetching}
              isPaused={photos.isPaused}
              isError={photos.isError}
              onRefresh={refresh}
            />
            <Button
              type="button"
              variant="outline"
              loading={exportPhotos.isPending}
              disabled={exporting}
              onClick={() => exportPhotos.mutate()}
            >
              <Download aria-hidden="true" />
              Export photos
            </Button>
            <Button
              type="button"
              variant="outline"
              loading={exportPyrite.isPending}
              disabled={exporting}
              onClick={() => exportPyrite.mutate()}
            >
              <FileArchive aria-hidden="true" />
              Export Pyrite assets
            </Button>
          </>
        }
      />

      {exportError && (
        <Alert variant="destructive">
          <AlertTitle>{photoManagementError(exportError)}</AlertTitle>
        </Alert>
      )}
      {(photos.isError && photos.data) ||
      (submissions.isError && submissions.data) ? (
        <Alert variant="destructive">
          <AlertTitle>
            Photo refresh failed. Showing potentially outdated data.
          </AlertTitle>
        </Alert>
      ) : null}

      <div className="grid gap-3 sm:grid-cols-3">
        <StatTile
          label="Teams with a photo"
          value={
            photoList && accounts.data ? `${photoList.length} / ${teams}` : "—"
          }
          tone="success"
          icon={<Images aria-hidden="true" />}
          active={view === "photos"}
          onClick={() => setView("photos")}
        />
        <StatTile
          label="Without a photo"
          value={photoList && accounts.data ? missing.length : "—"}
          tone={missing.length ? "warning" : "muted"}
          active={view === "missing"}
          onClick={() => setView("missing")}
        />
        <StatTile
          label="Need a decision"
          value={submissions.data ? attention.length : "—"}
          hint={submissions.hasNextPage ? "More pages available" : undefined}
          tone={attention.length ? "warning" : "muted"}
          active={view === "attention"}
          onClick={() => setView("attention")}
        />
      </div>

      <div className="flex flex-wrap items-center gap-3">
        <ViewTabs
          scope={scope}
          label="Photo views"
          value={view}
          onValueChange={setView}
          items={[
            { value: "photos", label: "Current photos" },
            { value: "missing", label: "Without photo" },
            { value: "attention", label: "Needs decision" },
          ]}
        />
        <Input
          aria-label="Filter by team, school, or username"
          placeholder="Filter by team, school, or username"
          value={search}
          onChange={(event) => setSearch(event.target.value)}
          className="max-w-xs"
        />
      </div>

      <ViewTabPanel scope={scope} value={view}>
        {view === "photos" && (
          <DataState
            isLoading={photos.isLoading}
            error={photos.data ? null : photos.error}
            isEmpty={!photoList?.length}
            emptyLabel="No team photos yet"
            emptyDescription="Import and commit a Photo Client package to publish photos."
            emptyAction={
              <Button asChild variant="outline">
                <Link to="/photo/imports">Open Photo imports</Link>
              </Button>
            }
          >
            <DataTable
              label="Current photos"
              columns={photoColumns}
              data={shownPhotos}
              getRowId={(photo) => photo.account_id}
              emptyLabel="No photos match the filter."
            />
          </DataState>
        )}
        {view === "missing" && (
          <DataState
            isLoading={accounts.isLoading || photos.isLoading}
            error={
              accounts.data && photos.data
                ? null
                : (accounts.error ?? photos.error)
            }
            isEmpty={!missing.length}
            emptyLabel="Every team has a photo"
          >
            <DataTable
              label="Teams without a photo"
              columns={missingColumns}
              data={shownMissing}
              getRowId={(account) => account.account_id}
              emptyLabel="No teams match the filter."
            />
          </DataState>
        )}
        {view === "attention" && (
          <div className="space-y-3">
            <DataState
              isLoading={submissions.isLoading}
              error={submissions.data ? null : submissions.error}
              isEmpty={!attention.length}
              emptyLabel="Nothing needs a decision"
              emptyDescription="Photos that differ from a team’s current photo, or whose team left the roster, appear here."
            >
              <DataTable
                label="Photos needing a decision"
                columns={attentionColumns}
                data={shownAttention}
                getRowId={(entry) =>
                  `${entry.source_id}/${entry.submission_id}`
                }
                emptyLabel="No photos match the filter."
              />
            </DataState>
            {submissions.hasNextPage && (
              <Button
                type="button"
                variant="outline"
                loading={submissions.isFetchingNextPage}
                onClick={() => void submissions.fetchNextPage()}
              >
                Load more
              </Button>
            )}
          </div>
        )}
      </ViewTabPanel>
    </div>
  );
}
