// Subsequence fuzzy matching with bonuses for word starts and runs, good
// enough for a command palette of a few hundred entries.

export interface Match {
  score: number;
  positions: number[];
}

export function fuzzy(query: string, text: string): Match | null {
  if (!query) return { score: 0, positions: [] };
  const q = query.toLowerCase();
  const t = text.toLowerCase();
  const positions: number[] = [];
  let score = 0;
  let ti = 0;
  let prev = -2;
  for (const ch of q) {
    if (ch === " ") continue;
    const i = t.indexOf(ch, ti);
    if (i < 0) return null;
    const wordStart = i === 0 || /[\s/_\-.·:]/.test(t[i - 1]) || (text[i] !== t[i] && text[i - 1] === t[i - 1]);
    score += 1 + (wordStart ? 6 : 0) + (i === prev + 1 ? 4 : 0) - Math.min(3, (i - ti) * 0.05);
    positions.push(i);
    prev = i;
    ti = i + 1;
  }
  // Prefer shorter texts and matches near the start.
  score -= text.length * 0.02 + (positions[0] ?? 0) * 0.1;
  if (t.startsWith(q)) score += 10;
  return { score, positions };
}

/** Wrap matched characters for display (escaped). */
export function markMatches(text: string, positions: number[]): string {
  const set = new Set(positions);
  let out = "";
  for (let i = 0; i < text.length; i++) {
    const c = text[i].replace(/[&<>"']/g, (x) => `&#${x.charCodeAt(0)};`);
    out += set.has(i) ? `<mark>${c}</mark>` : c;
  }
  return out;
}
