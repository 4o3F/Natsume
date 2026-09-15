import { useState } from "react";
import { FileSpreadsheet, LoaderCircle, Upload } from "lucide-react";

import { cn } from "@/lib/utils";

export function WorkbookDropzone({
  file,
  disabled,
  onSelect,
}: {
  file: File | null;
  disabled: boolean;
  onSelect: (files: File[]) => void;
}) {
  const [dragging, setDragging] = useState(false);
  const Icon = disabled ? LoaderCircle : file ? FileSpreadsheet : Upload;

  return (
    <div
      role="group"
      aria-label="XLSX upload"
      className="relative"
      onDragEnter={(event) => {
        if (!disabled && event.dataTransfer.types.includes("Files")) {
          event.preventDefault();
          setDragging(true);
        }
      }}
      onDragOver={(event) => {
        event.preventDefault();
        event.dataTransfer.dropEffect = disabled ? "none" : "copy";
      }}
      onDragLeave={() => setDragging(false)}
      onDrop={(event) => {
        event.preventDefault();
        setDragging(false);
        if (!disabled) onSelect(Array.from(event.dataTransfer.files));
      }}
    >
      <input
        aria-label="XLSX file"
        aria-describedby="workbook-upload-hint"
        type="file"
        accept=".xlsx"
        disabled={disabled}
        className="peer absolute inset-0 z-10 h-full w-full cursor-pointer opacity-0 disabled:cursor-not-allowed"
        onChange={(event) => {
          onSelect(Array.from(event.target.files ?? []));
          event.target.value = "";
        }}
      />
      <div
        className={cn(
          "pointer-events-none flex min-h-48 flex-col items-center justify-center gap-3 rounded-lg border-2 border-dashed px-6 py-8 text-center transition-colors peer-focus-visible:ring-2 peer-focus-visible:ring-ring peer-focus-visible:ring-offset-2",
          disabled
            ? "border-border bg-muted/20 opacity-60"
            : dragging
              ? "border-primary bg-accent"
              : "border-border bg-muted/20 peer-hover:border-muted-foreground/50 peer-hover:bg-muted/40",
        )}
      >
        <div className="flex size-12 items-center justify-center rounded-xl border bg-background text-muted-foreground">
          <Icon
            aria-hidden="true"
            className={cn(
              "size-6",
              disabled && "animate-spin motion-reduce:animate-none",
            )}
          />
        </div>
        <div className="max-w-full space-y-1">
          <p className="break-all text-sm font-semibold">
            {disabled
              ? "Uploading workbook…"
              : dragging
                ? "Drop your workbook here"
                : file
                  ? file.name
                  : "Drag & drop your XLSX file here"}
          </p>
          <p className="text-sm text-muted-foreground">
            {file
              ? `${file.size < 1024 * 1024 ? `${Math.max(1, Math.ceil(file.size / 1024))} KiB` : `${(file.size / (1024 * 1024)).toFixed(1)} MiB`} · ${disabled ? "Creating preview" : "Click or drop a file to replace"}`
              : "or click to browse files"}
          </p>
        </div>
        <p id="workbook-upload-hint" className="text-xs text-muted-foreground">
          One .xlsx workbook · Up to 8 MiB · Teams sheet required
        </p>
      </div>
    </div>
  );
}
