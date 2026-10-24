import { Fragment, type MouseEvent, type ReactNode } from "react";
import {
  type ColumnDef,
  type RowData,
  columnVisibilityFeature,
  createSortedRowModel,
  flexRender,
  rowSortingFeature,
  sortFns,
  tableFeatures,
  useTable,
} from "@tanstack/react-table";
import { ArrowDown, ArrowUp, ChevronsUpDown } from "lucide-react";

import { Checkbox } from "@/components/ui/checkbox";
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from "@/components/ui/table";
import { cn } from "@/lib/utils";

const dataTableFeatures = tableFeatures({
  columnVisibilityFeature,
  rowSortingFeature,
  sortedRowModel: createSortedRowModel(),
  sortFns,
});

export type DataTableColumnDef<TData extends RowData> = ColumnDef<
  typeof dataTableFeatures,
  TData
>;

export interface DataTableSelection {
  selected: ReadonlySet<string>;
  onChange: (selected: Set<string>) => void;
  /** Rows that cannot be selected (for example revoked devices). */
  isSelectable?: (id: string) => boolean;
}

const interactiveSelector =
  "button, a, input, label, [role=checkbox], [role=menuitem], [role=dialog], [data-row-click-ignore]";

export function DataTable<TData extends RowData>({
  columns,
  data,
  rowClassName,
  getRowId,
  renderExpandedRow,
  scrollable = false,
  selection,
  onRowClick,
  activeRowId,
  emptyLabel = "No results.",
  label,
}: {
  columns: DataTableColumnDef<TData>[];
  data: TData[];
  rowClassName?: (row: TData) => string;
  getRowId?: (row: TData) => string;
  renderExpandedRow?: (row: TData) => ReactNode;
  scrollable?: boolean;
  selection?: DataTableSelection;
  onRowClick?: (row: TData) => void;
  activeRowId?: string | null;
  emptyLabel?: string;
  label?: string;
}) {
  const table = useTable({
    features: dataTableFeatures,
    data,
    columns,
    getRowId,
    defaultColumn: { enableSorting: false },
  });
  const rows = table.getRowModel().rows;
  const selectableIds = selection
    ? rows
        .map((row) => row.id)
        .filter((id) => selection.isSelectable?.(id) ?? true)
    : [];
  const selectedVisible = selectableIds.filter((id) =>
    selection?.selected.has(id),
  ).length;
  const headerState =
    selectedVisible === 0
      ? false
      : selectedVisible === selectableIds.length
        ? true
        : "indeterminate";

  function toggle(id: string, checked: boolean) {
    if (!selection) return;
    const next = new Set(selection.selected);
    if (checked) next.add(id);
    else next.delete(id);
    selection.onChange(next);
  }

  function handleRowClick(event: MouseEvent<HTMLTableRowElement>, row: TData) {
    if (!onRowClick) return;
    const target = event.target as HTMLElement;
    if (target.closest(interactiveSelector)) return;
    if (window.getSelection()?.toString()) return;
    onRowClick(row);
  }

  return (
    <div className="w-full min-w-0 overflow-hidden border bg-card">
      <Table
        aria-label={label}
        containerClassName={scrollable ? "isolate max-h-96" : undefined}
      >
        <TableHeader className={scrollable ? "sticky top-0 z-10" : undefined}>
          {table.getHeaderGroups().map((headerGroup) => (
            <TableRow key={headerGroup.id} className="hover:bg-transparent">
              {selection && (
                <TableHead>
                  <Checkbox
                    aria-label="Select all visible rows"
                    checked={headerState}
                    disabled={selectableIds.length === 0}
                    onCheckedChange={(checked) => {
                      const next = new Set(selection.selected);
                      for (const id of selectableIds) {
                        if (checked === true) next.add(id);
                        else next.delete(id);
                      }
                      selection.onChange(next);
                    }}
                  />
                </TableHead>
              )}
              {headerGroup.headers.map((header) => (
                <TableHead
                  key={header.id}
                  aria-sort={
                    header.column.getCanSort()
                      ? header.column.getIsSorted() === "asc"
                        ? "ascending"
                        : header.column.getIsSorted() === "desc"
                          ? "descending"
                          : "none"
                      : undefined
                  }
                >
                  {header.isPlaceholder ? null : header.column.getCanSort() ? (
                    <button
                      type="button"
                      className="-ml-1.5 inline-flex cursor-pointer items-center gap-1 px-1.5 py-1 transition-colors outline-none hover:text-foreground focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-ring"
                      onClick={header.column.getToggleSortingHandler()}
                    >
                      {flexRender(
                        header.column.columnDef.header,
                        header.getContext(),
                      )}
                      {header.column.getIsSorted() === "asc" ? (
                        <ArrowUp aria-hidden="true" className="size-3" />
                      ) : header.column.getIsSorted() === "desc" ? (
                        <ArrowDown aria-hidden="true" className="size-3" />
                      ) : (
                        <ChevronsUpDown
                          aria-hidden="true"
                          className="size-3 opacity-60"
                        />
                      )}
                    </button>
                  ) : (
                    flexRender(
                      header.column.columnDef.header,
                      header.getContext(),
                    )
                  )}
                </TableHead>
              ))}
            </TableRow>
          ))}
        </TableHeader>
        <TableBody>
          {rows.length ? (
            rows.map((row) => {
              const expanded = renderExpandedRow?.(row.original);
              const isSelected = selection?.selected.has(row.id) ?? false;
              const selectable = selection?.isSelectable?.(row.id) ?? true;
              return (
                <Fragment key={row.id}>
                  <TableRow
                    data-state={
                      isSelected || activeRowId === row.id
                        ? "selected"
                        : undefined
                    }
                    className={cn(
                      rowClassName?.(row.original),
                      onRowClick &&
                        "cursor-pointer outline-none focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-ring",
                      activeRowId === row.id &&
                        "shadow-[inset_4px_0_0_var(--primary)]",
                    )}
                    tabIndex={onRowClick ? 0 : undefined}
                    onKeyDown={(event) => {
                      if (
                        onRowClick &&
                        event.target === event.currentTarget &&
                        (event.key === "Enter" || event.key === " ")
                      ) {
                        event.preventDefault();
                        onRowClick(row.original);
                      }
                    }}
                    onClick={(event) => handleRowClick(event, row.original)}
                  >
                    {selection && (
                      <TableCell>
                        <Checkbox
                          aria-label={`Select row ${row.id}`}
                          checked={isSelected}
                          disabled={!selectable}
                          onCheckedChange={(checked) =>
                            toggle(row.id, checked === true)
                          }
                        />
                      </TableCell>
                    )}
                    {row.getVisibleCells().map((cell) => (
                      <TableCell key={cell.id}>
                        {flexRender(
                          cell.column.columnDef.cell,
                          cell.getContext(),
                        )}
                      </TableCell>
                    ))}
                  </TableRow>
                  {expanded && (
                    <TableRow className="hover:bg-transparent">
                      <TableCell
                        colSpan={
                          row.getVisibleCells().length + (selection ? 1 : 0)
                        }
                        className="bg-muted/30 whitespace-normal"
                      >
                        {expanded}
                      </TableCell>
                    </TableRow>
                  )}
                </Fragment>
              );
            })
          ) : (
            <TableRow className="hover:bg-transparent">
              <TableCell
                colSpan={columns.length + (selection ? 1 : 0)}
                className="h-28 text-center text-muted-foreground"
              >
                {emptyLabel}
              </TableCell>
            </TableRow>
          )}
        </TableBody>
      </Table>
    </div>
  );
}
