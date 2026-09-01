import { zodResolver } from "@hookform/resolvers/zod";
import { useForm } from "react-hook-form";
import { Navigate } from "react-router";
import { z } from "zod";

import { ApiError } from "@/api/errors";
import { useLogin, useSession } from "@/auth/use-session";
import { Alert, AlertTitle } from "@/components/ui/alert";
import { Button } from "@/components/ui/button";
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@/components/ui/card";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";

const loginSchema = z.object({
  login_name: z.string().min(1, "Login name is required"),
  password: z.string().min(1, "Password is required"),
});

type LoginValues = z.infer<typeof loginSchema>;

export function LoginPage() {
  const session = useSession().data;
  const login = useLogin();
  const {
    register,
    handleSubmit,
    formState: { errors },
  } = useForm<LoginValues>({
    resolver: zodResolver(loginSchema),
    defaultValues: {
      login_name: "",
      password: "",
    },
  });

  if (session) {
    return <Navigate to="/" replace />;
  }

  const apiError = login.error instanceof ApiError ? login.error : null;

  return (
    <main className="flex min-h-screen items-center justify-center px-4">
      <Card className="w-full max-w-sm">
        <CardHeader>
          <CardTitle>Sign in</CardTitle>
          <CardDescription>
            Enter your Natsume operator credentials.
          </CardDescription>
        </CardHeader>
        <CardContent>
          <form
            className="space-y-4"
            onSubmit={handleSubmit((values) => login.mutate(values))}
          >
            {apiError && (
              <Alert variant="destructive">
                <AlertTitle>
                  {apiError.code === "AUTHENTICATION_FAILED"
                    ? `Login failed: ${apiError.title}`
                    : apiError.title}
                </AlertTitle>
              </Alert>
            )}
            <div className="space-y-2">
              <Label htmlFor="login_name">Login name</Label>
              <Input
                id="login_name"
                autoComplete="username"
                aria-invalid={Boolean(errors.login_name)}
                {...register("login_name")}
              />
              {errors.login_name && (
                <p className="text-sm text-destructive">
                  {errors.login_name.message}
                </p>
              )}
            </div>
            <div className="space-y-2">
              <Label htmlFor="password">Password</Label>
              <Input
                id="password"
                type="password"
                autoComplete="current-password"
                aria-invalid={Boolean(errors.password)}
                {...register("password")}
              />
              {errors.password && (
                <p className="text-sm text-destructive">
                  {errors.password.message}
                </p>
              )}
            </div>
            <Button className="w-full" type="submit" disabled={login.isPending}>
              Sign in
            </Button>
          </form>
        </CardContent>
      </Card>
    </main>
  );
}
