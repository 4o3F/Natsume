import { useRef, useState } from "react";
import { Ban, CirclePause, CirclePlay, Ellipsis, Unlink } from "lucide-react";

import type { DeviceResponse_Serialize as Device } from "@/api/generated/types";
import { ConfirmDialog } from "@/components/confirm-dialog";
import { Button } from "@/components/ui/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";

export interface DeviceActionHandlers {
  isUnbinding: boolean;
  unbindDevice: (deviceId: string) => void;
  isUpdatingLifecycle: boolean;
  updateLifecycle: (change: {
    deviceId: string;
    state: Device["state"];
  }) => void;
}

type Pending = "unbind" | "revoke" | null;

/**
 * Admin actions for one device, rendered as a row overflow menu or as a
 * button row (detail sheet). Unbind and revoke always confirm first.
 */
export function DeviceActions({
  device,
  layout = "menu",
  isUnbinding,
  unbindDevice,
  isUpdatingLifecycle,
  updateLifecycle,
}: DeviceActionHandlers & {
  device: Device;
  layout?: "menu" | "buttons";
}) {
  const [pending, setPending] = useState<Pending>(null);
  const actionsTrigger = useRef<HTMLButtonElement>(null);
  const binding = device.convergence.binding.target;
  const seat = binding?.state === "bound" ? binding.context.seat_code : null;
  const canUnbind = !isUnbinding && seat !== null;
  const canChangeLifecycle = !isUpdatingLifecycle && device.state !== "revoked";
  const nextState = device.state === "enabled" ? "disabled" : "enabled";
  const toggleLifecycle = () =>
    updateLifecycle({ deviceId: device.device_id, state: nextState });

  const dialogs = (
    <>
      <ConfirmDialog
        open={pending === "unbind"}
        returnFocusRef={actionsTrigger}
        onOpenChange={(open) => !open && setPending(null)}
        icon={<Unlink aria-hidden="true" />}
        tone="warning"
        title={seat ? `Unbind seat ${seat}?` : "Unbind this device?"}
        description={
          <>
            Device{" "}
            <code className="font-mono break-all">
              {device.machine_hardware_id}
            </code>{" "}
            (<code className="font-mono break-all">{device.device_id}</code>)
            will return to seat selection when it is enabled, connected, and
            ready for binding.
          </>
        }
        confirmLabel="Unbind"
        disabled={!canUnbind}
        onConfirm={() => unbindDevice(device.device_id)}
      />
      <ConfirmDialog
        open={pending === "revoke"}
        returnFocusRef={actionsTrigger}
        onOpenChange={(open) => !open && setPending(null)}
        icon={<Ban aria-hidden="true" />}
        title="Permanently revoke device?"
        description={
          <>
            Revocation is terminal and immediately evicts the current connection
            of{" "}
            <code className="font-mono break-all">
              {device.machine_hardware_id}
            </code>{" "}
            (<code className="font-mono break-all">{device.device_id}</code>).
            The device must re-enroll to be used again.
          </>
        }
        confirmLabel="Revoke device"
        disabled={!canChangeLifecycle}
        onConfirm={() =>
          updateLifecycle({ deviceId: device.device_id, state: "revoked" })
        }
      />
    </>
  );

  if (layout === "buttons") {
    return (
      <div className="flex flex-wrap gap-2">
        <Button
          type="button"
          variant="outline"
          size="sm"
          disabled={!canUnbind}
          onClick={() => setPending("unbind")}
        >
          <Unlink aria-hidden="true" />
          Unbind
        </Button>
        <Button
          type="button"
          variant="outline"
          size="sm"
          disabled={!canChangeLifecycle}
          loading={isUpdatingLifecycle}
          onClick={toggleLifecycle}
        >
          {device.state === "enabled" ? (
            <CirclePause aria-hidden="true" />
          ) : (
            <CirclePlay aria-hidden="true" />
          )}
          {device.state === "enabled" ? "Disable" : "Enable"}
        </Button>
        <Button
          type="button"
          variant="destructive-outline"
          size="sm"
          className="ml-auto"
          disabled={!canChangeLifecycle}
          onClick={() => setPending("revoke")}
        >
          <Ban aria-hidden="true" />
          Revoke
        </Button>
        {dialogs}
      </div>
    );
  }

  return (
    <>
      <DropdownMenu modal={false}>
        <DropdownMenuTrigger asChild>
          <Button
            type="button"
            variant="ghost"
            size="icon-sm"
            ref={actionsTrigger}
            aria-label={`Actions for ${seat ?? device.machine_hardware_id}`}
          >
            <Ellipsis aria-hidden="true" />
          </Button>
        </DropdownMenuTrigger>
        <DropdownMenuContent align="end" className="w-48">
          <DropdownMenuLabel className="truncate font-mono">
            {seat ?? device.machine_hardware_id.slice(0, 12)}
          </DropdownMenuLabel>
          <DropdownMenuItem
            disabled={!canChangeLifecycle}
            onSelect={toggleLifecycle}
          >
            {device.state === "enabled" ? (
              <CirclePause aria-hidden="true" />
            ) : (
              <CirclePlay aria-hidden="true" />
            )}
            {device.state === "enabled" ? "Disable" : "Enable"}
          </DropdownMenuItem>
          <DropdownMenuItem
            disabled={!canUnbind}
            onSelect={() => setPending("unbind")}
          >
            <Unlink aria-hidden="true" />
            Unbind seat
          </DropdownMenuItem>
          <DropdownMenuSeparator />
          <DropdownMenuItem
            variant="destructive"
            disabled={!canChangeLifecycle}
            onSelect={() => setPending("revoke")}
          >
            <Ban aria-hidden="true" />
            Revoke…
          </DropdownMenuItem>
        </DropdownMenuContent>
      </DropdownMenu>
      {dialogs}
    </>
  );
}
