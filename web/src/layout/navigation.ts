import {
  ClipboardList,
  Cpu,
  History,
  KeyRound,
  LayoutGrid,
  type LucideIcon,
  Monitor,
  Printer,
  Rocket,
  Settings2,
  ShieldCheck,
  Target,
  Users,
} from "lucide-react";

export interface NavItem {
  to: string;
  label: string;
  icon: LucideIcon;
  description: string;
  adminOnly?: boolean;
  /** Match only the exact path (for parents of nested routes). */
  end?: boolean;
}

export interface NavGroup {
  label: string;
  items: NavItem[];
}

export const navigation: NavGroup[] = [
  {
    label: "Contest floor",
    items: [
      {
        to: "/devices",
        label: "Devices",
        icon: Monitor,
        description: "Connection, convergence, and lifecycle of every machine",
      },
      {
        to: "/targets",
        label: "Targets",
        icon: Target,
        description: "Desired session and foreground state",
      },
      {
        to: "/seats",
        label: "Seats",
        icon: LayoutGrid,
        description: "Seat map and team assignments",
      },
      {
        to: "/enrollment",
        label: "Enrollment",
        icon: ShieldCheck,
        description: "Provisioning window and device approvals",
      },
      {
        to: "/events",
        label: "Events",
        icon: History,
        description:
          "Journal of connections, state changes and operator activity",
      },
    ],
  },
  {
    label: "Setup",
    items: [
      {
        to: "/preparation",
        label: "Preparation",
        icon: Rocket,
        description: "Import rosters and export to DOMjudge",
      },
      {
        to: "/accounts",
        label: "Accounts",
        icon: KeyRound,
        description: "Team contest accounts",
      },
    ],
  },
  {
    label: "Printing",
    items: [
      {
        to: "/print",
        label: "Print queue",
        icon: ClipboardList,
        description: "Jobs waiting for and sent to printers",
        end: true,
      },
      {
        to: "/print/printers",
        label: "Printers",
        icon: Printer,
        description: "Paired print agents",
        adminOnly: true,
      },
      {
        to: "/print/sources",
        label: "Print sources",
        icon: Cpu,
        description: "Clients allowed to submit jobs",
        adminOnly: true,
      },
    ],
  },
  {
    label: "Administration",
    items: [
      {
        to: "/users",
        label: "Operators",
        icon: Users,
        description: "Operator accounts and invitations",
        adminOnly: true,
      },
      {
        to: "/change-password",
        label: "Change password",
        icon: Settings2,
        description: "Update your operator password",
      },
    ],
  },
];

export function visibleNavigation(isAdmin: boolean): NavGroup[] {
  return navigation
    .map((group) => ({
      ...group,
      items: group.items.filter((item) => isAdmin || !item.adminOnly),
    }))
    .filter((group) => group.items.length > 0);
}

export function findNavItem(pathname: string): NavItem | undefined {
  const items = navigation.flatMap((group) => group.items);
  return (
    items.find((item) => item.to === pathname) ??
    items
      .filter((item) => pathname.startsWith(`${item.to}/`))
      .sort((a, b) => b.to.length - a.to.length)[0]
  );
}
