import type { SortingState } from "@tanstack/react-table";
import { useCallback, useEffect, useState } from "react";
import type { TableSortingSettings } from "@/types";

const SORTING_STORAGE_KEY = "buglogin.profile-table.sorting.v1";
const DEFAULT_SORTING: TableSortingSettings = {
  column: "name",
  direction: "asc",
};

export function useTableSorting() {
  const [sortingSettings, setSortingSettings] =
    useState<TableSortingSettings>(DEFAULT_SORTING);
  const [isLoaded, setIsLoaded] = useState(false);

  // Load sorting settings on mount
  useEffect(() => {
    try {
      const saved = window.sessionStorage.getItem(SORTING_STORAGE_KEY);
      if (saved) {
        const parsed = JSON.parse(saved) as Partial<TableSortingSettings>;
        if (
          typeof parsed.column === "string" &&
          (parsed.direction === "asc" || parsed.direction === "desc")
        ) {
          setSortingSettings({
            column: parsed.column,
            direction: parsed.direction,
          });
        }
      }
    } catch {
      // A malformed or unavailable browser-session preference falls back to
      // the default sort; it must not block profile loading.
    }
    setIsLoaded(true);
  }, []);

  // Sorting is a transient table view preference, scoped to this app session.
  const saveSortingSettings = useCallback((settings: TableSortingSettings) => {
    try {
      window.sessionStorage.setItem(
        SORTING_STORAGE_KEY,
        JSON.stringify(settings),
      );
      setSortingSettings(settings);
    } catch {
      setSortingSettings(settings);
    }
  }, []);

  // Convert our settings to tanstack table sorting format
  const getTableSorting = useCallback((): SortingState => {
    if (!isLoaded) return [];

    return [
      {
        id: sortingSettings.column,
        desc: sortingSettings.direction === "desc",
      },
    ];
  }, [sortingSettings, isLoaded]);

  // Update sorting when table state changes
  const updateSorting = useCallback(
    (sorting: SortingState) => {
      if (!isLoaded) return;

      if (sorting.length > 0) {
        const newSettings: TableSortingSettings = {
          column: sorting[0].id,
          direction: sorting[0].desc ? "desc" : "asc",
        };
        saveSortingSettings(newSettings);
      }
    },
    [saveSortingSettings, isLoaded],
  );

  return {
    sortingSettings,
    isLoaded,
    getTableSorting,
    updateSorting,
  };
}
