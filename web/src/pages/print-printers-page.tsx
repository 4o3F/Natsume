import { useState } from "react";
import { useQuery } from "@tanstack/react-query";
import {
  Check,
  Info,
  KeyRound,
  Plus,
  Printer,
  RotateCw,
  Unlink,
} from "lucide-react";

import { usePageToast } from "@/components/use-page-toast";

import { unwrap } from "@/api/errors";
import type { PrintPairingCode, PrintPrinter } from "@/api/generated/types";
import { LIST_POLL_MS } from "@/api/polling";
import { useSessionScope } from "@/auth/session-context";
import { CopyButton } from "@/components/copy-button";
import { DataState } from "@/components/data-state";
import { RefreshCountdown, StatusDot } from "@/components/device-status";
import { tones, type Tone } from "@/components/device-status-style";
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
import { useNow } from "@/lib/use-now";
import { cn } from "@/lib/utils";
import { useOperatorAction } from "@/operator/use-operator-action";
import { printManagementError } from "@/pages/print-management";
import { countdown } from "@/pages/print-status";

type PairingStatus = "paired" | "confirming" | "code" | "expired" | "unpaired";

const statusMeta: Record<PairingStatus, { label: string; tone: Tone }> = {
  paired: { label: "Paired", tone: "success" },
  confirming: { label: "Awaiting client confirmation", tone: "warning" },
  code: { label: "Pairing code issued", tone: "info" },
  expired: { label: "Pairing code expired", tone: "muted" },
  unpaired: { label: "Not paired", tone: "muted" },
};

function pairingStatus(printer: PrintPrinter, now: number): PairingStatus {
  if (printer.client_id)
    return printer.client_confirmed ? "paired" : "confirming";
  if (printer.pairing_expires_at_unix_ms === null) return "unpaired";
  return printer.pairing_expires_at_unix_ms > now ? "code" : "expired";
}

