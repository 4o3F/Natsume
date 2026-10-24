import { useCallback, useState } from "react";
import {
  ChevronRight,
  ChevronsUpDown,
  KeyRound,
  LogOut,
  Menu,
  PanelLeftClose,
  PanelLeftOpen,
  Search,
} from "lucide-react";
import { Link, NavLink, Outlet, useLocation } from "react-router";

import { ApiError } from "@/api/errors";
import type { SessionResponse } from "@/api/generated/types";
import { useLogout, useSession } from "@/auth/use-session";
import { Alert, AlertTitle } from "@/components/ui/alert";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { Kbd } from "@/components/ui/kbd";
import {
  Sheet,
  SheetContent,
  SheetDescription,
  SheetTitle,
} from "@/components/ui/sheet";
import {
  Tooltip,
  TooltipContent,
  TooltipTrigger,
} from "@/components/ui/tooltip";
import { cn } from "@/lib/utils";
import { CommandMenu } from "./command-menu";
import { findNavItem, navigation, visibleNavigation } from "./navigation";
import { ThemeMenu } from "./theme-menu";
import { useCommandMenuShortcut } from "./use-command-shortcut";

const COLLAPSED_KEY = "natsume-sidebar-collapsed";

function Brand({ collapsed }: { collapsed: boolean }) {
  return (
    <Link
      to="/"
      className="flex h-12 items-center gap-3 px-2 outline-none focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-ring"
    >
      <span className="flex size-8 shrink-0 items-center justify-center bg-inverse text-sm font-semibold text-inverse-foreground">
        N
      </span>
      {!collapsed && (
        <span className="flex min-w-0 flex-col leading-tight">
          <span className="text-sm font-semibold">Natsume</span>
          <span className="text-caption text-muted-foreground">
            Contest control
          </span>
        </span>
      )}
    </Link>
  );
}

function SidebarNav({
  isAdmin,
  collapsed,
  onNavigate,
}: {
  isAdmin: boolean;
  collapsed: boolean;
  onNavigate?: () => void;
}) {
  return (
    <nav aria-label="Primary navigation" className="flex flex-col gap-5">
      {visibleNavigation(isAdmin).map((group) => (
        <div key={group.label} className="flex flex-col gap-0.5">
          {collapsed ? (
            <span
              aria-hidden="true"
              className="mx-3 mb-1 h-px bg-sidebar-border"
            />
          ) : (
            <span className="px-3 pb-1 text-caption text-muted-foreground">
              {group.label}
            </span>
          )}
          {group.items.map((item) => {
            const link = (
              <NavLink
                key={item.to}
                to={item.to}
                end={item.end}
                onClick={onNavigate}
                aria-label={collapsed ? item.label : undefined}
                className={({ isActive }) =>
                  cn(
                    "group relative flex h-8 items-center gap-3 px-3 text-sm text-sidebar-foreground transition-colors outline-none hover:bg-sidebar-accent hover:text-sidebar-accent-foreground focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-ring",
                    collapsed && "justify-center px-0",
                    isActive &&
                      "bg-sidebar-accent font-semibold text-sidebar-accent-foreground before:absolute before:inset-y-0 before:left-0 before:w-1 before:bg-sidebar-primary",
                  )
                }
              >
                <item.icon aria-hidden="true" className="size-4 shrink-0" />
                {!collapsed && <span className="truncate">{item.label}</span>}
              </NavLink>
            );
            return collapsed ? (
              <Tooltip key={item.to}>
                <TooltipTrigger asChild>{link}</TooltipTrigger>
                <TooltipContent side="right">{item.label}</TooltipContent>
              </Tooltip>
            ) : (
              link
            );
          })}
        </div>
      ))}
    </nav>
  );
}

function initials(name: string) {
  return name
    .split(/[\s._@-]+/)
    .filter(Boolean)
    .slice(0, 2)
    .map((part) => part[0]?.toUpperCase())
    .join("");
}

