import { useState } from "react";
import { ImageOff } from "lucide-react";

import { photoImageHref } from "@/api/client";
import { cn } from "@/lib/utils";

/** Current team photo, read through the admin-only image endpoint. */
export function PhotoThumb({
  accountId,
  label,
  className = "size-16",
}: {
  accountId: string;
  label: string;
  className?: string;
}) {
  const [failedAccount, setFailedAccount] = useState<string | null>(null);
  const href = photoImageHref(accountId);
  return failedAccount !== accountId ? (
    <a
      href={href}
      target="_blank"
      rel="noreferrer"
      aria-label={`Open full photo of ${label}`}
      className="inline-flex shrink-0 outline-none focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-ring"
    >
      <img
        src={href}
        alt={`Photo of ${label}`}
        loading="lazy"
        className={cn("border bg-muted object-cover", className)}
        onError={() => setFailedAccount(accountId)}
      />
    </a>
  ) : (
    <span
      role="img"
      aria-label={`Photo of ${label} (unavailable)`}
      title="Photo unavailable"
      className={cn(
        "inline-flex shrink-0 items-center justify-center border bg-muted text-muted-foreground",
        className,
      )}
    >
      <ImageOff aria-hidden="true" className="size-5" />
    </span>
  );
}
