import type { Entry } from "./api";
import { extOf, typeLabel } from "./format";

export type SortKey = "name" | "modified" | "type" | "size";
export interface SortSpec {
  key: SortKey;
  desc: boolean;
}

const collator = new Intl.Collator(undefined, { numeric: true, sensitivity: "base" });
const byName = (a: Entry, b: Entry) => collator.compare(a.name, b.name);

/** Comparator for a sort spec. Folders always come first, like Explorer. */
export function comparator(spec: SortSpec): (a: Entry, b: Entry) => number {
  const dir = spec.desc ? -1 : 1;
  const key: (a: Entry, b: Entry) => number =
    spec.key === "name"
      ? byName
      : spec.key === "size"
        ? (a, b) => a.size - b.size
        : spec.key === "modified"
          ? (a, b) => (a.modified ?? 0) - (b.modified ?? 0)
          : (a, b) => collator.compare(a.isDir ? "" : extOf(a.name), b.isDir ? "" : extOf(b.name)) || collator.compare(typeLabel(a), typeLabel(b));
  return (a, b) => {
    if (a.isDir !== b.isDir) return a.isDir ? -1 : 1;
    return dir * key(a, b) || byName(a, b);
  };
}

/** Merge two sorted arrays in O(n). Used to fold streamed batches in. */
export function mergeSorted<T>(a: T[], b: T[], cmp: (x: T, y: T) => number): T[] {
  if (a.length === 0) return b;
  if (b.length === 0) return a;
  const out = new Array<T>(a.length + b.length);
  let i = 0,
    j = 0,
    k = 0;
  while (i < a.length && j < b.length) out[k++] = cmp(a[i], b[j]) <= 0 ? a[i++] : b[j++];
  while (i < a.length) out[k++] = a[i++];
  while (j < b.length) out[k++] = b[j++];
  return out;
}

/** Index at which `item` should be inserted to keep `arr` sorted. */
export function insertionIndex<T>(arr: T[], item: T, cmp: (x: T, y: T) => number): number {
  let lo = 0,
    hi = arr.length;
  while (lo < hi) {
    const mid = (lo + hi) >>> 1;
    if (cmp(arr[mid], item) <= 0) lo = mid + 1;
    else hi = mid;
  }
  return lo;
}
