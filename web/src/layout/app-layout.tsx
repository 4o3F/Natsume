import { ChevronDown, KeyRound, LogOut, UserRound } from "lucide-react";
import { Link, NavLink, Outlet } from "react-router";

import { ApiError } from "@/api/errors";
import { useLogout, useSession } from "@/auth/use-session";
import { Alert, AlertTitle } from "@/components/ui/alert";
import { Badge } from "@/components/ui/badge";
import { Button, buttonVariants } from "@/components/ui/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { cn } from "@/lib/utils";

const navigation = [
  { to: "/preparation", label: "Preparation" },
  { to: "/seats", label: "Seats" },
  { to: "/accounts", label: "Accounts" },
  { to: "/devices", label: "Devices" },
  { to: "/enrollment", label: "Enrollment" },
  { to: "/targets", label: "Targets" },
  { to: "/print", label: "Print queue" },
];

export function AppLayout() {
  const session = useSession().data;
  const logout = useLogout();

  if (!session) {
    return null;
  }

  return (
    <div className="min-h-screen bg-background">
      <header className="border-b">
        <div className="mx-auto flex max-w-7xl flex-wrap items-center gap-x-6 gap-y-3 px-4 py-3 lg:h-16 lg:flex-nowrap lg:py-0">
          <span className="shrink-0 text-lg font-semibold">Natsume</span>
          <nav
            className="order-last flex w-full min-w-0 items-center gap-1 overflow-x-auto lg:order-none lg:w-auto lg:flex-1"
            aria-label="Primary navigation"
          >
            {session.role === "admin" && (
              <NavLink
                to="/users"
                className={({ isActive }) =>
                  cn(
                    buttonVariants({ variant: "ghost", size: "sm" }),
                    isActive
                      ? "bg-primary text-primary-foreground hover:bg-primary/90 hover:text-primary-foreground"
                      : "text-muted-foreground",
                  )
                }
              >
                Users
              </NavLink>
            )}
            {navigation.map((item) => (
              <NavLink
                key={item.to}
                to={item.to}
                className={({ isActive }) =>
                  cn(
                    buttonVariants({ variant: "ghost", size: "sm" }),
                    isActive
                      ? "bg-primary text-primary-foreground hover:bg-primary/90 hover:text-primary-foreground"
                      : "text-muted-foreground",
                  )
                }
              >
                {item.label}
              </NavLink>
            ))}
          </nav>
          <div className="ml-auto shrink-0">
            <DropdownMenu modal={false}>
              <DropdownMenuTrigger asChild>
                <Button
                  type="button"
                  variant="ghost"
                  size="sm"
                  aria-label={`Account menu for ${session.username}`}
                  title={session.username}
                >
                  <UserRound aria-hidden="true" />
                  <span className="max-w-32 truncate xl:max-w-48">
                    {session.username}
                  </span>
                  <ChevronDown aria-hidden="true" />
                </Button>
              </DropdownMenuTrigger>
              <DropdownMenuContent align="end">
                <DropdownMenuLabel>
                  <div className="max-w-64 break-all font-medium">
                    {session.username}
                  </div>
                  <div className="mt-2 flex items-center gap-2 text-muted-foreground">
                    <span>Role</span>
                    <Badge variant="secondary">
                      {session.role.toUpperCase()}
                    </Badge>
                  </div>
                </DropdownMenuLabel>
                <DropdownMenuSeparator />
                <DropdownMenuItem asChild>
                  <Link to="/change-password">
                    <KeyRound aria-hidden="true" />
                    Change password
                  </Link>
                </DropdownMenuItem>
                <DropdownMenuSeparator />
                <DropdownMenuItem
                  disabled={logout.isPending}
                  onSelect={() => logout.mutate()}
                >
                  <LogOut aria-hidden="true" />
                  Logout
                </DropdownMenuItem>
              </DropdownMenuContent>
            </DropdownMenu>
          </div>
        </div>
      </header>
      <main className="mx-auto min-w-0 w-full max-w-7xl px-4 py-8">
        {logout.error instanceof ApiError && (
          <Alert variant="destructive" className="mb-6">
            <AlertTitle>{logout.error.title}</AlertTitle>
          </Alert>
        )}
        <Outlet />
      </main>
    </div>
  );
}
