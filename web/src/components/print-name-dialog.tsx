import { useState, type ReactNode } from "react";

import { Alert, AlertTitle } from "@/components/ui/alert";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { validPrintName } from "@/pages/print-management";

type Props = {
  id: string;
  title: string;
  description: ReactNode;
  label: string;
  placeholder: string;
  hint: string;
  submitLabel: string;
  pending: boolean;
  error: string | null;
  onSubmit: (name: string) => void;
};

/** Single-name create form for print printers and sources. */
export function PrintNameDialog({
  open,
  onOpenChange,
  ...props
}: Props & { open: boolean; onOpenChange: (open: boolean) => void }) {
  return (
    <Dialog
      open={open}
      onOpenChange={(next) => {
        if (!props.pending) onOpenChange(next);
      }}
    >
      <DialogContent className="sm:max-w-md">
        {/* Mounted only while open, so the field resets between uses. */}
        <NameForm {...props} onCancel={() => onOpenChange(false)} />
      </DialogContent>
    </Dialog>
  );
}

function NameForm({
  id,
  title,
  description,
  label,
  placeholder,
  hint,
  submitLabel,
  pending,
  error,
  onSubmit,
  onCancel,
}: Props & { onCancel: () => void }) {
  const [name, setName] = useState("");
  const valid = validPrintName(name);
  return (
    <form
      className="space-y-5"
      onSubmit={(event) => {
        event.preventDefault();
        if (valid) onSubmit(name);
      }}
    >
      <DialogHeader>
        <DialogTitle>{title}</DialogTitle>
        <DialogDescription>{description}</DialogDescription>
      </DialogHeader>
      <div className="space-y-2">
        <Label htmlFor={id}>{label}</Label>
        <Input
          id={id}
          placeholder={placeholder}
          autoComplete="off"
          aria-describedby={`${id}-hint`}
          value={name}
          onChange={(event) => setName(event.target.value)}
          disabled={pending}
        />
        <p id={`${id}-hint`} className="text-xs text-muted-foreground">
          {hint}
        </p>
      </div>
      {error && (
        <Alert variant="destructive">
          <AlertTitle>{error}</AlertTitle>
        </Alert>
      )}
      <DialogFooter>
        <Button
          type="button"
          variant="outline"
          disabled={pending}
          onClick={onCancel}
        >
          Cancel
        </Button>
        <Button type="submit" loading={pending} disabled={!valid}>
          {submitLabel}
        </Button>
      </DialogFooter>
    </form>
  );
}
