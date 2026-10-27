import {
  Fragment,
  type KeyboardEvent,
  type MouseEvent,
  type ReactNode,
  useCallback,
  useLayoutEffect,
  useRef,
  useState,
} from "react";
import {
  defaultRangeExtractor,
  type Range,
  useWindowVirtualizer,
} from "@tanstack/react-virtual";
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
  virtualized = false,
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
  virtualized?: boolean;
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
  const bodyRef = useRef<HTMLTableSectionElement>(null);
  const [scrollMargin, setScrollMargin] = useState(0);
  const [focusedRowId, setFocusedRowId] = useState<string | null>(null);
  const pendingFocus = useRef<{ id: string; last: boolean } | null>(null);
  const virtual = virtualized && rows.length > 100 && !renderExpandedRow;
  const getItemKey = useCallback((index: number) => rows[index].id, [rows]);
  const rangeExtractor = useCallback(
    (range: Range) => {
      const indexes = new Set(defaultRangeExtractor(range));
      // Keep focus and detail-sheet return targets mounted outside the viewport.
      for (const id of [focusedRowId, activeRowId]) {
        const index = id ? rows.findIndex((row) => row.id === id) : -1;
        if (index >= 0) indexes.add(index);
      }
      return [...indexes].sort((a, b) => a - b);
    },
    [rows, focusedRowId, activeRowId],
  );
  const virtualizer = useWindowVirtualizer({
    count: rows.length,
    enabled: virtual,
    estimateSize: () => 56,
    overscan: 5,
    getItemKey,
    rangeExtractor,
    scrollMargin,
    scrollPaddingStart: 48,
  });
  useLayoutEffect(() => {
    if (!virtual) return;
    const updateMargin = () => {
      const body = bodyRef.current;
      if (body)
        setScrollMargin(body.getBoundingClientRect().top + window.scrollY);
    };
    updateMargin();
    const observer = new ResizeObserver(updateMargin);
    observer.observe(document.documentElement);
    return () => observer.disconnect();
  }, [virtual]);
  useLayoutEffect(() => {
    const request = pendingFocus.current;
    if (!request) return;
    if (!virtual) {
      pendingFocus.current = null;
      return;
    }
    const index = rows.findIndex((row) => row.id === request.id);
    if (index < 0) {
      pendingFocus.current = null;
      return;
    }
    const row = bodyRef.current?.querySelector<HTMLTableRowElement>(
      `tr[data-index="${index}"]`,
    );
    if (!row) return;
    const target = request.last ? (rowTabStops(row).at(-1) ?? row) : row;
    target.focus({ preventScroll: true });
    pendingFocus.current = null;
  });
  const virtualItems = virtualizer.getVirtualItems();
  const renderedRows = virtual
    ? virtualItems.map((item, position) => ({
        row: rows[item.index],
        index: item.index,
        paddingBefore:
          item.start - (virtualItems[position - 1]?.end ?? scrollMargin),
      }))
    : rows.map((row, index) => ({ row, index, paddingBefore: 0 }));
  const paddingAfter = virtual
    ? virtualizer.getTotalSize() -
      ((virtualItems.at(-1)?.end ?? scrollMargin) - scrollMargin)
    : 0;
  const columnCount =
    table.getVisibleLeafColumns().length + (selection ? 1 : 0);
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

  function focusRow(index: number, last = false) {
    pendingFocus.current = { id: rows[index].id, last };
    setFocusedRowId(rows[index].id);
    virtualizer.scrollToIndex(index, { align: "auto" });
  }

  function handleRowKeyDown(
    event: KeyboardEvent<HTMLTableRowElement>,
    index: number,
  ) {
    if (!virtual || !onRowClick) return;
    const row = event.currentTarget;
    if (event.key === "Tab") {
      const last = rowTabStops(row).at(-1) ?? row;
      if (event.shiftKey && event.target === row && index > 0) {
        event.preventDefault();
        focusRow(index - 1, true);
      } else if (
        !event.shiftKey &&
        event.target === last &&
        index < rows.length - 1
      ) {
        event.preventDefault();
        focusRow(index + 1);
      }
      return;
    }
    if (event.target !== row) return;
    const next =
      event.key === "ArrowDown"
        ? Math.min(index + 1, rows.length - 1)
        : event.key === "ArrowUp"
          ? Math.max(index - 1, 0)
          : event.key === "Home"
            ? 0
            : event.key === "End"
              ? rows.length - 1
              : null;
    if (next !== null) {
      event.preventDefault();
      if (next !== index) focusRow(next);
    }
  }

  return (
    <div className="w-full min-w-0 overflow-hidden border bg-card">
      <Table
        aria-label={label}
        aria-rowcount={virtual ? rows.length + 1 : undefined}
        containerClassName={scrollable ? "isolate max-h-96" : undefined}
      >
        <TableHeader className={scrollable ? "sticky top-0 z-10" : undefined}>
          {table.getHeaderGroups().map((headerGroup) => (
            <TableRow
              key={headerGroup.id}
              aria-rowindex={virtual ? 1 : undefined}
              className="hover:bg-transparent"
            >
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
        <TableBody ref={bodyRef}>
          {rows.length ? (
            renderedRows.map(({ row, index, paddingBefore }) => {
              const expanded = renderExpandedRow?.(row.original);
              const isSelected = selection?.selected.has(row.id) ?? false;
              const selectable = selection?.isSelectable?.(row.id) ?? true;
              return (
                <Fragment key={row.id}>
                  {virtual && (
                    <RowSpacer height={paddingBefore} columns={columnCount} />
                  )}
                  <TableRow
                    ref={virtual ? virtualizer.measureElement : undefined}
                    data-index={virtual ? index : undefined}
                    aria-rowindex={virtual ? index + 2 : undefined}
                    onFocusCapture={
                      virtual ? () => setFocusedRowId(row.id) : undefined
                    }
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
                      handleRowKeyDown(event, index);
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
          {virtual && <RowSpacer height={paddingAfter} columns={columnCount} />}
        </TableBody>
      </Table>
    </div>
  );
}

function RowSpacer({ height, columns }: { height: number; columns: number }) {
  if (height <= 0) return null;
  return (
    <tr aria-hidden="true" role="presentation">
      <td colSpan={columns} style={{ height, padding: 0, border: 0 }} />
    </tr>
  );
}

function rowTabStops(row: HTMLTableRowElement) {
  return [
    ...row.querySelectorAll<HTMLElement>(
      "button, a[href], input, select, textarea, [tabindex]",
    ),
  ].filter(
    (element) =>
      element.tabIndex >= 0 &&
      !element.matches(":disabled") &&
      element.getClientRects().length > 0,
  );
}
