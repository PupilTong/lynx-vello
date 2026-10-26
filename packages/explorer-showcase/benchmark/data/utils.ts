/** Repeat a fixture without changing the requested workload size. */
export function getPaddedList<T>(list: readonly T[], count: number): T[] {
  if (list.length === 0) return [];
  return Array.from(
    { length: count },
    (_, index) => list[index % list.length]!,
  );
}