function AccountMenu({
  session,
  collapsed,
  logout,
  onNavigate,
}: {
  session: SessionResponse;
  collapsed: boolean;
  logout: ReturnType<typeof useLogout>;
  onNavigate?: () => void;
}) {
  return (
    <DropdownMenu modal={false}>
      <DropdownMenuTrigger asChild>
        <Button
          type="button"
          variant="ghost"
          className={cn(
            "h-12 w-full justify-start gap-3 px-2 text-foreground hover:bg-sidebar-accent",
            collapsed && "justify-center px-0",
          )}
          aria-label={`Account menu for ${session.username}`}
        >
          <span className="flex size-8 shrink-0 items-center justify-center rounded-full bg-secondary text-xs font-semibold text-secondary-foreground">
            {initials(session.username) || "?"}
          </span>
          {!collapsed && (
            <>
              <span className="flex min-w-0 flex-1 flex-col items-start leading-tight">
                <span className="max-w-full truncate text-sm">
                  {session.username}
                </span>
                <span className="text-caption text-muted-foreground capitalize">
                  {session.role}
                </span>
              </span>
              <ChevronsUpDown
                aria-hidden="true"
                className="text-muted-foreground"
              />
            </>
          )}
        </Button>
      </DropdownMenuTrigger>
      <DropdownMenuContent
        side={collapsed ? "right" : "top"}
        align={collapsed ? "end" : "start"}
        className="w-56"
      >
        <DropdownMenuLabel className="flex items-center justify-between gap-2">
          <span className="truncate text-foreground">{session.username}</span>
          <Badge variant={session.role === "admin" ? "default" : "secondary"}>
            {session.role}
          </Badge>
        </DropdownMenuLabel>
        <DropdownMenuSeparator />
        <DropdownMenuItem asChild>
          <Link to="/change-password" onClick={onNavigate}>
            <KeyRound aria-hidden="true" />
            Change password
          </Link>
        </DropdownMenuItem>
        <DropdownMenuItem
          variant="destructive"
          disabled={logout.isPending}
          onSelect={() => {
            onNavigate?.();
            logout.mutate();
          }}
        >
          <LogOut aria-hidden="true" />
          Log out
        </DropdownMenuItem>
      </DropdownMenuContent>
    </DropdownMenu>
  );
}

function Breadcrumb() {
  const { pathname } = useLocation();
  const item = findNavItem(pathname);
  const group = navigation.find((entry) =>
    entry.items.some((candidate) => candidate === item),
  );
  if (!item) return null;
  return (
    <div
      aria-label="Breadcrumb"
      role="navigation"
      className="flex min-w-0 items-center gap-1.5 text-sm"
    >
      {group && (
        <>
          <span className="hidden truncate text-muted-foreground sm:inline">
            {group.label}
          </span>
          <ChevronRight
            aria-hidden="true"
            className="hidden size-3.5 shrink-0 text-muted-foreground/60 sm:block"
          />
        </>
      )}
      <span aria-current="page" className="truncate font-semibold">
        {item.label}
      </span>
    </div>
  );
}

