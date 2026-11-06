import { useState, type ReactNode } from "react";
import { useQuery } from "@tanstack/react-query";
import {
  Ban,
  Camera,
  CirclePause,
  CirclePlay,
  FileCheck2,
  KeyRound,
  Plus,
  RotateCcw,
  ShieldAlert,
} from "lucide-react";

import { usePageToast } from "@/components/use-page-toast";

import { unwrap } from "@/api/errors";
import type { PhotoPairingCode, PhotoSource } from "@/api/generated/types";
import { LIST_POLL_MS } from "@/api/polling";
import { useSessionScope } from "@/auth/session-context";
import { CopyButton } from "@/components/copy-button";
import { DataState } from "@/components/data-state";
import { RefreshCountdown } from "@/components/device-status";
import { tones } from "@/components/device-status-style";
import { PageHeader } from "@/components/page-header";
import { PrintNameDialog } from "@/components/print-name-dialog";
import { PrintSecret } from "@/components/print-secret";
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
import { cn } from "@/lib/utils";
import { useOperatorAction } from "@/operator/use-operator-action";
import {
  photoManagementError,
  sourceActions,
  sourceStatus,
} from "@/pages/photo-management";

type Confirmation = { kind: "reset" | "revoke"; source: PhotoSource };

const confirmationCopy = {
  reset: {
    icon: RotateCcw,
    title: "Replace enrollment for",
    description:
      "This invalidates the current binding, public key, and credentials of this source, discards its unfinished initialization snapshot, and issues a new pairing code. The Photo Client must pair again with a new local workspace path.",
    keep: "Keep enrollment",
    confirm: "Replace enrollment",
    done: "Enrollment replaced",
  },
  revoke: {
    icon: Ban,
    title: "Revoke",
    description:
      "This permanently stops this source from authenticating or being used to accept new packages and can’t be undone. Photos already accepted are not deleted. Use Disable instead if you may need the source again.",
    keep: "Keep source",
    confirm: "Revoke source",
    done: "Source revoked",
  },
} as const;

