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

/** Semantic tones shared by icons, dots, and badges. */
export const tones = {
  success: { text: "text-success-strong", dot: "bg-success", badge: "success" },
  info: { text: "text-info", dot: "bg-info", badge: "info" },
  warning: {
    text: "text-warning-strong",
    dot: "bg-warning",
    badge: "warning",
  },
  danger: {
    text: "text-destructive",
    dot: "bg-destructive",
    badge: "destructive",
  },
  muted: {
    text: "text-muted-foreground",
    dot: "bg-muted-foreground/40",
    badge: "muted",
  },
} as const;
export type Tone = keyof typeof tones;

export const convergenceIcons = {
  converged: { icon: "check", tone: "text-success-strong", key: "success" },
  reconciling: { icon: "sync", tone: "text-info", key: "info" },
  drifted: { icon: "drift", tone: "text-warning-strong", key: "warning" },
  failed: { icon: "failed", tone: "text-destructive", key: "danger" },
  awaiting_actual: {
    icon: "clock",
    tone: "text-muted-foreground",
    key: "muted",
  },
} as const;

export const connections = {
  active: {
    icon: "wifi",
    label: "Online",
    tone: "text-success-strong",
    key: "success",
    row: "",
  },
  awaiting_fresh_state: {
    icon: "clock",
    label: "Connected, awaiting fresh state",
    tone: "text-warning-strong",
    key: "warning",
    row: "bg-warning-subtle/50 hover:bg-warning-subtle",
  },
  offline: {
    icon: "wifiOff",
    label: "Offline",
    tone: "text-destructive",
    key: "danger",
    row: "bg-destructive-subtle/50 hover:bg-destructive-subtle",
  },
} as const;

/** Convergence parts in display order, keyed by convergence response field. */
export const convergenceParts = [
  ["gateway", "Gateway"],
  ["binding", "Binding"],
  ["runtime_config", "Runtime"],
  ["session_control", "Session"],
  ["home", "Home"],
] as const;
