import { useState } from "react";
import { useMutation, useQuery } from "@tanstack/react-query";
import { ChevronDown, Download, LifeBuoy } from "lucide-react";

import { useSessionScope } from "@/auth/session-context";
import { unwrap } from "@/api/errors";
import type { DeviceResponse_Serialize as DeviceResponse } from "@/api/generated/types";
import { LIST_POLL_MS } from "@/api/polling";
import { Button } from "@/components/ui/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { deviceIpExport } from "./device-network";

type Device = DeviceResponse;

export function DeviceEmergencyPanel() {
  const { api } = useSessionScope();
  const [expanded, setExpanded] = useState(false);
  async function readAllDevices(signal?: AbortSignal) {
    return unwrap<Device[]>(
      await api.listDevices(["enabled", "disabled"], { signal }),
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
    <DropdownMenu open={expanded} onOpenChange={setExpanded} modal={false}>
      <DropdownMenuTrigger asChild>
        <Button type="button" variant="outline" size="sm">
          <LifeBuoy aria-hidden="true" className="text-warning-strong" />
          Emergency
          <ChevronDown aria-hidden="true" />
        </Button>
      </DropdownMenuTrigger>
      <DropdownMenuContent
        align="end"
        collisionPadding={16}
        aria-label="Emergency tools"
        aria-labelledby={undefined}
        className="max-h-[var(--radix-dropdown-menu-content-available-height)] w-80 overflow-y-auto p-4"
      >
        <div
          role="region"
          aria-label="Emergency IP export"
          className="space-y-3"
        >
          <DropdownMenuLabel className="p-0 text-sm font-semibold text-foreground">
            Emergency IP export
          </DropdownMenuLabel>
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
            <dl className="grid grid-cols-4 gap-2 text-center">
              {(
                [
                  ["Unique IPs", report.addresses.length],
                  ["Offline", report.offline],
                  ["Mismatch", report.mismatched],
                  ["Missing", report.missing],
                ] as const
              ).map(([label, value]) => (
                <div
                  key={label}
                  className="flex flex-col-reverse rounded-md bg-muted px-1 py-1.5"
                >
                  <dt className="text-xs text-muted-foreground">{label}</dt>
                  <dd className="text-base font-semibold tabular-nums">
                    {value}
                  </dd>
                </div>
              ))}
            </dl>
          )}
          <DropdownMenuItem
            asChild
            disabled={download.isPending}
            onSelect={(event) => {
              event.preventDefault();
              download.mutate();
            }}
          >
            <Button
              type="button"
              className="w-full"
              disabled={download.isPending}
            >
              <Download aria-hidden="true" />
              {download.isPending ? "Exporting…" : "Export all device IPs"}
            </Button>
          </DropdownMenuItem>
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
      </DropdownMenuContent>
    </DropdownMenu>
  );
}