export function AppLayout() {
  const session = useSession().data;
  const logout = useLogout();
  const [collapsed, setCollapsed] = useState(
    () => localStorage.getItem(COLLAPSED_KEY) === "1",
  );
  const [mobileOpen, setMobileOpen] = useState(false);
  const [commandOpen, setCommandOpen] = useState(false);
  const openCommand = useCallback(() => setCommandOpen(true), []);
  useCommandMenuShortcut(openCommand);

  if (!session) {
    return null;
  }
  const isAdmin = session.role === "admin";

  function toggleCollapsed() {
    setCollapsed((value) => {
      localStorage.setItem(COLLAPSED_KEY, value ? "0" : "1");
      return !value;
    });
  }

  return (
    <div className="flex min-h-screen bg-background">
      <aside
        className={cn(
          "sticky top-0 hidden h-screen shrink-0 flex-col border-r border-sidebar-border bg-sidebar text-sidebar-foreground transition-[width] duration-200 lg:flex",
          collapsed ? "w-16" : "w-60",
        )}
      >
        <div className="px-3">
          <Brand collapsed={collapsed} />
        </div>
        <div className="flex-1 overflow-y-auto px-3 py-3">
          <SidebarNav isAdmin={isAdmin} collapsed={collapsed} />
        </div>
        <div className="flex flex-col gap-1 border-t border-sidebar-border p-2">
          <Button
            type="button"
            variant="ghost"
            size="sm"
            onClick={toggleCollapsed}
            aria-label={collapsed ? "Expand sidebar" : "Collapse sidebar"}
            className={cn(
              "justify-start text-muted-foreground",
              collapsed && "justify-center",
            )}
          >
            {collapsed ? (
              <PanelLeftOpen aria-hidden="true" />
            ) : (
              <>
                <PanelLeftClose aria-hidden="true" />
                Collapse
              </>
            )}
          </Button>
          <AccountMenu
            session={session}
            collapsed={collapsed}
            logout={logout}
          />
        </div>
      </aside>

      <Sheet open={mobileOpen} onOpenChange={setMobileOpen}>
        <SheetContent
          side="left"
          className="w-72 gap-0 bg-sidebar p-0 text-sidebar-foreground"
        >
          <SheetTitle className="sr-only">Navigation</SheetTitle>
          <SheetDescription className="sr-only">Natsume pages</SheetDescription>
          <div className="px-3">
            <Brand collapsed={false} />
          </div>
          <div className="flex-1 overflow-y-auto px-3 py-3">
            <SidebarNav
              isAdmin={isAdmin}
              collapsed={false}
              onNavigate={() => setMobileOpen(false)}
            />
          </div>
          <div className="border-t border-sidebar-border p-2">
            <AccountMenu
              session={session}
              collapsed={false}
              logout={logout}
              onNavigate={() => setMobileOpen(false)}
            />
          </div>
        </SheetContent>
      </Sheet>

      <div className="flex min-w-0 flex-1 flex-col">
        <header className="sticky top-0 z-30 flex h-12 shrink-0 items-center gap-3 border-b bg-background px-4 sm:px-6">
          <Button
            type="button"
            variant="ghost"
            size="icon-sm"
            className="-ml-1 lg:hidden"
            aria-label="Open navigation"
            onClick={() => setMobileOpen(true)}
          >
            <Menu aria-hidden="true" />
          </Button>
          <Breadcrumb />
          <div className="ml-auto flex items-center gap-1.5">
            <Button
              type="button"
              variant="ghost"
              size="sm"
              onClick={openCommand}
              className="w-8 justify-start border-0 border-b border-border-strong bg-input px-0 text-muted-foreground hover:bg-accent hover:text-foreground sm:w-64 sm:px-3"
              aria-label="Search (Ctrl+K)"
            >
              <Search aria-hidden="true" className="mx-auto sm:mx-0" />
              <span className="hidden sm:inline">Search…</span>
              <Kbd className="ml-auto hidden sm:inline-flex">⌘K</Kbd>
            </Button>
            <ThemeMenu />
          </div>
        </header>
        <main className="mx-auto w-full max-w-screen-2xl min-w-0 flex-1 px-4 py-6 sm:px-6 lg:px-8 lg:py-8">
          {logout.error instanceof ApiError && (
            <Alert variant="destructive" className="mb-6">
              <AlertTitle>{logout.error.title}</AlertTitle>
            </Alert>
          )}
          <Outlet />
        </main>
      </div>
      <CommandMenu
        open={commandOpen}
        onOpenChange={setCommandOpen}
        isAdmin={isAdmin}
      />
    </div>
  );
}
