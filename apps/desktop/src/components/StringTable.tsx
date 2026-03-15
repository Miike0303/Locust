import { useState, useRef, useCallback, useMemo } from "react";
import {
  useReactTable,
  getCoreRowModel,
  getSortedRowModel,
  flexRender,
  type ColumnDef,
  type SortingState,
} from "@tanstack/react-table";
import clsx from "clsx";
import type { StringEntry } from "../lib/api";
import { patchString } from "../lib/api";
import { useEditorStore } from "../stores/editorStore";

const statusBadge: Record<string, string> = {
  pending: "bg-gray-200 text-gray-700",
  translated: "bg-blue-100 text-blue-700",
  reviewed: "bg-amber-100 text-amber-700",
  approved: "bg-green-100 text-green-700",
  error: "bg-red-100 text-red-700",
};

function InlineEdit({ entry, onSave }: { entry: StringEntry; onSave: () => void }) {
  const [editing, setEditing] = useState(false);
  const [value, setValue] = useState(entry.translation || "");
  const ref = useRef<HTMLTextAreaElement>(null);

  const handleBlur = async () => {
    setEditing(false);
    if (value !== (entry.translation || "")) {
      await patchString(entry.id, { translation: value } as any);
      onSave();
    }
  };

  if (editing) {
    return (
      <textarea
        ref={ref}
        value={value}
        onChange={(e) => setValue(e.target.value)}
        onBlur={handleBlur}
        onKeyDown={(e) => {
          if (e.key === "Enter" && e.ctrlKey) handleBlur();
          if (e.key === "Escape") { setEditing(false); setValue(entry.translation || ""); }
        }}
        autoFocus
        className="w-full p-1 text-sm border border-emerald-400 rounded bg-white dark:bg-gray-800 focus:outline-none resize-none"
        rows={2}
      />
    );
  }

  return (
    <div
      onClick={(e) => { e.stopPropagation(); setEditing(true); setValue(entry.translation || ""); }}
      className="cursor-text text-sm truncate"
    >
      {entry.translation || <span className="text-gray-400 italic">Click to edit...</span>}
    </div>
  );
}

interface StringTableProps {
  data: StringEntry[];
  onRefetch: () => void;
}

export default function StringTable({ data, onRefetch }: StringTableProps) {
  const { selectedEntryId, setSelected } = useEditorStore();
  const [sorting, setSorting] = useState<SortingState>([]);

  const columns = useMemo<ColumnDef<StringEntry, any>[]>(
    () => [
      {
        accessorKey: "status",
        header: "Status",
        size: 100,
        cell: ({ getValue }) => {
          const status = getValue() as string;
          return (
            <span className={clsx("px-2 py-0.5 rounded-full text-xs font-medium", statusBadge[status] || "bg-gray-100")}>
              {status}
            </span>
          );
        },
      },
      {
        accessorKey: "source",
        header: "Source",
        size: 300,
        cell: ({ getValue }) => (
          <div className="text-sm line-clamp-2" title={getValue() as string}>
            {getValue() as string}
          </div>
        ),
      },
      {
        accessorKey: "translation",
        header: "Translation",
        size: 300,
        cell: ({ row }) => <InlineEdit entry={row.original} onSave={onRefetch} />,
      },
      {
        accessorKey: "file_path",
        header: "File",
        size: 150,
        cell: ({ getValue }) => {
          const full = getValue() as string;
          const name = full.split(/[/\\]/).pop() || full;
          return <span className="text-xs text-gray-500" title={full}>{name}</span>;
        },
      },
      {
        accessorKey: "tags",
        header: "Tags",
        size: 120,
        cell: ({ getValue }) => (
          <div className="flex gap-1 flex-wrap">
            {(getValue() as string[]).map((t) => (
              <span key={t} className="px-1.5 py-0.5 bg-gray-100 dark:bg-gray-700 rounded text-xs">{t}</span>
            ))}
          </div>
        ),
      },
    ],
    [onRefetch]
  );

  const table = useReactTable({
    data,
    columns,
    state: { sorting },
    onSortingChange: setSorting,
    getCoreRowModel: getCoreRowModel(),
    getSortedRowModel: getSortedRowModel(),
  });

  return (
    <div className="overflow-auto flex-1">
      <table className="w-full text-left">
        <thead className="sticky top-0 bg-gray-50 dark:bg-gray-800 z-10">
          {table.getHeaderGroups().map((hg) => (
            <tr key={hg.id}>
              {hg.headers.map((header) => (
                <th
                  key={header.id}
                  onClick={header.column.getToggleSortingHandler()}
                  className="px-3 py-2 text-xs font-semibold text-gray-500 uppercase cursor-pointer hover:bg-gray-100 dark:hover:bg-gray-700 select-none"
                  style={{ width: header.getSize() }}
                >
                  {flexRender(header.column.columnDef.header, header.getContext())}
                  {{ asc: " ↑", desc: " ↓" }[header.column.getIsSorted() as string] ?? ""}
                </th>
              ))}
            </tr>
          ))}
        </thead>
        <tbody>
          {table.getRowModel().rows.map((row) => (
            <tr
              key={row.id}
              onClick={() => setSelected(row.original.id)}
              className={clsx(
                "border-b border-gray-100 dark:border-gray-800 cursor-pointer transition-colors",
                selectedEntryId === row.original.id
                  ? "bg-emerald-50 dark:bg-emerald-900/20"
                  : "hover:bg-gray-50 dark:hover:bg-gray-800/50"
              )}
            >
              {row.getVisibleCells().map((cell) => (
                <td key={cell.id} className="px-3 py-2" style={{ maxWidth: cell.column.getSize() }}>
                  {flexRender(cell.column.columnDef.cell, cell.getContext())}
                </td>
              ))}
            </tr>
          ))}
        </tbody>
      </table>
      {data.length === 0 && (
        <div className="flex items-center justify-center h-48 text-gray-400">
          No strings found. Open a project to get started.
        </div>
      )}
    </div>
  );
}
