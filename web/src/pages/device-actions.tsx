import type { DeviceResponse_Serialize as Device } from "@/api/generated/types";
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
  AlertDialogTrigger,
} from "@/components/ui/alert-dialog";
import { Button } from "@/components/ui/button";

export function DeviceActions({
  device,
  isUnbinding,
  unbindDevice,
  isUpdatingLifecycle,
  updateLifecycle,
}: {
  device: Device;
  isUnbinding: boolean;
  unbindDevice: (deviceId: string) => void;
  isUpdatingLifecycle: boolean;
  updateLifecycle: (change: {
    deviceId: string;
    state: Device["state"];
  }) => void;
}) {
  const binding = device.convergence.binding.target;
  return (
    <div className="flex gap-2">
      <AlertDialog>
        <AlertDialogTrigger asChild>
          <Button
            type="button"
            variant="outline"
            size="sm"
            className="w-20"
            disabled={isUnbinding || binding?.state !== "bound"}
          >
            Unbind
          </Button>
        </AlertDialogTrigger>
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>
              {binding?.state === "bound"
                ? `Unbind seat ${binding.context.seat_code}?`
                : "Unbind this device?"}
            </AlertDialogTitle>
            <AlertDialogDescription>
              Device{" "}
              <code className="break-all">{device.machine_hardware_id}</code> (
              <code className="break-all">{device.device_id}</code>) will return
              to seat selection when it is enabled, connected, and ready for
              binding.
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel>Cancel</AlertDialogCancel>
            <AlertDialogAction
              className="bg-destructive text-white hover:bg-destructive/90"
              disabled={isUnbinding || binding?.state !== "bound"}
              onClick={() => unbindDevice(device.device_id)}
            >
              Unbind
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
      <Button
        type="button"
        variant="outline"
        size="sm"
        className="w-20"
        disabled={isUpdatingLifecycle || device.state === "revoked"}
        onClick={() =>
          updateLifecycle({
            deviceId: device.device_id,
            state: device.state === "enabled" ? "disabled" : "enabled",
          })
        }
      >
        {device.state === "enabled" ? "Disable" : "Enable"}
      </Button>
      <AlertDialog>
        <AlertDialogTrigger asChild>
          <Button
            type="button"
            variant="destructive"
            size="sm"
            className="w-20"
            disabled={isUpdatingLifecycle || device.state === "revoked"}
          >
            Revoke
          </Button>
        </AlertDialogTrigger>
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>Permanently revoke device?</AlertDialogTitle>
            <AlertDialogDescription>
              Revocation is terminal and immediately evicts the current
              connection.
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel>Cancel</AlertDialogCancel>
            <AlertDialogAction
              className="bg-destructive text-white hover:bg-destructive/90"
              disabled={isUpdatingLifecycle || device.state === "revoked"}
              onClick={() =>
                updateLifecycle({
                  deviceId: device.device_id,
                  state: "revoked",
                })
              }
            >
              Revoke
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </div>
  );
}