export function PhotoSourcesPage() {
  const toast = usePageToast();
  const { api, queryClient } = useSessionScope();
  const action = useOperatorAction(photoManagementError);
  const [createOpen, setCreateOpen] = useState(false);
  const [issued, setIssued] = useState<PhotoPairingCode | null>(null);
  const [confirmation, setConfirmation] = useState<Confirmation | null>(null);
  const sources = useQuery({
    queryKey: ["photo-sources"],
    queryFn: async ({ signal }) =>
      unwrap(await api.listPhotoSources({ signal })),
    refetchInterval: LIST_POLL_MS,
  });
  const refresh = () => {
    void queryClient.invalidateQueries({ queryKey: ["photo-sources"] });
  };
  const list = sources.data ?? [];
  const live = list.filter((source) => source.state !== "revoked");
  const revoked = list.filter((source) => source.state === "revoked");
  const issuedSource = list.find(
    (source) => source.source_id === issued?.source_id,
  );
  const issueCode = (source: PhotoSource) => {
    setIssued(null);
    void action.run(
      async (signal) =>
        unwrap(await api.issuePhotoPairingCode(source.source_id, { signal })),
      (code) => {
        setIssued(code);
        refresh();
      },
    );
  };
  const setState = (source: PhotoSource, state: "enabled" | "disabled") => {
    setIssued(null);
    void action.run(
      async (signal) =>
        unwrap(
          await api.updatePhotoSourceState(
            source.source_id,
            { state },
            { signal },
          ),
        ),
      () => {
        toast.success(
          state === "enabled" ? "Source enabled" : "Source disabled",
          {
            description: source.name,
          },
        );
        refresh();
      },
    );
  };
  const confirmed = confirmation ? confirmationCopy[confirmation.kind] : null;

  return (
    <div className="min-w-0 space-y-6">
      <PageHeader
        title="Photo sources"
        description="Photo stations allowed to initialize from the roster and sign result packages for import."
        actions={
          <>
            <RefreshCountdown
              resource="source"
              updatedAt={Math.max(
                sources.dataUpdatedAt,
                sources.errorUpdatedAt,
              )}
              isFetching={sources.isFetching}
              isPaused={sources.isPaused}
              isError={sources.isError}
              onRefresh={refresh}
            />
            <Button type="button" onClick={() => setCreateOpen(true)}>
              <Plus aria-hidden="true" />
              Create source
            </Button>
          </>
        }
      />

      {action.error && !createOpen && !confirmation && (
        <Alert variant="destructive">
          <AlertTitle>{action.error}</AlertTitle>
        </Alert>
      )}
      {issued && (
        <PrintSecret
          key={issued.code}
          kind="code"
          title={`Pairing code${issuedSource ? ` for ${issuedSource.name}` : ""}`}
          value={issued.code}
          expires={issued.expires_at_unix_ms}
          description={
            <>
              <p>Enter this code in the Photo Client during first setup.</p>
              <p>
                It works once and is replaced by the next code you issue. It
                can’t be shown again after you dismiss it.
              </p>
            </>
          }
          meta={<span className="font-mono">Source ID {issued.source_id}</span>}
          onDismiss={() => setIssued(null)}
        />
      )}
      {sources.isError && sources.data && (
        <Alert variant="destructive">
          <AlertTitle>
            Source refresh failed. Showing potentially outdated source data.
          </AlertTitle>
        </Alert>
      )}

      <div className="grid items-start gap-6 lg:grid-cols-[minmax(0,1fr)_18rem]">
        <DataState
          isLoading={sources.isLoading}
          error={sources.data ? null : sources.error}
          isEmpty={!list.length}
          emptyLabel="No photo sources"
          emptyDescription="Create a source for each photo station, then issue its pairing code."
        >
          <div className="space-y-6">
            <SourceGroup
              label="Active"
              count={live.length}
              sources={live}
              empty="No usable sources. Create one before a station can initialize."
              disabled={action.pending}
              onIssue={issueCode}
              onReset={(source) => {
                setIssued(null);
                setConfirmation({ kind: "reset", source });
              }}
              onRevoke={(source) => {
                setIssued(null);
                setConfirmation({ kind: "revoke", source });
              }}
              onState={setState}
            />
            {revoked.length > 0 && (
              <SourceGroup
                label="Revoked"
                count={revoked.length}
                sources={revoked}
                disabled
                onIssue={issueCode}
                onReset={() => {}}
                onRevoke={() => {}}
                onState={() => {}}
              />
            )}
          </div>
        </DataState>

        <aside
          aria-label="About photo sources"
          className="space-y-4 border bg-muted p-6 text-sm"
        >
          <p className="font-semibold">How sources work</p>
          <ul className="space-y-3 text-muted-foreground">
            <Point icon={KeyRound}>
              A pairing code is valid for ten minutes and can be used once. The
              station keeps its own signing key.
            </Point>
            <Point icon={FileCheck2}>
              After initialization the station works offline; its signed
              packages are uploaded on the Photo imports page.
            </Point>
            <Point icon={RotateCcw}>
              Replacing enrollment is only possible before initialization
              completes.
            </Point>
            <Point icon={ShieldAlert}>
              Disabling can be undone. Revoking is permanent and keeps photos
              already accepted.
            </Point>
          </ul>
        </aside>
      </div>

      <PrintNameDialog
        id="photo-source-name"
        open={createOpen}
        onOpenChange={setCreateOpen}
        title="Create photo source"
        description="Creates a source without credentials. Issue its pairing code afterwards."
        label="Source name"
        placeholder="Photo station 1"
        hint="Up to 128 characters. Name it after the station or photographer."
        submitLabel="Create source"
        pending={action.pending}
        error={action.error}
        onSubmit={(name) => {
          setIssued(null);
          void action.run(
            async (signal) =>
              unwrap(await api.createPhotoSource({ name }, { signal })),
            (source) => {
              setCreateOpen(false);
              toast.success("Source created", {
                description: `Issue a pairing code for ${source.name} to connect its station.`,
              });
              refresh();
            },
          );
        }}
      />

      <AlertDialog
        open={confirmation !== null}
        onOpenChange={(open) => {
          if (!open && !action.pending) setConfirmation(null);
        }}
      >
        <AlertDialogContent>
          {confirmation && confirmed && (
            <>
              <AlertDialogHeader>
                <AlertDialogMedia>
                  <confirmed.icon aria-hidden="true" />
                </AlertDialogMedia>
                <AlertDialogTitle>
                  {confirmed.title} {confirmation.source.name}?
                </AlertDialogTitle>
                <AlertDialogDescription>
                  {confirmed.description}
                </AlertDialogDescription>
              </AlertDialogHeader>
              {action.error && (
                <Alert variant="destructive">
                  <AlertTitle>{action.error}</AlertTitle>
                </Alert>
              )}
              <AlertDialogFooter>
                <AlertDialogCancel disabled={action.pending}>
                  {confirmed.keep}
                </AlertDialogCancel>
                <Button
                  type="button"
                  variant="destructive"
                  loading={action.pending}
                  onClick={() => {
                    const { kind, source } = confirmation;
                    void action.run(
                      async (signal) =>
                        kind === "reset"
                          ? unwrap(
                              await api.resetPhotoEnrollment(source.source_id, {
                                signal,
                              }),
                            )
                          : unwrap(
                              await api.updatePhotoSourceState(
                                source.source_id,
                                { state: "revoked" },
                                { signal },
                              ),
                            ),
                      (code) => {
                        toast.success(confirmed.done, {
                          description: source.name,
                        });
                        if (code) setIssued(code);
                        setConfirmation(null);
                        refresh();
                      },
                    );
                  }}
                >
                  {confirmed.confirm}
                </Button>
              </AlertDialogFooter>
            </>
          )}
        </AlertDialogContent>
      </AlertDialog>
    </div>
  );
}

