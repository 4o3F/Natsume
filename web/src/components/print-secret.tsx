import { useEffect, useState } from "react";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import { Button } from "@/components/ui/button";

export function PrintSecret({
  value,
  title,
  description,
  expires,
  onDismiss,
}: {
  value: string;
  title: string;
  description: string;
  expires?: number;
  onDismiss: () => void;
}) {
  const [notice, setNotice] = useState("");
  useEffect(() => {
    if (expires === undefined) return;
    const timer = window.setTimeout(
      onDismiss,
      Math.max(0, expires - Date.now()),
    );
    return () => window.clearTimeout(timer);
  }, [expires, onDismiss]);
  return (
    <Alert>
      <AlertTitle>{title}</AlertTitle>
      <AlertDescription className="space-y-3">
        <p>{description}</p>
        {expires !== undefined && (
          <p>
            Expires at {new Date(expires).toLocaleString()}. Server validation
            remains authoritative.
          </p>
        )}
        <code className="block max-w-full break-all rounded border bg-background p-3 text-foreground">
          {value}
        </code>
        <div className="flex gap-2">
          <Button
            type="button"
            variant="outline"
            onClick={() => {
              if (!navigator.clipboard) {
                setNotice(
                  "Copy unavailable. Select the value and copy it manually.",
                );
                return;
              }
              void navigator.clipboard.writeText(value).then(
                () => setNotice("Copied."),
                () =>
                  setNotice(
                    "Copy failed. Select the value and copy it manually.",
                  ),
              );
            }}
          >
            Copy secret
          </Button>
          <Button type="button" variant="outline" onClick={onDismiss}>
            Dismiss secret
          </Button>
        </div>
        <p role="status">{notice}</p>
      </AlertDescription>
    </Alert>
  );
}
