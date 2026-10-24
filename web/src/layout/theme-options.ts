import { Monitor, Moon, Sun } from "lucide-react";

import type { Theme } from "@/components/theme-provider";

export const themeOptions: { value: Theme; label: string; icon: typeof Sun }[] =
  [
    { value: "light", label: "Light", icon: Sun },
    { value: "dark", label: "Dark", icon: Moon },
    { value: "system", label: "System", icon: Monitor },
  ];
