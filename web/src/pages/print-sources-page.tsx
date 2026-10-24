import { useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { unwrap } from "@/api/errors";
import type { PrintSource, PrintSourceIssued } from "@/api/generated/types";
import { LIST_POLL_MS } from "@/api/polling";
import { useSessionScope } from "@/auth/session-context";
import { DataState } from "@/components/data-state";
import { PrintNavigation } from "@/components/print-navigation";
import { PrintSecret } from "@/components/print-secret";
import { Alert, AlertTitle } from "@/components/ui/alert";
import {
  AlertDialog,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "@/components/ui/alert-dialog";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from "@/components/ui/table";
import { useOperatorAction } from "@/operator/use-operator-action";
import { printManagementError, validPrintName } from "@/pages/print-management";

export function PrintSourcesPage() {
  const { api, queryClient } = useSessionScope();
  const action = useOperatorAction(printManagementError);
  const [name, setName] = useState("");
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
  return (
    <div className="min-w-0 space-y-5">
      <PrintNavigation />
      <h1 className="text-2xl font-semibold">Print sources</h1>
      <p className="text-sm text-muted-foreground">
        Manage dedicated DOMjudge source credentials. These do not authorize
        Windows client execution or administrator operations.
      </p>
      {action.error && (
        <Alert variant="destructive">
          <AlertTitle>{action.error}</AlertTitle>
        </Alert>
      )}
      {issued && (
        <PrintSecret
          key={issued.source_id}
          title={`New source token for ${issued.name} — shown once`}
          value={issued.token}
          description={`Source ID: ${issued.source_id}. Save this token in the deployment-owned restricted token file. It cannot be recovered from the list. Dismissing this value or leaving the page removes it from this view. Copying uses the system clipboard, which is not cleared by this page.`}
          onDismiss={() => setIssued(null)}
        />
      )}
      <form
        className="flex flex-wrap items-end gap-3"
        onSubmit={(event) => {
          event.preventDefault();
          setIssued(null);
          if (!validPrintName(name)) return;
          void action.run(
            async (signal) =>
              unwrap(await api.createPrintSource({ name }, { signal })),
            (source) => {
              setIssued(source);
              setName("");
              refresh();
            },
          );
        }}
      >
        <div className="space-y-2">
          <Label htmlFor="source-name">Source name</Label>
          <Input
            id="source-name"
            value={name}
            onChange={(event) => setName(event.target.value)}
            disabled={action.pending}
          />
        </div>
        <Button disabled={action.pending || !validPrintName(name)}>
          Create source
        </Button>
        <Button type="button" variant="outline" onClick={refresh}>
          Refresh sources
        </Button>
      </form>
      {sources.isError && sources.data && (
        <Alert variant="destructive">
          <AlertTitle>
            Source refresh failed. Showing potentially outdated source data.
          </AlertTitle>
        </Alert>
      )}
      <DataState
        isLoading={sources.isLoading}
        error={sources.data ? null : sources.error}
        isEmpty={!sources.data?.length}
        emptyLabel="No print sources."
      >
        <Table>
          <TableHeader>
            <TableRow>
              <TableHead>Name</TableHead>
              <TableHead>Source ID</TableHead>
              <TableHead>Status</TableHead>
              <TableHead>Actions</TableHead>
            </TableRow>
          </TableHeader>
          <TableBody>
            {sources.data?.map((source) => (
              <TableRow key={source.source_id}>
                <TableCell className="max-w-xs whitespace-normal break-all">
                  {source.name}
                </TableCell>
                <TableCell className="max-w-xs break-all font-mono text-xs">
                  {source.source_id}
                </TableCell>
                <TableCell>{source.revoked ? "Revoked" : "Enabled"}</TableCell>
                <TableCell>
                  <Button
                    variant="outline"
                    disabled={action.pending || source.revoked}
                    onClick={() => {
                      setIssued(null);
                      setConfirmation(source);
                    }}
                  >
                    Revoke source
                  </Button>
                </TableCell>
              </TableRow>
            ))}
          </TableBody>
        </Table>
      </DataState>
      <AlertDialog
        open={confirmation !== null}
        onOpenChange={(open) => {
          if (!open && !action.pending) setConfirmation(null);
        }}
      >
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>Revoke print source?</AlertDialogTitle>
            <AlertDialogDescription>
              This stops future authentication for {confirmation?.name}.
              Existing requests, admitted documents and execution history remain
              unchanged; this does not cancel printing. This credential cannot
              be restored.
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
              disabled={action.pending}
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
                    setConfirmation(null);
                    refresh();
                  },
                );
              }}
            >
              Confirm revoke source
            </Button>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </div>
  );
}
