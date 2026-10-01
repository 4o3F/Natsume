import { z } from "zod";

export const PASSWORD_SPECIALS = "!@#$%^&*()-_=+[]{};:,.?/~";
const passwordPattern = /^[A-Za-z0-9!@#$%^&*()_=+[\]{};:,.?/~-]+$/;

const password = z
  .string()
  .min(16, "Use at least 16 characters")
  .max(1024, "Use at most 1024 characters")
  .regex(
    passwordPattern,
    "Use only ASCII letters, digits and the listed symbols",
  )
  .regex(/[0-9]/, "Include at least one digit")
  .refine(
    (value) =>
      [...value].some((character) => PASSWORD_SPECIALS.includes(character)),
    "Include at least one listed symbol",
  );

export const passwordFields = z.object({
  password,
  password_confirmation: z.string(),
});
export const matchingPasswords = {
  message: "Passwords must match exactly",
  path: ["password_confirmation"],
};
export const passwordsMatch = (values: z.infer<typeof passwordFields>) =>
  values.password === values.password_confirmation;

export const registrationSchema = passwordFields
  .extend({
    username: z
      .string()
      .min(1, "Username is required")
      .refine(
        (value) => new TextEncoder().encode(value).length <= 128,
        "Use at most 128 UTF-8 bytes",
      )
      .refine(
        (value) => !/^\p{White_Space}|\p{White_Space}$/u.test(value),
        "Username cannot start or end with whitespace",
      ),
  })
  .refine(passwordsMatch, matchingPasswords);

export const resetPasswordSchema = passwordFields.refine(
  passwordsMatch,
  matchingPasswords,
);
export const changePasswordSchema = passwordFields
  .extend({
    current_password: z
      .string()
      .min(1, "Current password is required")
      .refine(
        (value) => new TextEncoder().encode(value).length <= 1024,
        "Current password must be at most 1024 UTF-8 bytes",
      ),
  })
  .refine(passwordsMatch, matchingPasswords);
