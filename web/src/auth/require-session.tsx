import { Navigate, Outlet } from "react-router";

import { ApiError } from "@/api/errors";
import { useSession } from "@/auth/use-session";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import { Button } from "@/components/ui/button";

export function RequireSession() {
  const { data: session, error, refetch } = useSession();

  if (error) {
    return (
      <div className="flex min-h-screen items-center justify-center px-4">
        <Alert variant="destructive" className="max-w-md">
          <AlertTitle>
            {error instanceof ApiError ? error.title : error.message}
          </AlertTitle>
          <AlertDescription>
            {error instanceof ApiError && error.correlationId && (
              <p>Correlation ID: {error.correlationId}</p>
            )}
            <Button variant="outline" size="sm" onClick={() => refetch()}>
              Retry
            </Button>
          </AlertDescription>
        </Alert>
      </div>
    );
  }

  if (session === undefined) {
    return (
      <div className="flex min-h-screen items-center justify-center text-sm text-muted-foreground">
        Checking session...
      </div>
    );
  }

  if (session === null) {
    return <Navigate to="/login" replace />;
  }

  return <Outlet />;
}
