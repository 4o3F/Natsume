import type { UseFormRegisterReturn } from "react-hook-form";

import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { PASSWORD_SPECIALS } from "./password";

export function PasswordInputs({
  password,
  confirmation,
  passwordError,
  confirmationError,
  disabled,
}: {
  password: UseFormRegisterReturn;
  confirmation: UseFormRegisterReturn;
  passwordError?: string;
  confirmationError?: string;
  disabled: boolean;
}) {
  return (
    <>
      <div className="space-y-2">
        <Label htmlFor="password">New password</Label>
        <Input
          id="password"
          type="password"
          autoComplete="new-password"
          disabled={disabled}
          aria-invalid={Boolean(passwordError)}
          aria-describedby={
            passwordError ? "password-policy password-error" : "password-policy"
          }
          {...password}
        />
        <p
          id="password-policy"
          className="break-words text-sm text-muted-foreground"
        >
          16–1024 ASCII characters. Include a digit and a symbol. Letters A–Z,
          a–z, digits 0–9 and these symbols are allowed; no spaces:
          <span className="mt-1 block break-all font-mono">
            {PASSWORD_SPECIALS}
          </span>
        </p>
        {passwordError && (
          <p id="password-error" className="text-sm text-destructive">
            {passwordError}
          </p>
        )}
      </div>
      <div className="space-y-2">
        <Label htmlFor="password_confirmation">Confirm new password</Label>
        <Input
          id="password_confirmation"
          type="password"
          autoComplete="new-password"
          disabled={disabled}
          aria-invalid={Boolean(confirmationError)}
          aria-describedby={
            confirmationError ? "confirmation-error" : undefined
          }
          {...confirmation}
        />
        {confirmationError && (
          <p id="confirmation-error" className="text-sm text-destructive">
            {confirmationError}
          </p>
        )}
      </div>
    </>
  );
}
