import { useState, type ReactNode } from "react";
import { useMutation, useQuery } from "@tanstack/react-query";
import { ChevronDown, Download, LifeBuoy } from "lucide-react";

import { useSessionScope } from "@/auth/session-context";
import { unwrap } from "@/api/errors";
import type { components } from "@/api/generated/schema";
import { LIST_POLL_MS } from "@/api/polling";
import { Button } from "@/components/ui/button";
import {
  DeviceStateFilter,
  type DeviceStateFilterValue,
} from "./device-state-filter";
import { deviceIpExport } from "./device-network";

type Device = components["schemas"]["DeviceResponse"];

export function DeviceFilterPanel({
  value,
  onChange,
  children,
}: {
  value: DeviceStateFilterValue;
  onChange: (value: DeviceStateFilterValue) => void;
  children: ReactNode;
}) {
  const { api } = useSessionScope();
  const [expanded, setExpanded] = useState(false);
  async function readAllDevices(signal?: AbortSignal) {
    return unwrap<Device[]>(
      await api.GET("/api/v2/devices", {
        signal,
        params: { query: { state: ["enabled", "disabled"] } },
      }),
    );
  }
  const devices = useQuery({
    queryKey: ["devices", ["enabled", "disabled"]],
    queryFn: ({ signal }) => readAllDevices(signal),
    enabled: expanded,
    refetchInterval: expanded ? LIST_POLL_MS : false,
  });
  const download = useMutation({
    mutationFn: async () => {
      const report = deviceIpExport(await readAllDevices());
      if (report.addresses.length) {
        const blob = new Blob([`${report.addresses.join("\n")}\n`], {
          type: "text/plain;charset=utf-8",
        });
        const url = URL.createObjectURL(blob);
        const link = document.createElement("a");
        link.href = url;
        link.download = `natsume-device-ips-${new Date().toISOString().replace(/[:.]/g, "-")}.txt`;
        link.click();
        window.setTimeout(() => URL.revokeObjectURL(url), 1000);
      }
      return report;
    },
    gcTime: 0,
  });
  const report = devices.data ? deviceIpExport(devices.data) : null;
  return (
    <section
      aria-label="Device filters"
      className="overflow-hidden rounded-xl border bg-card shadow-sm"
    >
      <div className="flex flex-wrap items-center justify-between gap-3 p-4">
        <DeviceStateFilter value={value} onChange={onChange} />
        {children}
      </div>
      <details
        className="group border-t"
        onToggle={(event) => setExpanded(event.currentTarget.open)}
      >
        <summary className="flex cursor-pointer list-none items-center justify-between gap-3 px-4 py-3 text-sm font-medium hover:bg-muted/40 focus-visible:outline-2 focus-visible:outline-ring [&::-webkit-details-marker]:hidden">
          <span className="flex items-center gap-2">
            <LifeBuoy aria-hidden="true" className="size-4 text-amber-700" />
            Emergency
          </span>
          <ChevronDown
            aria-hidden="true"
            className="size-4 text-muted-foreground transition-transform group-open:rotate-180"
          />
        </summary>
        <div
          role="region"
          aria-label="Emergency IP export"
          className="space-y-3 border-t bg-muted/20 p-4"
        >
          <p className="text-sm text-muted-foreground">
            Export Server-observed IPs for all enabled and disabled devices,
            including offline devices. Search and list filters do not limit this
            export. Revoked devices are excluded.
          </p>
          {devices.isLoading && (
            <p role="status" className="text-sm text-muted-foreground">
              Loading device addresses…
            </p>
          )}
          {devices.isError && (
            <p role="alert" className="text-sm text-destructive">
              Could not refresh device addresses. Export will retry the full
              device list.
            </p>
          )}
          {report && (
            <p className="text-sm tabular-nums">
              Unique IPs: {report.addresses.length} · Offline: {report.offline}{" "}
              · IP mismatches: {report.mismatched} · Missing IPs:{" "}
              {report.missing}
            </p>
          )}
          <Button
            type="button"
            variant="outline"
            disabled={download.isPending}
            onClick={() => download.mutate()}
          >
            <Download aria-hidden="true" />
            {download.isPending ? "Exporting…" : "Export all device IPs"}
          </Button>
          {download.isError && (
            <p role="alert" className="text-sm text-destructive">
              IP export failed. No file was downloaded. Try again.
            </p>
          )}
          {download.isSuccess && (
            <p role="status" className="text-sm text-muted-foreground">
              {download.data.addresses.length
                ? `Exported ${download.data.addresses.length} unique IPs. Offline: ${download.data.offline} · IP mismatches: ${download.data.mismatched} · Missing IPs: ${download.data.missing}`
                : "No IP addresses available to export."}
            </p>
          )}
        </div>
      </details>
    </section>
  );
}