export function PrintPrintersPage() {
  const toast = usePageToast();
  const { api, queryClient } = useSessionScope();
  const action = useOperatorAction(printManagementError);
  const [registerOpen, setRegisterOpen] = useState(false);
  const [issued, setIssued] = useState<PrintPairingCode | null>(null);
  const [confirmation, setConfirmation] = useState<PrintPrinter | null>(null);
  const now = useNow(1_000);
  const printers = useQuery({
    queryKey: ["print-printers"],
    queryFn: async ({ signal }) =>
      unwrap(await api.listPrintPrinters({ signal })),
    refetchInterval: LIST_POLL_MS,
  });
  const refresh = () => {
    void queryClient.invalidateQueries({ queryKey: ["print-printers"] });
  };
  const issueCode = (printer: PrintPrinter) => {
    setIssued(null);
    void action.run(
      async (signal) =>
        unwrap(await api.issuePrintPairingCode(printer.printer_id, { signal })),
      (code) => {
        setIssued(code);
        refresh();
      },
    );
  };
  const list = printers.data ?? [];
  const counts = list.reduce<Record<PairingStatus, number>>(
    (total, printer) => {
      total[pairingStatus(printer, now)] += 1;
      return total;
    },
    { paired: 0, confirming: 0, code: 0, expired: 0, unpaired: 0 },
  );
  const issuedPrinter = list.find(
    (printer) => printer.printer_id === issued?.printer_id,
  );

  return (
    <div className="min-w-0 space-y-6">
      <PageHeader
        title="Printers"
        description="Register each physical printer once, then pair it with the Windows client that drives its queue."
        actions={
          <>
            <RefreshCountdown
              resource="printer"
              updatedAt={Math.max(
                printers.dataUpdatedAt,
                printers.errorUpdatedAt,
              )}
              isFetching={printers.isFetching}
              isPaused={printers.isPaused}
              isError={printers.isError}
              onRefresh={refresh}
            />
            <Button type="button" onClick={() => setRegisterOpen(true)}>
              <Plus aria-hidden="true" />
              Register printer
            </Button>
          </>
        }
      />

      {action.error && !registerOpen && !confirmation && (
        <Alert variant="destructive">
          <AlertTitle>{action.error}</AlertTitle>
        </Alert>
      )}
      {issued && (
        <PrintSecret
          key={issued.code}
          kind="code"
          title={`Pairing code${issuedPrinter ? ` for ${issuedPrinter.name}` : ""}`}
          value={issued.code}
          expires={issued.expires_at_unix_ms}
          description={
            <>
              <p>Enter this code in the Windows client.</p>
              <p>
                Issuing another code replaces this one. It can’t be shown again
                after you dismiss it.
              </p>
            </>
          }
          meta={
            <span className="font-mono">Printer ID {issued.printer_id}</span>
          }
          onDismiss={() => setIssued(null)}
        />
      )}
      {printers.isError && printers.data && (
        <Alert variant="destructive">
          <AlertTitle>
            Printer refresh failed. Showing potentially outdated registration
            data.
          </AlertTitle>
        </Alert>
      )}

      <section aria-label="Registered printers" className="space-y-3">
        {printers.data && list.length > 0 && (
          <div className="flex flex-wrap items-center gap-x-4 gap-y-1 text-xs text-muted-foreground">
            {(["paired", "confirming", "code", "unpaired"] as const).map(
              (key) =>
                (counts[key] > 0 || key === "paired") && (
                  <span key={key} className="flex items-center gap-1.5">
                    <StatusDot tone={statusMeta[key].tone} />
                    <span className="tabular-nums">
                      {key === "unpaired"
                        ? counts.unpaired + counts.expired
                        : counts[key]}
                    </span>{" "}
                    {key === "code"
                      ? "code issued"
                      : statusMeta[key].label.toLowerCase()}
                  </span>
                ),
            )}
          </div>
        )}
        <DataState
          isLoading={printers.isLoading}
          error={printers.data ? null : printers.error}
          isEmpty={!list.length}
          emptyLabel="No printers registered"
          emptyDescription="Register a printer to issue its first pairing code."
        >
          <div className="grid grid-cols-1 gap-4 sm:grid-cols-2 xl:grid-cols-3">
            {list.map((printer) => (
              <PrinterCard
                key={printer.printer_id}
                printer={printer}
                status={pairingStatus(printer, now)}
                now={now}
                disabled={action.pending}
                onIssue={() => issueCode(printer)}
                onClear={() => {
                  setIssued(null);
                  setConfirmation(printer);
                }}
              />
            ))}
            <Button
              type="button"
              variant="ghost"
              className="h-auto min-h-48 flex-col gap-2 border-dashed border-border-strong"
              onClick={() => setRegisterOpen(true)}
            >
              <span className="flex size-10 items-center justify-center bg-muted">
                <Plus aria-hidden="true" />
              </span>
              Register another printer
            </Button>
          </div>
        </DataState>
        <p className="flex items-start gap-2 text-xs text-muted-foreground">
          <Info aria-hidden="true" className="mt-0.5 size-3.5 shrink-0" />
          Pairing shows registration only. It doesn’t tell you whether the
          client is online or the printer is ready. Check each client’s Windows
          queue mapping.
        </p>
      </section>

      <PrintNameDialog
        id="printer-name"
        open={registerOpen}
        onOpenChange={setRegisterOpen}
        title="Register printer"
        description="Register each physical printer only once. You’ll issue a pairing code for its Windows client next."
        label="Printer name"
        placeholder="Hall A · HP M404"
        hint="Up to 128 characters. Include the hall or room so staff can find it."
        submitLabel="Register printer"
        pending={action.pending}
        error={action.error}
        onSubmit={(name) => {
          setIssued(null);
          void action.run(
            async (signal) =>
              unwrap(await api.createPrintPrinter({ name }, { signal })),
            (printer) => {
              setRegisterOpen(false);
              toast.success("Printer registered", {
                description: `Issue a pairing code for ${printer.name} to connect its client.`,
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
          <AlertDialogHeader>
            <AlertDialogMedia>
              <Unlink aria-hidden="true" />
            </AlertDialogMedia>
            <AlertDialogTitle>
              Clear binding for {confirmation?.name}?
            </AlertDialogTitle>
            <AlertDialogDescription>
              This revokes the old client credential and any unused pairing
              code. Active attempts prevent this operation. Task history and
              attempt counts are not reset.
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
              type="button"
              variant="destructive"
              loading={action.pending}
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
                    toast.success("Binding cleared", {
                      description: confirmation.name,
                    });
                    setConfirmation(null);
                    refresh();
                  },
                );
              }}
            >
              Clear binding
            </Button>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </div>
  );
}

function PrinterCard({
  printer,
  status,
  now,
  disabled,
  onIssue,
  onClear,
}: {
  printer: PrintPrinter;
  status: PairingStatus;
  now: number;
  disabled: boolean;
  onIssue: () => void;
  onClear: () => void;
}) {
  const meta = statusMeta[status];
  const canClear =
    printer.client_id !== null || printer.pairing_expires_at_unix_ms !== null;
  const left =
    status === "code" && printer.pairing_expires_at_unix_ms !== null
      ? countdown(printer.pairing_expires_at_unix_ms, now)
      : null;
  const steps: { label: string; state: "done" | "current" | "todo" }[] = [
    { label: "Registered", state: "done" },
    {
      label: "Code issued",
      state: printer.client_id
        ? "done"
        : status === "code"
          ? "current"
          : "todo",
    },
    {
      label: "Client confirmed",
      state:
        status === "paired"
          ? "done"
          : status === "confirming"
            ? "current"
            : "todo",
    },
  ];
  return (
    <article aria-label={printer.name} className="flex flex-col border bg-card">
      <div className="flex items-start gap-3 p-4 pb-3">
        <span
          className={cn(
            "flex size-10 shrink-0 items-center justify-center rounded-lg",
            status === "paired"
              ? "bg-success-subtle text-success-strong"
              : status === "confirming"
                ? "bg-warning-subtle text-warning-strong"
                : status === "code"
                  ? "bg-info-subtle text-info"
                  : "bg-muted text-muted-foreground",
          )}
        >
          <Printer aria-hidden="true" className="size-5" />
        </span>
        <div className="min-w-0 flex-1 space-y-1">
          <p className="leading-tight font-semibold break-words">
            {printer.name}
          </p>
          <Badge variant={tones[meta.tone].badge} dot>
            {meta.label}
            {left && <span className="tabular-nums">· {left}</span>}
          </Badge>
        </div>
      </div>

      <ol
        aria-label="Pairing progress"
        className="flex items-center gap-1.5 px-4 py-2"
      >
        {steps.map((step, index) => (
          <li
            key={step.label}
            className="flex min-w-0 flex-1 flex-col gap-1.5"
            aria-current={step.state === "current" ? "step" : undefined}
          >
            <span
              className={cn(
                "h-1",
                step.state === "done"
                  ? "bg-success"
                  : step.state === "current"
                    ? "bg-info motion-safe:animate-pulse"
                    : "bg-muted",
              )}
            />
            <span
              className={cn(
                "flex items-center gap-1 truncate text-caption",
                step.state === "todo"
                  ? "text-muted-foreground"
                  : "text-foreground",
              )}
            >
              {step.state === "done" && (
                <Check
                  aria-hidden="true"
                  className="size-3 text-success-strong"
                />
              )}
              <span className="sr-only">Step {index + 1}: </span>
              {step.label}
            </span>
          </li>
        ))}
      </ol>

      <dl className="mx-4 my-2 space-y-1 bg-muted px-3 py-2 text-xs">
        <IdRow label="Printer" value={printer.printer_id} />
        <IdRow label="Client" value={printer.client_id} />
      </dl>

      <div className="mt-auto flex items-center gap-2 border-t px-4 py-3">
        {printer.client_id === null && (
          <Button
            type="button"
            size="sm"
            variant={status === "code" ? "outline" : "default"}
            disabled={disabled}
            onClick={onIssue}
          >
            {status === "code" ? (
              <RotateCw aria-hidden="true" />
            ) : (
              <KeyRound aria-hidden="true" />
            )}
            {status === "code" ? "Replace code" : "Issue pairing code"}
          </Button>
        )}
        {canClear && (
          <Button
            type="button"
            size="sm"
            variant="ghost"
            className="ml-auto text-destructive hover:bg-destructive-subtle hover:text-destructive"
            disabled={disabled}
            onClick={onClear}
          >
            <Unlink aria-hidden="true" />
            Clear binding
          </Button>
        )}
      </div>
    </article>
  );
}

function IdRow({ label, value }: { label: string; value: string | null }) {
  return (
    <div className="flex items-center gap-2">
      <dt className="w-12 shrink-0 text-muted-foreground">{label}</dt>
      <dd className="flex min-w-0 flex-1 items-center gap-1">
        {value ? (
          <>
            <code className="truncate font-mono" title={value}>
              {value}
            </code>
            <CopyButton
              value={value}
              label={`Copy ${label.toLowerCase()} ID`}
              size="icon-xs"
              className="size-6"
            />
          </>
        ) : (
          <span className="text-muted-foreground">None</span>
        )}
      </dd>
    </div>
  );
}