function SourceGroup({
  label,
  count,
  sources,
  empty,
  disabled,
  onIssue,
  onReset,
  onRevoke,
  onState,
}: {
  label: string;
  count: number;
  sources: PhotoSource[];
  empty?: string;
  disabled: boolean;
  onIssue: (source: PhotoSource) => void;
  onReset: (source: PhotoSource) => void;
  onRevoke: (source: PhotoSource) => void;
  onState: (source: PhotoSource, state: "enabled" | "disabled") => void;
}) {
  return (
    <section aria-label={`${label} sources`} className="space-y-2">
      <div className="flex items-center gap-2">
        <p className="text-sm font-semibold">{label}</p>
        <Badge variant="muted">{count}</Badge>
      </div>
      {sources.length ? (
        <ul className="divide-y overflow-hidden border bg-card">
          {sources.map((source) => {
            const status = sourceStatus(source);
            const actions = sourceActions(source);
            const revokedSource = source.state === "revoked";
            return (
              <li
                key={source.source_id}
                className={cn(
                  "flex flex-wrap items-center gap-x-4 gap-y-3 px-4 py-3.5",
                  revokedSource && "bg-muted/30",
                )}
              >
                <span
                  className={cn(
                    "flex size-10 shrink-0 items-center justify-center",
                    revokedSource
                      ? "bg-muted text-muted-foreground"
                      : "bg-info-subtle text-primary",
                  )}
                >
                  <Camera aria-hidden="true" className="size-5" />
                </span>
                <div className="min-w-0 flex-1 basis-48">
                  <p
                    className={cn(
                      "font-medium break-words",
                      revokedSource && "text-muted-foreground line-through",
                    )}
                  >
                    {source.name}
                  </p>
                  <span className="flex items-center gap-1">
                    <code className="truncate font-mono text-xs text-muted-foreground">
                      {source.source_id}
                    </code>
                    <CopyButton
                      value={source.source_id}
                      label="Copy source ID"
                      className="size-6"
                    />
                  </span>
                </div>
                <Badge variant={tones[status.tone].badge} dot>
                  {status.label}
                </Badge>
                <div className="flex flex-wrap items-center gap-2">
                  {actions.issueCode && (
                    <Button
                      type="button"
                      size="sm"
                      disabled={disabled}
                      onClick={() => onIssue(source)}
                    >
                      <KeyRound aria-hidden="true" />
                      Issue pairing code
                    </Button>
                  )}
                  {actions.resetEnrollment && (
                    <Button
                      type="button"
                      size="sm"
                      variant="outline"
                      disabled={disabled}
                      onClick={() => onReset(source)}
                    >
                      <RotateCcw aria-hidden="true" />
                      Replace enrollment
                    </Button>
                  )}
                  {actions.disable && (
                    <Button
                      type="button"
                      size="sm"
                      variant="outline"
                      disabled={disabled}
                      onClick={() => onState(source, "disabled")}
                    >
                      <CirclePause aria-hidden="true" />
                      Disable
                    </Button>
                  )}
                  {actions.enable && (
                    <Button
                      type="button"
                      size="sm"
                      variant="outline"
                      disabled={disabled}
                      onClick={() => onState(source, "enabled")}
                    >
                      <CirclePlay aria-hidden="true" />
                      Enable
                    </Button>
                  )}
                  {actions.revoke && (
                    <Button
                      type="button"
                      size="sm"
                      variant="destructive-outline"
                      disabled={disabled}
                      onClick={() => onRevoke(source)}
                    >
                      Revoke
                    </Button>
                  )}
                </div>
              </li>
            );
          })}
        </ul>
      ) : (
        <p className="border px-4 py-6 text-center text-sm text-muted-foreground">
          {empty}
        </p>
      )}
    </section>
  );
}

function Point({
  icon: Icon,
  children,
}: {
  icon: typeof KeyRound;
  children: ReactNode;
}) {
  return (
    <li className="flex gap-2.5">
      <Icon
        aria-hidden="true"
        className="mt-0.5 size-4 shrink-0 text-foreground"
      />
      <span>{children}</span>
    </li>
  );
}
