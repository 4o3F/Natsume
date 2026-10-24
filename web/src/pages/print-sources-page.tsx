import { useState, type ReactNode } from "react";
import { useQuery } from "@tanstack/react-query";
import {
  Ban,
  FileCheck2,
  KeyRound,
  Plug,
  Plus,
  ShieldAlert,
  ShieldCheck,
} from "lucide-react";

import { usePageToast } from "@/components/use-page-toast";

import { unwrap } from "@/api/errors";
import type { PrintSource, PrintSourceIssued } from "@/api/generated/types";
import { LIST_POLL_MS } from "@/api/polling";
import { useSessionScope } from "@/auth/session-context";
import { CopyButton } from "@/components/copy-button";
import { DataState } from "@/components/data-state";
import { RefreshCountdown } from "@/components/device-status";
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
import { printManagementError } from "@/pages/print-management";

export function PrintSourcesPage() {
  const toast = usePageToast();
  const { api, queryClient } = useSessionScope();
  const action = useOperatorAction(printManagementError);
  const [createOpen, setCreateOpen] = useState(false);
  const [issued, setIssued] = useState<PrintSourceIssued | null>(null);
  const [confirmation, setConfirmation] = useState<PrintSource | null>(null);
  const sources = useQuery({
    queryKey: ["print-sources"],
    queryFn: async ({ signal }) =>
      unwrap(await api.listPrintSources({ signal })),
    refetchInterval: LIST_POLL_MS,
  });
  const refresh = () => {
    void queryClient.invalidateQueries({ queryKey: ["print-sources"] });
  };
  const active = sources.data?.filter((source) => !source.revoked) ?? [];
  const revoked = sources.data?.filter((source) => source.revoked) ?? [];

  return (
    <div className="min-w-0 space-y-6">
      <PageHeader
        title="Print sources"
        description="Credentials that let a DOMjudge instance submit documents to the print queue."
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
          key={issued.source_id}
          kind="token"
          title={`Token for ${issued.name}`}
          value={issued.token}
          description={
            <>
              <p>
                Save it in the deployment-owned restricted token file now. It
                can’t be recovered from this list.
              </p>
              <p>
                Dismissing it or leaving the page removes it from view. Copying
                uses the system clipboard, which this page doesn’t clear.
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
          isEmpty={!sources.data?.length}
          emptyLabel="No print sources"
          emptyDescription="Create a source for each DOMjudge instance that should print."
        >
          <div className="space-y-6">
            <SourceGroup
              label="Active"
              count={active.length}
              sources={active}
              empty="No active sources. Documents can’t be submitted until you create one."
              disabled={action.pending}
              onRevoke={(source) => {
                setIssued(null);
                setConfirmation(source);
              }}
            />
            {revoked.length > 0 && (
              <SourceGroup
                label="Revoked"
                count={revoked.length}
                sources={revoked}
                disabled
                onRevoke={() => {}}
              />
            )}
          </div>
        </DataState>

        <aside
          aria-label="About print sources"
          className="space-y-4 border bg-muted p-6 text-sm"
        >
          <p className="font-semibold">How sources work</p>
          <ul className="space-y-3 text-muted-foreground">
            <Point icon={KeyRound}>
              Each source gets one token, shown once when it’s created.
            </Point>
            <Point icon={FileCheck2}>
              DOMjudge uses the token to admit documents into the queue.
            </Point>
            <Point icon={ShieldAlert}>
              Tokens don’t authorize Windows clients or administrator
              operations.
            </Point>
            <Point icon={Ban}>
              Revoking is permanent and doesn’t cancel documents already
              admitted.
            </Point>
          </ul>
        </aside>
      </div>

      <PrintNameDialog
        id="source-name"
        open={createOpen}
        onOpenChange={setCreateOpen}
        title="Create print source"
        description="A token is generated for this source and shown once. Have the deployment’s token file ready."
        label="Source name"
        placeholder="DOMjudge Hall A"
        hint="Up to 128 characters. Name it after the DOMjudge instance that will use it."
        submitLabel="Create source"
        pending={action.pending}
        error={action.error}
        onSubmit={(name) => {
          setIssued(null);
          void action.run(
            async (signal) =>
              unwrap(await api.createPrintSource({ name }, { signal })),
            (source) => {
              setCreateOpen(false);
              setIssued(source);
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
          <AlertDialogHeader>
            <AlertDialogMedia>
              <Ban aria-hidden="true" />
            </AlertDialogMedia>
            <AlertDialogTitle>Revoke {confirmation?.name}?</AlertDialogTitle>
            <AlertDialogDescription>
              This stops future authentication for this source and can’t be
              undone. Existing requests, admitted documents and execution
              history stay as they are; printing isn’t cancelled.
            </AlertDialogDescription>
          </AlertDialogHeader>
          {action.error && (
            <Alert variant="destructive">
              <AlertTitle>{action.error}</AlertTitle>
            </Alert>
          )}
          <AlertDialogFooter>
            <AlertDialogCancel disabled={action.pending}>
              Keep source
            </AlertDialogCancel>
            <Button
              type="button"
              variant="destructive"
              loading={action.pending}
              onClick={() => {
                if (!confirmation) return;
                void action.run(
                  async (signal) =>
                    unwrap(
                      await api.revokePrintSource(confirmation.source_id, {
                        signal,
                      }),
                    ),
                  () => {
                    toast.success("Source revoked", {
                      description: confirmation.name,
                    });
                    setConfirmation(null);
                    refresh();
                  },
                );
              }}
            >
              Revoke source
            </Button>
          </AlertDialogFooter>
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
  onRevoke,
}: {
  label: string;
  count: number;
  sources: PrintSource[];
  empty?: string;
  disabled: boolean;
  onRevoke: (source: PrintSource) => void;
}) {
  return (
    <section aria-label={`${label} sources`} className="space-y-2">
      <div className="flex items-center gap-2">
        <p className="text-sm font-semibold">{label}</p>
        <Badge variant="muted">{count}</Badge>
      </div>
      {sources.length ? (
        <ul className="divide-y overflow-hidden border bg-card">
          {sources.map((source) => (
            <li
              key={source.source_id}
              className={cn(
                "flex flex-wrap items-center gap-x-4 gap-y-3 px-4 py-3.5",
                source.revoked && "bg-muted/30",
              )}
            >
              <span
                className={cn(
                  "flex size-10 shrink-0 items-center justify-center rounded-lg",
                  source.revoked
                    ? "bg-muted text-muted-foreground"
                    : "bg-info-subtle text-primary",
                )}
              >
                {source.revoked ? (
                  <Ban aria-hidden="true" className="size-5" />
                ) : (
                  <Plug aria-hidden="true" className="size-5" />
                )}
              </span>
              <div className="min-w-0 flex-1 basis-48">
                <p
                  className={cn(
                    "font-medium break-words",
                    source.revoked && "text-muted-foreground line-through",
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
              {source.revoked ? (
                <Badge variant="muted">
                  <Ban aria-hidden="true" />
                  Revoked
                </Badge>
              ) : (
                <>
                  <Badge variant="success">
                    <ShieldCheck aria-hidden="true" />
                    Accepting documents
                  </Badge>
                  <Button
                    type="button"
                    size="sm"
                    variant="destructive-outline"
                    disabled={disabled}
                    onClick={() => onRevoke(source)}
                  >
                    Revoke
                  </Button>
                </>
              )}
            </li>
          ))}
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
