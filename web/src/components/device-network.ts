import type { components } from "@/api/generated/schema";

type Device = components["schemas"]["DeviceResponse"];

export function hasIpMismatch(network: Device["network"]) {
  return Boolean(
    network?.client_ip && network.client_ip !== network.server_observed_ip,
  );
}

export function deviceIpExport(devices: Device[]) {
  const addresses = [
    ...new Set(
      devices.flatMap((device) =>
        device.network ? [device.network.server_observed_ip] : [],
      ),
    ),
  ].sort((a, b) => a.localeCompare(b, "en", { numeric: true }));
  return {
    addresses,
    offline: devices.filter(
      (device) => device.convergence.connection_state === "offline",
    ).length,
    mismatched: devices.filter((device) => hasIpMismatch(device.network))
      .length,
    missing: devices.filter((device) => !device.network).length,
  };
}
