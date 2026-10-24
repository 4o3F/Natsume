import { useState } from "react";
import { Check, Copy, Network, TriangleAlert } from "lucide-react";

import { usePageToast } from "@/components/use-page-toast";

import type { DeviceResponse_Serialize as DeviceResponse } from "@/api/generated/types";
import { Button } from "@/components/ui/button";
import {
  Tooltip,
  TooltipContent,
  TooltipTrigger,
} from "@/components/ui/tooltip";
import { hasIpMismatch } from "./device-network";

type Device = DeviceResponse;

export function DeviceIpWarning({
  device,
  onView,
}: {
  device: Device;
  onView: () => void;
}) {
  if (!hasIpMismatch(device.network)) return null;
  return (
    <Tooltip>
      <TooltipTrigger asChild>
        <Button
          type="button"
          variant="ghost"
          size="icon-xs"
          className="text-warning-strong hover:bg-warning-subtle hover:text-warning-strong"
          aria-label={`IP mismatch: view details for ${device.machine_hardware_id}`}
          onClick={onView}
        >
          <TriangleAlert aria-hidden="true" />
        </Button>
      </TooltipTrigger>
      <TooltipContent>
        Client and Server IP differ. View details.
      </TooltipContent>
    </Tooltip>
  );
}

export function DeviceNetworkDetails({ device }: { device: Device }) {
  const toast = usePageToast();
  const [copied, setCopied] = useState<string | null>(null);
  const { network } = device;
  async function copy(label: string, address: string) {
    try {
      await navigator.clipboard.writeText(address);
      setCopied(label);
      toast.success(`${label} copied`, { description: address });
      window.setTimeout(() => setCopied(null), 1500);
    } catch {
      toast.error("Could not copy the address", {
        description: "Select and copy the IP manually.",
      });
    }
  }
  return (
    <section aria-label="Network addresses" className="space-y-3">
      <div className="flex flex-wrap items-center justify-between gap-2">
        <p className="flex items-center gap-2 text-sm font-semibold">
          <Network
            aria-hidden="true"
            className="size-4 text-muted-foreground"
          />
          Network addresses
        </p>
        {hasIpMismatch(network) && (
          <span className="flex items-center gap-1.5 text-xs font-medium text-warning-strong">
            <TriangleAlert aria-hidden="true" className="size-3.5" />
            Client and Server IP differ
          </span>
        )}
      </div>
      {network ? (
        <>
          <div className="grid gap-2 sm:grid-cols-2">
            {(
              [
                ["Server IP", "Server-observed", network.server_observed_ip],
                ["Client IP", "Client-reported", network.client_ip],
              ] as const
            ).map(([label, caption, address]) => (
              <div
                key={label}
                className="flex min-w-0 items-center justify-between gap-2 rounded-lg border bg-card px-3 py-2"
              >
                <div className="min-w-0">
                  <p className="text-xs text-muted-foreground">{caption}</p>
                  <code
                    className={
                      address
                        ? "font-mono text-sm break-all"
                        : "text-sm text-muted-foreground"
                    }
                  >
                    {address ?? "Not reported"}
                  </code>
                </div>
                {address && (
                  <Button
                    type="button"
                    variant="ghost"
                    size="icon-sm"
                    aria-label={`Copy ${label}`}
                    onClick={() => void copy(label, address)}
                  >
                    {copied === label ? (
                      <Check
                        aria-hidden="true"
                        className="text-success-strong"
                      />
                    ) : (
                      <Copy aria-hidden="true" />
                    )}
                  </Button>
                )}
              </div>
            ))}
          </div>
          <p className="text-xs text-muted-foreground">
            Recorded {new Date(network.observed_at_unix_ms).toLocaleString()}
            {device.convergence.connection_state === "offline"
              ? " · Device offline; showing last recorded addresses."
              : " · Addresses from the last recorded connection."}
          </p>
        </>
      ) : (
        <p className="rounded-lg border border-dashed px-3 py-4 text-center text-sm text-muted-foreground">
          No IP addresses recorded yet.
        </p>
      )}
    </section>
  );
}
