import { useId, useState } from "react";
import { Copy, Network, TriangleAlert } from "lucide-react";

import type { components } from "@/api/generated/schema";
import { Button } from "@/components/ui/button";
import { hasIpMismatch } from "./device-network";

type Device = components["schemas"]["DeviceResponse"];

export function DeviceIpWarning({
  device,
  onView,
}: {
  device: Device;
  onView: () => void;
}) {
  const tooltipId = useId();
  if (!hasIpMismatch(device.network)) return null;
  return (
    <span className="group relative inline-flex">
      <Button
        type="button"
        variant="ghost"
        size="icon"
        className="size-7 text-amber-700 hover:bg-amber-100 hover:text-amber-800"
        aria-label={`IP mismatch: view details for ${device.machine_hardware_id}`}
        aria-describedby={tooltipId}
        onClick={onView}
      >
        <TriangleAlert aria-hidden="true" className="size-4" />
      </Button>
      <span
        id={tooltipId}
        role="tooltip"
        className="pointer-events-none absolute top-full right-0 z-20 mt-1 hidden w-48 rounded-md border bg-popover px-3 py-2 text-xs whitespace-normal text-popover-foreground shadow-md group-hover:block group-focus-within:block"
      >
        Client and Server IP differ. View details.
      </span>
    </span>
  );
}

export function DeviceNetworkDetails({ device }: { device: Device }) {
  const [message, setMessage] = useState("");
  const [copyFailed, setCopyFailed] = useState(false);
  const { network } = device;
  async function copy(label: string, address: string) {
    try {
      await navigator.clipboard.writeText(address);
      setCopyFailed(false);
      setMessage(`${label} copied.`);
    } catch {
      setCopyFailed(true);
      setMessage(
        "Could not copy the address. Select and copy the IP manually.",
      );
    }
  }
  return (
    <section
      aria-label="Network addresses"
      className="space-y-3 rounded-md border p-4"
    >
      <div className="flex flex-wrap items-center justify-between gap-2">
        <h3 className="flex items-center gap-2 font-medium">
          <Network
            aria-hidden="true"
            className="size-4 text-muted-foreground"
          />
          Network addresses
        </h3>
        {hasIpMismatch(network) && (
          <span className="flex items-center gap-1.5 text-sm text-amber-700">
            <TriangleAlert aria-hidden="true" className="size-4" />
            Client and Server IP differ
          </span>
        )}
      </div>
      {network ? (
        <>
          <div className="grid gap-3 sm:grid-cols-2">
            {(
              [
                ["Server IP", network.server_observed_ip],
                ["Client IP", network.client_ip],
              ] as const
            ).map(([label, address]) => (
              <div
                key={label}
                className="min-w-0 rounded-md bg-muted/30 px-3 py-2"
              >
                <p className="text-xs text-muted-foreground">
                  {label === "Server IP"
                    ? "Server-observed IP"
                    : "Client-reported IP"}
                </p>
                <div className="mt-1 flex items-center justify-between gap-2">
                  <code className="min-w-0 text-sm break-all">
                    {address ?? "Not reported"}
                  </code>
                  {address && (
                    <Button
                      type="button"
                      variant="ghost"
                      size="icon"
                      className="size-7"
                      aria-label={`Copy ${label}`}
                      onClick={() => void copy(label, address)}
                    >
                      <Copy aria-hidden="true" className="size-3.5" />
                    </Button>
                  )}
                </div>
              </div>
            ))}
          </div>
          <p className="text-xs text-muted-foreground">
            Recorded: {new Date(network.observed_at_unix_ms).toLocaleString()}
            {device.convergence.connection_state === "offline"
              ? " · Device offline; showing last recorded addresses."
              : " · Addresses from the last recorded connection."}
          </p>
        </>
      ) : (
        <p className="text-sm text-muted-foreground">
          No IP addresses recorded yet.
        </p>
      )}
      {message && (
        <p
          role={copyFailed ? "alert" : "status"}
          className={
            copyFailed
              ? "text-sm text-destructive"
              : "text-sm text-muted-foreground"
          }
        >
          {message}
        </p>
      )}
    </section>
  );
}
