import { useEffect, useState } from "react";
import { zodResolver } from "@hookform/resolvers/zod";
import { useForm } from "react-hook-form";
import { Link, useLocation, useNavigate } from "react-router";
import { z } from "zod";

import { unwrap } from "@/api/errors";
import type { components } from "@/api/generated/schema";
import { useSessionScope } from "@/auth/session-context";
import { useLogout, useSession } from "@/auth/use-session";
import { Alert, AlertTitle } from "@/components/ui/alert";
import { Button, buttonVariants } from "@/components/ui/button";
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@/components/ui/card";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { readOperatorLink, type OperatorLinkKind } from "@/operator/links";
import { PasswordInputs } from "@/operator/password-inputs";
import { registrationSchema, resetPasswordSchema } from "@/operator/password";
import {
  operatorErrorMessage,
  useOperatorAction,
} from "@/operator/use-operator-action";

type Inspection =
  | components["schemas"]["OperatorRegistrationInspectionResponse"]
  | components["schemas"]["OperatorPasswordResetInspectionResponse"];

export function OperatorLinkPage({ kind }: { kind: OperatorLinkKind }) {
  const session = useSession();
  const logout = useLogout();
  const title =
    kind === "register" ? "Create your user account" : "Reset your password";

  return (
    <main className="flex min-h-screen items-center justify-center px-4 py-8">
      <Card className="w-full max-w-md">
        <CardHeader>
          <CardTitle>{title}</CardTitle>
          <CardDescription>Natsume operator account</CardDescription>
        </CardHeader>
        <CardContent className="space-y-4">
          {session.error ? (
            <>
              <Alert variant="destructive">
                <AlertTitle>Unable to check your session.</AlertTitle>
              </Alert>
              <Button onClick={() => session.refetch()}>Retry</Button>
            </>
          ) : session.data === undefined ? (
            <p role="status">Checking session...</p>
          ) : session.data ? (
            <>
              <p>Sign out to continue using this link.</p>
              {logout.error && (
                <Alert variant="destructive">
                  <AlertTitle>Unable to sign out. Please try again.</AlertTitle>
                </Alert>
              )}
              <Button
                disabled={logout.isPending}
                onClick={() => logout.mutate()}
              >
                Sign out and continue
              </Button>
            </>
          ) : (
            <AnonymousLinkForm key={kind} kind={kind} />
          )}
          <Link to="/login" className={buttonVariants({ variant: "link" })}>
            Back to sign in
          </Link>
        </CardContent>
      </Card>
    </main>
  );
}

function AnonymousLinkForm({ kind }: { kind: OperatorLinkKind }) {
  const { api } = useSessionScope();
  const location = useLocation();
  const navigate = useNavigate();
  // This form only mounts after the session is confirmed anonymous. The fragment
  // remains available through an explicit logout, but not a later login/remount.
  const [token] = useState(() => readOperatorLink(location.hash, kind));
  const [inspection, setInspection] = useState<Inspection | null>(null);
  const [inspectionError, setInspectionError] = useState<string | null>(null);
  const [attempt, setAttempt] = useState(0);
  const action = useOperatorAction();
  const form = useForm<z.infer<typeof registrationSchema>>({
    resolver: zodResolver(
      kind === "register"
        ? registrationSchema
        : resetPasswordSchema.safeExtend({ username: z.string() }),
    ),
    defaultValues: { username: "", password: "", password_confirmation: "" },
  });

  useEffect(() => {
    if (location.hash)
      navigate(location.pathname, { replace: true, state: null });
  }, [location.hash, location.pathname, navigate]);

  useEffect(() => {
    if (!token) return;
    const controller = new AbortController();
    async function inspect() {
      try {
        const result =
          kind === "register"
            ? await api.POST("/api/v2/operator/register/inspect", {
                body: { token },
                signal: controller.signal,
              })
            : await api.POST("/api/v2/operator/password/reset/inspect", {
                body: { token },
                signal: controller.signal,
              });
        const data = await unwrap<Inspection>(result);
        if (!controller.signal.aborted) setInspection(data);
      } catch (error) {
        if (!controller.signal.aborted)
          setInspectionError(operatorErrorMessage(error));
      }
    }
    void inspect();
    return () => controller.abort();
  }, [api, token, kind, attempt]);

  if (!token)
    return (
      <Alert variant="destructive">
        <AlertTitle>
          This link is no longer available. Contact an administrator.
        </AlertTitle>
      </Alert>
    );
  if (inspectionError)
    return (
      <>
        <Alert variant="destructive">
          <AlertTitle>{inspectionError}</AlertTitle>
        </Alert>
        <Button
          variant="outline"
          onClick={() => {
            setInspectionError(null);
            setAttempt(attempt + 1);
          }}
        >
          Check link again
        </Button>
      </>
    );
  if (!inspection) return <p role="status">Checking link...</p>;

  async function submit(values: z.infer<typeof registrationSchema>) {
    await action.run(
      async (signal) => {
        if (kind === "register") {
          return unwrap(
            await api.POST("/api/v2/operator/register", {
              body: { token, ...values },
              signal,
            }),
          );
        }
        await unwrap<void>(
          await api.POST("/api/v2/operator/password/reset", {
            body: {
              token,
              password: values.password,
              password_confirmation: values.password_confirmation,
            },
            signal,
          }),
        );
        return null;
      },
      (account) => {
        form.reset();
        const username = account?.username;
        navigate("/login", {
          replace: true,
          state: {
            operatorNotice: username
              ? `Account created for ${username}. Sign in with your new password.`
              : "Password reset. Sign in with your new password.",
            loginName: username ?? "",
          },
        });
      },
    );
  }

  const errors = form.formState.errors;
  return (
    <form className="space-y-4" onSubmit={form.handleSubmit(submit)}>
      {"role" in inspection ? (
        <p>
          Your role: <strong>{inspection.role.toUpperCase()}</strong>
        </p>
      ) : (
        <p className="break-all">
          Reset password for <strong>{inspection.username}</strong>
        </p>
      )}
      {action.error && (
        <Alert variant="destructive">
          <AlertTitle>{action.error}</AlertTitle>
        </Alert>
      )}
      {kind === "register" && (
        <div className="space-y-2">
          <Label htmlFor="username">Username</Label>
          <Input
            id="username"
            autoComplete="username"
            disabled={action.pending}
            aria-invalid={Boolean(errors.username)}
            aria-describedby="username-help"
            {...form.register("username")}
          />
          <p id="username-help" className="text-sm text-muted-foreground">
            Case-sensitive, up to 128 UTF-8 bytes; no surrounding whitespace.
          </p>
          {errors.username && (
            <p className="text-sm text-destructive">
              {errors.username.message}
            </p>
          )}
        </div>
      )}
      <PasswordInputs
        password={form.register("password")}
        confirmation={form.register("password_confirmation")}
        passwordError={errors.password?.message}
        confirmationError={errors.password_confirmation?.message}
        disabled={action.pending}
      />
      <Button type="submit" className="w-full" disabled={action.pending}>
        {action.pending
          ? "Saving..."
          : kind === "register"
            ? "Create account"
            : "Set new password"}
      </Button>
    </form>
  );
}
