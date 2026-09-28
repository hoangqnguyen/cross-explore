// Code folding for previews: where regions start and end, and highlighted
// HTML cut into lines so each line can be shown or hidden on its own.

/** A foldable region: `start` stays visible, lines start+1..end hide. */
export interface FoldRange {
  start: number;
  end: number;
}

/**
 * Split highlighter output into one HTML string per line. Spans that run
 * across a newline (block comments, template strings) are closed at the end
 * of the line and reopened on the next, so every line is well-formed.
 */
export function splitHtmlLines(html: string): string[] {
  const lines: string[] = [];
  const open: string[] = [];
  let cur = "";
  const re = /<span\b[^>]*>|<\/span>|\n/g;
  let last = 0;
  for (const m of html.matchAll(re)) {
    cur += html.slice(last, m.index);
    last = m.index! + m[0].length;
    if (m[0] === "\n") {
      lines.push(cur + "</span>".repeat(open.length));
      cur = open.join("");
    } else if (m[0] === "</span>") {
      open.pop();
      cur += m[0];
    } else {
      open.push(m[0]);
      cur += m[0];
    }
  }
  lines.push(cur + html.slice(last));
  return lines;
}

/** Languages whose blocks are marked by indentation rather than brackets. */
const INDENTED = new Set(["py", "python", "pyi", "yaml", "yml", "nim", "coffee", "sass", "pug", "haml", "fs", "gd"]);

const indentOf = (s: string) => {
  let n = 0;
  for (const c of s) {
    if (c === " ") n++;
    else if (c === "\t") n += 4;
    else break;
  }
  return n;
};

/** Foldable regions of `text`, outermost first for equal starts. */
export function foldRanges(text: string, lang: string | null, maxLines = 50_000): FoldRange[] {
  const lines = text.split("\n");
  if (lines.length > maxLines) return [];
  const l = (lang ?? "").toLowerCase();
  // Prose and data have no blocks worth folding.
  if (!l || ["txt", "text", "plaintext", "log", "csv", "tsv", "md", "markdown"].includes(l)) return [];
  const ranges = INDENTED.has(l) ? byIndent(lines) : byBrackets(lines, l);
  return ranges.filter((r) => r.end > r.start).sort((a, b) => a.start - b.start || b.end - a.end);
}

function byIndent(lines: string[]): FoldRange[] {
  const out: FoldRange[] = [];
  const blank = lines.map((s) => s.trim() === "");
  for (let i = 0; i < lines.length; i++) {
    if (blank[i]) continue;
    const ind = indentOf(lines[i]);
    // Next non-blank line.
    let j = i + 1;
    while (j < lines.length && blank[j]) j++;
    if (j >= lines.length || indentOf(lines[j]) <= ind) continue;
    // Extend while lines are blank or deeper.
    let end = j;
    for (let k = j; k < lines.length; k++) {
      if (blank[k]) continue;
      if (indentOf(lines[k]) <= ind) break;
      end = k;
    }
    out.push({ start: i, end });
  }
  return out;
}

const PAIRS: Record<string, string> = { "{": "}", "[": "]", "(": ")" };

/**
 * Bracket blocks spanning lines. Strings and comments are skipped with a
 * light scanner (good enough for folding; not a parser). Markup (HTML/XML)
 * folds on elements that open and close on different lines.
 */
function byBrackets(lines: string[], lang: string): FoldRange[] {
  if (["html", "htm", "xml", "svg", "plist", "vue", "svelte", "xhtml"].includes(lang)) return byTags(lines);
  const out: FoldRange[] = [];
  const stack: { ch: string; line: number }[] = [];
  const hash = ["sh", "bash", "zsh", "rb", "toml", "conf", "ini", "r", "pl", "ps1", "dockerfile", "makefile", "env"].includes(lang);
  let inBlock = false; // /* … */
  let inTemplate = false; // `…`
  for (let n = 0; n < lines.length; n++) {
    const s = lines[n];
    let quote: string | null = null;
    for (let i = 0; i < s.length; i++) {
      const c = s[i];
      if (inBlock) {
        if (c === "*" && s[i + 1] === "/") (inBlock = false), i++;
        continue;
      }
      if (inTemplate) {
        if (c === "\\") i++;
        else if (c === "`") inTemplate = false;
        continue;
      }
      if (quote) {
        if (c === "\\") i++;
        else if (c === quote) quote = null;
        continue;
      }
      if (c === "/" && s[i + 1] === "/") break;
      if (c === "/" && s[i + 1] === "*") {
        inBlock = true;
        i++;
        continue;
      }
      if (hash && c === "#") break;
      if (c === '"' || c === "'") {
        // Rust lifetimes / char-ish apostrophes: only treat ' as a quote when it closes on this line.
        if (c === "'" && s.indexOf("'", i + 1) < 0) continue;
        quote = c;
        continue;
      }
      if (c === "`") {
        inTemplate = true;
        continue;
      }
      if (c in PAIRS) stack.push({ ch: c, line: n });
      else if (c === "}" || c === "]" || c === ")") {
        // Pop to the matching opener (tolerate stray closers).
        for (let k = stack.length - 1; k >= 0; k--) {
          if (PAIRS[stack[k].ch] === c) {
            const open = stack[k];
            stack.length = k;
            // Hide the inner lines; the closing line stays when it has only the closer.
            const end = s.slice(0, i).trim() === "" ? n - 1 : n;
            if (end > open.line) out.push({ start: open.line, end });
            break;
          }
        }
      }
    }
  }
  // Several blocks opening on one line (`({`) give duplicate starts: keep the widest.
  const best = new Map<number, FoldRange>();
  for (const r of out) {
    const b = best.get(r.start);
    if (!b || r.end > b.end) best.set(r.start, r);
  }
  return [...best.values()];
}

function byTags(lines: string[]): FoldRange[] {
  const out: FoldRange[] = [];
  const stack: { name: string; line: number }[] = [];
  const voids = new Set("area base br col embed hr img input link meta source track wbr".split(" "));
  const tag = /<(\/?)([A-Za-z][\w:.-]*)[^>]*?(\/?)>|<!--[\s\S]*?-->/g;
  for (let n = 0; n < lines.length; n++) {
    for (const m of lines[n].matchAll(tag)) {
      if (!m[2]) continue;
      const [, close, raw, selfClose] = m;
      const name = raw.toLowerCase();
      if (selfClose || voids.has(name)) continue;
      if (!close) stack.push({ name, line: n });
      else {
        for (let k = stack.length - 1; k >= 0; k--) {
          if (stack[k].name === name) {
            const open = stack[k];
            stack.length = k;
            const end = lines[n].slice(0, m.index).trim() === "" ? n - 1 : n;
            if (end > open.line) out.push({ start: open.line, end });
            break;
          }
        }
      }
    }
  }
  return out;
}
