import { NavLink, Outlet } from "react-router";

import { ApiError } from "@/api/errors";
import { useLogout, useSession } from "@/auth/use-session";
import { Alert, AlertTitle } from "@/components/ui/alert";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { cn } from "@/lib/utils";

const navigation = [
  { to: "/preparation", label: "Preparation" },
  { to: "/seats", label: "Seats" },
  { to: "/accounts", label: "Accounts" },
  { to: "/bindings", label: "Bindings" },
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
        <div className="mx-auto flex h-16 max-w-7xl items-center gap-6 px-4">
          <span className="text-lg font-semibold">Natsume</span>
          <nav
            className="flex items-center gap-1"
            aria-label="Primary navigation"
          >
            {navigation.map((item) => (
              <Button key={item.to} asChild variant="ghost" size="sm">
                <NavLink
                  to={item.to}
                  className={({ isActive }) =>
                    cn(isActive && "bg-accent text-accent-foreground")
                  }
                >
                  {item.label}
                </NavLink>
              </Button>
            ))}
          </nav>
          <div className="ml-auto flex items-center gap-3">
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
      <main className="mx-auto max-w-7xl px-4 py-8">
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
