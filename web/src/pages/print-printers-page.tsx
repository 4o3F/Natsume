import { useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { unwrap } from "@/api/errors";
import type { PrintPairingCode, PrintPrinter } from "@/api/generated/types";
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

export function PrintPrintersPage() {
  const { api, queryClient } = useSessionScope();
  const action = useOperatorAction(printManagementError);
  const [name, setName] = useState("");
  const [issued, setIssued] = useState<PrintPairingCode | null>(null);
  const [confirmation, setConfirmation] = useState<PrintPrinter | null>(null);
  const printers = useQuery({
    queryKey: ["print-printers"],
    queryFn: async ({ signal }) =>
      unwrap(await api.listPrintPrinters({ signal })),
    refetchInterval: LIST_POLL_MS,
  });
  const refresh = () => {
    void queryClient.invalidateQueries({ queryKey: ["print-printers"] });
  };
  return (
    <div className="min-w-0 space-y-5">
      <PrintNavigation />
      <h1 className="text-2xl font-semibold">Printers and pairing</h1>
      <p className="text-sm text-muted-foreground">
        Registration and binding facts are not evidence that a client is online
        or a physical printer is ready. Register each physical device only once;
        verify its Windows queue mapping.
      </p>
      {action.error && (
        <Alert variant="destructive">
          <AlertTitle>{action.error}</AlertTitle>
        </Alert>
      )}
      {issued && (
        <PrintSecret
          key={issued.code}
          title="New pairing code"
          value={issued.code}
          expires={issued.expires_at_unix_ms}
          description={`Printer ID: ${issued.printer_id}. Enter this code in the Windows client. Issuing another code replaces the unused old code. This page cannot recover it after dismissal.`}
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
              unwrap(await api.createPrintPrinter({ name }, { signal })),
            () => {
              setName("");
              refresh();
            },
          );
        }}
      >
        <div className="space-y-2">
          <Label htmlFor="printer-name">Printer name</Label>
          <Input
            id="printer-name"
            value={name}
            onChange={(event) => setName(event.target.value)}
            disabled={action.pending}
          />
        </div>
        <Button disabled={action.pending || !validPrintName(name)}>
          Register printer
        </Button>
        <Button type="button" variant="outline" onClick={refresh}>
          Refresh printers
        </Button>
      </form>
      {printers.isError && printers.data && (
        <Alert variant="destructive">
          <AlertTitle>
            Printer refresh failed. Showing potentially outdated registration
            data.
          </AlertTitle>
        </Alert>
      )}
      <DataState
        isLoading={printers.isLoading}
        error={printers.data ? null : printers.error}
        isEmpty={!printers.data?.length}
        emptyLabel="No printers registered."
      >
        <Table>
          <TableHeader>
            <TableRow>
              <TableHead>Name</TableHead>
              <TableHead>Binding</TableHead>
              <TableHead>Identifiers</TableHead>
              <TableHead>Actions</TableHead>
            </TableRow>
          </TableHeader>
          <TableBody>
            {printers.data?.map((printer) => (
              <TableRow key={printer.printer_id}>
                <TableCell className="max-w-xs whitespace-normal break-all">
                  {printer.name}
                </TableCell>
                <TableCell>
                  {printer.client_id
                    ? printer.client_confirmed
                      ? "Binding confirmed"
                      : "Awaiting client confirmation"
                    : "Not bound"}
                </TableCell>
                <TableCell className="max-w-xs break-all text-xs">
                  <p>Printer: {printer.printer_id}</p>
                  <p>Client: {printer.client_id ?? "None"}</p>
                  {printer.pairing_expires_at_unix_ms !== null && (
                    <p>
                      Code expiry:{" "}
                      {new Date(
                        printer.pairing_expires_at_unix_ms,
                      ).toLocaleString()}
                    </p>
                  )}
                </TableCell>
                <TableCell>
                  <div className="flex flex-wrap gap-2">
                    <Button
                      variant="outline"
                      disabled={action.pending || printer.client_id !== null}
                      onClick={() => {
                        setIssued(null);
                        void action.run(
                          async (signal) =>
                            unwrap(
                              await api.issuePrintPairingCode(
                                printer.printer_id,
                                { signal },
                              ),
                            ),
                          (code) => {
                            setIssued(code);
                            refresh();
                          },
                        );
                      }}
                    >
                      Issue pairing code
                    </Button>
                    <Button
                      variant="outline"
                      disabled={
                        action.pending ||
                        (printer.client_id === null &&
                          printer.pairing_expires_at_unix_ms === null)
                      }
                      onClick={() => {
                        setIssued(null);
                        setConfirmation(printer);
                      }}
                    >
                      Clear binding
                    </Button>
                  </div>
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
            <AlertDialogTitle>Clear printer binding?</AlertDialogTitle>
            <AlertDialogDescription>
              This revokes the old client credential and unused pairing code for{" "}
              {confirmation?.name}. Active attempts prevent this operation. Task
              history and attempt counts are not reset.
            </AlertDialogDescription>
          </AlertDialogHeader>
          {action.error && (
            <Alert variant="destructive">
              <AlertTitle>{action.error}</AlertTitle>
            </Alert>
          )}
          <AlertDialogFooter>
            <AlertDialogCancel disabled={action.pending}>
              Keep binding
            </AlertDialogCancel>
            <Button
              disabled={action.pending}
              onClick={() => {
                if (!confirmation) return;
                void action.run(
                  async (signal) =>
                    unwrap(
                      await api.clearPrintPrinterBinding(
                        confirmation.printer_id,
                        { signal },
                      ),
                    ),
                  () => {
                    setConfirmation(null);
                    refresh();
                  },
                );
              }}
            >
              Confirm clear binding
            </Button>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </div>
  );
}
