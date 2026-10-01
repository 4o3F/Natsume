import { zodResolver } from "@hookform/resolvers/zod";
import { useForm } from "react-hook-form";
import { useNavigate } from "react-router";
import { z } from "zod";

import { unwrap } from "@/api/errors";
import { useSessionScope } from "@/auth/session-context";
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
import { PasswordInputs } from "@/operator/password-inputs";
import { changePasswordSchema } from "@/operator/password";
import { useOperatorAction } from "@/operator/use-operator-action";

export function ChangePasswordPage() {
  const scope = useSessionScope();
  const navigate = useNavigate();
  const action = useOperatorAction();
  const form = useForm<z.infer<typeof changePasswordSchema>>({
    resolver: zodResolver(changePasswordSchema),
    defaultValues: {
      current_password: "",
      password: "",
      password_confirmation: "",
    },
  });
  const errors = form.formState.errors;

  return (
    <Card className="max-w-lg">
      <CardHeader>
        <CardTitle>Change password</CardTitle>
        <CardDescription>
          Changing your password signs you out on all browsers.
        </CardDescription>
      </CardHeader>
      <CardContent>
        <form
          className="space-y-4"
          onSubmit={form.handleSubmit((values) =>
            action.run(
              async (signal) => {
                await unwrap<void>(
                  await scope.api.POST("/api/v2/operator/password/change", {
                    body: values,
                    signal,
                  }),
                );
              },
              () => {
                form.reset();
                scope.logout();
                navigate("/login", {
                  replace: true,
                  state: {
                    operatorNotice:
                      "Password changed. Sign in with your new password.",
                  },
                });
              },
            ),
          )}
        >
          {action.error && (
            <Alert variant="destructive">
              <AlertTitle>{action.error}</AlertTitle>
            </Alert>
          )}
          <div className="space-y-2">
            <Label htmlFor="current_password">Current password</Label>
            <Input
              id="current_password"
              type="password"
              autoComplete="current-password"
              disabled={action.pending}
              aria-invalid={Boolean(errors.current_password)}
              {...form.register("current_password")}
            />
            {errors.current_password && (
              <p className="text-sm text-destructive">
                {errors.current_password.message}
              </p>
            )}
          </div>
          <PasswordInputs
            password={form.register("password")}
            confirmation={form.register("password_confirmation")}
            passwordError={errors.password?.message}
            confirmationError={errors.password_confirmation?.message}
            disabled={action.pending}
          />
          <Button type="submit" disabled={action.pending}>
            {action.pending ? "Saving..." : "Change password"}
          </Button>
        </form>
      </CardContent>
    </Card>
  );
}
