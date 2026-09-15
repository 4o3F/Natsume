import { NavLink, Outlet } from "react-router";

import { ApiError } from "@/api/errors";
import { useLogout, useSession } from "@/auth/use-session";
import { Alert, AlertTitle } from "@/components/ui/alert";
import { Badge } from "@/components/ui/badge";
import { Button, buttonVariants } from "@/components/ui/button";
import { cn } from "@/lib/utils";

const navigation = [
  { to: "/preparation", label: "Preparation" },
  { to: "/seats", label: "Seats" },
  { to: "/accounts", label: "Accounts" },
  { to: "/bindings", label: "Bindings" },
  { to: "/devices", label: "Devices" },
  { to: "/enrollment", label: "Enrollment" },
  { to: "/targets", label: "Targets" },
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
          <div className="ml-auto flex shrink-0 items-center gap-3">
            <Badge variant="secondary">{session.role.toUpperCase()}</Badge>
            <span
              className="font-mono text-sm text-muted-foreground"
              title={session.operator_id}
            >
              {session.operator_id.slice(0, 8)}
            </span>
            <Button
              type="button"
              variant="outline"
              size="sm"
              disabled={logout.isPending}
              onClick={() => logout.mutate()}
            >
              Logout
            </Button>
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
