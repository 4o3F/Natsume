import { useMemo, useState } from "react";
import { useMutation, useQuery } from "@tanstack/react-query";
import type { ColumnDef } from "@tanstack/react-table";
import {
  CircleCheck,
  CircleHelp,
  ImageOff,
  RefreshCw,
  TriangleAlert,
} from "lucide-react";

import { useSessionScope } from "@/auth/session-context";
import { unwrap } from "@/api/errors";
import type { components } from "@/api/generated/schema";
import { DataTable } from "@/components/data-table";
import { Button } from "@/components/ui/button";
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@/components/ui/card";
import { Input } from "@/components/ui/input";

type School = components["schemas"]["OrganizationLogoResponse"];
const states = {
  available: {
    label: "Available",
    icon: CircleCheck,
    color: "text-green-600",
  },
  missing: {
    label: "Missing",
    icon: ImageOff,
    color: "text-amber-600",
  },
  ambiguous: {
    label: "Ambiguous",
    icon: CircleHelp,
    color: "text-amber-600",
  },
  invalid: {
    label: "Invalid",
    icon: TriangleAlert,
    color: "text-destructive",
  },
};

export function PreparationLogos({ candidateId }: { candidateId?: string }) {
  const { api } = useSessionScope();
  const [search, setSearch] = useState("");
  const [problemsOnly, setProblemsOnly] = useState(false);
  const schools = useQuery({
    queryKey: ["organization-logos", candidateId ?? "committed"],
    queryFn: async () =>
      unwrap<School[]>(
        candidateId
          ? await api.GET("/api/v2/imports/{import_id}/organizations", {
              params: { path: { import_id: candidateId } },
            })
          : await api.GET("/api/v2/organizations"),
      ),
  });
  const download = useMutation({
    mutationFn: async () => {
      const blob = await unwrap<Blob>(
        await api.GET("/api/v2/exports/domjudge", { parseAs: "blob" }),
      );
      const url = URL.createObjectURL(blob);
      const link = document.createElement("a");
      link.href = url;
      link.download = "domjudge-export.zip";
      link.click();
      // Let the browser take ownership of the download before releasing the URL.
      window.setTimeout(() => URL.revokeObjectURL(url), 1000);
    },
    gcTime: 0,
  });
  const columns = useMemo<ColumnDef<School>[]>(
    () => [
      {
        accessorKey: "organization_id",
        header: "School ID",
        enableSorting: true,
      },
      {
        id: "school",
        header: "School",
        cell: ({ row }) => (
          <div className="max-w-sm whitespace-normal break-words">
            <p>{row.original.name_zh || row.original.name_en}</p>
            {row.original.name_zh && (
              <p className="text-sm text-muted-foreground">
                {row.original.name_en}
              </p>
            )}
          </div>
        ),
      },
      {
        id: "logo",
        header: "Logo",
        cell: ({ row }) => (
          <LogoThumbnail
            school={row.original}
            candidateId={candidateId}
            updatedAt={schools.dataUpdatedAt}
          />
        ),
      },
      {
        accessorKey: "status",
        header: "Status",
        enableSorting: true,
        cell: ({ row }) => {
          const { label, icon: Icon, color } = states[row.original.status];
          return (
            <span className={`inline-flex items-center gap-2 ${color}`}>
              <Icon aria-hidden="true" className="size-4 shrink-0" />
              {label}
            </span>
          );
        },
      },
      {
        id: "source",
        header: "Source image",
        cell: ({ row }) => (
          <div className="max-w-sm whitespace-normal break-words">
            <p>
              {row.original.files.join(", ") ||
                "Add an image named after this school"}
            </p>
            {row.original.detail && (
              <p className="text-sm text-destructive">{row.original.detail}</p>
            )}
          </div>
        ),
      },
    ],
    [candidateId, schools.dataUpdatedAt],
  );
  const rows = (schools.data ?? []).filter(
    (school) =>
      (!problemsOnly || school.status !== "available") &&
      [school.organization_id, school.name_zh, school.name_en].some((value) =>
        value.toLowerCase().includes(search.toLowerCase()),
      ),
  );
  const problemCount =
    schools.data?.filter((school) => school.status !== "available").length ?? 0;
  return (
    <Card>
      <CardHeader>
        <div className="flex flex-wrap items-start justify-between gap-4">
          <div className="space-y-1.5">
            <CardTitle>
              {candidateId
                ? "Preview school logos"
                : "Committed roster & DOMjudge export"}
            </CardTitle>
            <CardDescription>
              {candidateId
                ? "Candidate INST IDs and source images. Missing or ambiguous logos do not block import."
                : "Export all committed teams, schools, categories, current passwords and available PNG logos in one ZIP."}
            </CardDescription>
          </div>
          {!candidateId && (
            <Button
              onClick={() => download.mutate()}
              disabled={download.isPending || !schools.data?.length}
            >
              {download.isPending ? "Building ZIP..." : "Download DOMjudge ZIP"}
            </Button>
          )}
        </div>
      </CardHeader>
      <CardContent className="space-y-4">
        <div className="flex flex-wrap items-center gap-3">
          <Input
            aria-label={
              candidateId
                ? "Search preview schools"
                : "Search committed schools"
            }
            placeholder="Search school or INST ID"
            value={search}
            onChange={(event) => setSearch(event.target.value)}
            className="max-w-sm"
          />
          <Button
            variant={problemsOnly ? "secondary" : "outline"}
            aria-pressed={problemsOnly}
            onClick={() => setProblemsOnly(!problemsOnly)}
          >
            Problems ({problemCount})
          </Button>
          <Button
            variant="outline"
            disabled={schools.isFetching}
            onClick={() => void schools.refetch()}
          >
            <RefreshCw aria-hidden="true" className="size-4 shrink-0" />
            Refresh logos
          </Button>
          <span className="text-sm text-muted-foreground">
            {schools.data?.length ?? 0} schools
          </span>
        </div>
        {schools.isLoading && <p role="status">Checking school logos...</p>}
        {(schools.error || download.error) && (
          <p role="alert" className="text-destructive">
            {(schools.error || download.error)?.message}
          </p>
        )}
        <div data-testid={candidateId ? "preview-logos" : "committed-logos"}>
          <DataTable
            scrollable
            columns={columns}
            data={rows}
            getRowId={(row) => row.organization_id}
          />
        </div>
      </CardContent>
    </Card>
  );
}

function LogoThumbnail({
  school,
  candidateId,
  updatedAt,
}: {
  school: School;
  candidateId?: string;
  updatedAt: number;
}) {
  const [failedAttempt, setFailedAttempt] = useState<string | null>(null);
  const base = candidateId
    ? `/api/v2/imports/${encodeURIComponent(candidateId)}/organizations`
    : "/api/v2/organizations";
  const src = `${base}/${encodeURIComponent(school.organization_id)}/logo`;
  const attempt = `${src}@${updatedAt}`;
  return school.status === "available" && failedAttempt !== attempt ? (
    <img
      key={attempt}
      src={src}
      alt={`${school.name_zh || school.name_en} logo`}
      loading="lazy"
      className="size-12 object-contain"
      onError={() => setFailedAttempt(attempt)}
    />
  ) : (
    <span
      className="inline-flex size-12 items-center justify-center text-muted-foreground"
      title={
        failedAttempt === attempt
          ? "Image unavailable; refresh to retry"
          : "No image"
      }
    >
      <ImageOff aria-hidden="true" className="size-4 shrink-0" />
      <span className="sr-only">
        {failedAttempt === attempt ? "Image unavailable" : "No image"}
      </span>
    </span>
  );
}
