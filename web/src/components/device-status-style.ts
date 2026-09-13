import {
  Ban,
  CircleCheck,
  CircleHelp,
  CirclePause,
  CircleX,
  Clock3,
  Link,
  RefreshCw,
  Shield,
  ShieldCheck,
  TriangleAlert,
  Unlink,
  Wifi,
  WifiOff,
} from "lucide-react";

export const statusIcons = {
  check: CircleCheck,
  clock: Clock3,
  sync: RefreshCw,
  drift: TriangleAlert,
  failed: CircleX,
  pause: CirclePause,
  ban: Ban,
  shield: Shield,
  shieldCheck: ShieldCheck,
  wifi: Wifi,
  wifiOff: WifiOff,
  linked: Link,
  unlinked: Unlink,
  unknown: CircleHelp,
};

export const convergenceIcons = {
  converged: { icon: "check", tone: "text-emerald-700" },
  reconciling: { icon: "sync", tone: "text-sky-700" },
  drifted: { icon: "drift", tone: "text-amber-700" },
  failed: { icon: "failed", tone: "text-destructive" },
  awaiting_actual: { icon: "clock", tone: "text-muted-foreground" },
} as const;

export const connections = {
  active: {
    icon: "wifi",
    label: "Online",
    tone: "text-emerald-700",
    row: "bg-emerald-500/5 hover:bg-emerald-500/10",
  },
  awaiting_fresh_state: {
    icon: "clock",
    label: "Connected, awaiting fresh state",
    tone: "text-amber-700",
    row: "bg-amber-500/10 hover:bg-amber-500/15",
  },
  offline: {
    icon: "wifiOff",
    label: "Offline",
    tone: "text-destructive",
    row: "bg-destructive/10 hover:bg-destructive/15",
  },
} as const;
