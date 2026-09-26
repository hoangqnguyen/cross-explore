// Line diff (Myers, O(ND)) for comparing two text files side by side.

export type DiffLine = { kind: "same" | "add" | "del"; a: number | null; b: number | null; text: string };

export function diffLines(a: string[], b: string[]): DiffLine[] {
  const n = a.length;
  const m = b.length;
  const max = n + m;
  const v = new Int32Array(2 * max + 2);
  const trace: Int32Array[] = [];
  outer: for (let d = 0; d <= max; d++) {
    trace.push(v.slice());
    for (let k = -d; k <= d; k += 2) {
      let x = k === -d || (k !== d && v[max + k - 1] < v[max + k + 1]) ? v[max + k + 1] : v[max + k - 1] + 1;
      let y = x - k;
      while (x < n && y < m && a[x] === b[y]) {
        x++;
        y++;
      }
      v[max + k] = x;
      if (x >= n && y >= m) break outer;
    }
    if (d > 4000) break; // pathological inputs: give up on minimality
  }
  // Walk the trace backwards to recover the edit script.
  const out: DiffLine[] = [];
  let x = n;
  let y = m;
  for (let d = trace.length - 1; d >= 0 && (x > 0 || y > 0); d--) {
    const vv = trace[d];
    const k = x - y;
    const prevK = k === -d || (k !== d && vv[max + k - 1] < vv[max + k + 1]) ? k + 1 : k - 1;
    const prevX = vv[max + prevK];
    const prevY = prevX - prevK;
    while (x > prevX && y > prevY) out.push({ kind: "same", a: --x, b: --y, text: a[x] });
    if (d > 0) {
      if (x === prevX) out.push({ kind: "add", a: null, b: --y, text: b[y] });
      else out.push({ kind: "del", a: --x, b: null, text: a[x] });
    }
  }
  while (x > 0 && y > 0) out.push({ kind: "same", a: --x, b: --y, text: a[x] });
  return out.reverse();
}
