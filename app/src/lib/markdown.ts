// Minimal, safe Markdown → HTML for Quick Look. All text is escaped first,
// so file contents can never inject markup or scripts.
import { escapeHtml, highlight } from "./highlight";

function inline(s: string): string {
  return escapeHtml(s)
    .replace(/`([^`]+)`/g, "<code>$1</code>")
    .replace(/\*\*([^*]+)\*\*/g, "<strong>$1</strong>")
    .replace(/(^|[^*])\*([^*]+)\*/g, "$1<em>$2</em>")
    .replace(/\[([^\]]+)\]\((https?:[^)\s]+)\)/g, '<a href="$2" target="_blank" rel="noreferrer">$1</a>');
}

const LIST_ITEM = /^(\s*)([-*+]|\d+[.)])\s+(.*)$/;
const HEADING = /^(#{1,6})\s+(.*)$/;
const RULE = /^\s*(---+|\*\*\*+|___+)\s*$/;
const TABLE_DELIM = /^\s*\|?\s*:?-+:?\s*(\|\s*:?-+:?\s*)*\|?\s*$/;

/** Leading whitespace as columns (a tab counts as 4). */
const indentOf = (s: string) => s.match(/^\s*/)![0].replace(/\t/g, "    ").length;

/** A GFM table row's cells: outer pipes dropped, `\|` kept as a literal pipe. */
function cells(row: string): string[] {
  // Scanned rather than split with a lookbehind, which older macOS web views can't parse.
  const out: string[] = [];
  let cur = "";
  const s = row.trim();
  for (let k = 0; k < s.length; k++) {
    if (s[k] === "\\" && s[k + 1] === "|") cur += s[++k];
    else if (s[k] === "|") {
      if (k > 0) out.push(cur.trim());
      cur = "";
    } else cur += s[k];
  }
  if (cur.trim() || !s.endsWith("|")) out.push(cur.trim());
  return out;
}

/** A header row followed by a delimiter row (`|---|:--:|`) starts a table. */
function isTable(lines: string[], i: number): boolean {
  return i + 1 < lines.length && lines[i].includes("|") && TABLE_DELIM.test(lines[i + 1]) && lines[i + 1].includes("-") && cells(lines[i]).length === cells(lines[i + 1]).length;
}

/** Whether line `i` starts a block of its own (so it ends a paragraph or a list item's text). */
function startsBlock(lines: string[], i: number): boolean {
  const line = lines[i];
  return HEADING.test(line) || line.startsWith("```") || /^\s*>/.test(line) || LIST_ITEM.test(line) || RULE.test(line) || isTable(lines, i);
}

export function renderMarkdown(src: string): string {
  const lines = src.replace(/\r\n/g, "\n").split("\n");
  const out: string[] = [];
  let i = 0;
  while (i < lines.length) {
    const line = lines[i];
    const fence = /^```(\w*)/.exec(line);
    if (fence) {
      const body: string[] = [];
      i++;
      while (i < lines.length && !lines[i].startsWith("```")) body.push(lines[i++]);
      i++;
      out.push(`<pre><code>${highlight(body.join("\n"), fence[1] || null)}</code></pre>`);
      continue;
    }
    const h = HEADING.exec(line);
    if (h) {
      out.push(`<h${h[1].length}>${inline(h[2])}</h${h[1].length}>`);
      i++;
      continue;
    }
    if (isTable(lines, i)) {
      const head = cells(line);
      const align = cells(lines[i + 1]).map((d) => (d.startsWith(":") && d.endsWith(":") ? "center" : d.endsWith(":") ? "right" : d.startsWith(":") ? "left" : ""));
      const cell = (tag: string, text: string, k: number) => `<${tag}${align[k] ? ` style="text-align:${align[k]}"` : ""}>${inline(text)}</${tag}>`;
      const rows: string[] = [];
      i += 2;
      while (i < lines.length && lines[i].trim() && lines[i].includes("|") && !(HEADING.test(lines[i]) || lines[i].startsWith("```") || /^\s*>/.test(lines[i]))) {
        const c = cells(lines[i++]);
        rows.push(`<tr>${head.map((_, k) => cell("td", c[k] ?? "", k)).join("")}</tr>`);
      }
      out.push(`<div class="table"><table><thead><tr>${head.map((t, k) => cell("th", t, k)).join("")}</tr></thead><tbody>${rows.join("")}</tbody></table></div>`);
      continue;
    }
    if (LIST_ITEM.test(line)) {
      // Items with their indent, so deeper ones nest. A line that isn't an
      // item or another block continues the item above (wrapped text), and a
      // blank line only ends the list if what follows isn't more of it.
      const items: { indent: number; ordered: boolean; start: number; text: string }[] = [];
      while (i < lines.length) {
        const m = LIST_ITEM.exec(lines[i]);
        // Switching between bullets and numbers at the outer level starts a new list.
        if (m && items.length && indentOf(m[1]) <= items[0].indent && /\d/.test(m[2]) !== items[0].ordered) break;
        if (m) {
          items.push({ indent: indentOf(m[1]), ordered: /\d/.test(m[2]), start: parseInt(m[2], 10), text: m[3] });
          i++;
        } else if (lines[i].trim() && !startsBlock(lines, i)) {
          items[items.length - 1].text += " " + lines[i++].trim();
        } else if (!lines[i].trim()) {
          let j = i;
          while (j < lines.length && !lines[j].trim()) j++;
          if (j < lines.length && (LIST_ITEM.test(lines[j]) || (indentOf(lines[j]) >= 2 && !startsBlock(lines, j)))) i = j;
          else break;
        } else break;
      }
      const html: string[] = [];
      const open: { indent: number; tag: string }[] = [];
      for (const it of items) {
        const top = open[open.length - 1];
        if (!top || it.indent > top.indent) {
          const tag = it.ordered ? "ol" : "ul";
          html.push(it.ordered && it.start !== 1 ? `<ol start="${it.start}">` : `<${tag}>`);
          open.push({ indent: it.indent, tag });
        } else {
          while (open.length > 1 && it.indent < open[open.length - 1].indent && it.indent <= open[open.length - 2].indent) html.push(`</li></${open.pop()!.tag}>`);
          html.push("</li>");
        }
        const task = /^\[( |x)\]\s*(.*)$/i.exec(it.text);
        html.push(task ? `<li class="task"><input type="checkbox" disabled ${task[1] !== " " ? "checked" : ""}> ${inline(task[2])}` : `<li>${inline(it.text)}`);
      }
      while (open.length) html.push(`</li></${open.pop()!.tag}>`);
      out.push(html.join(""));
      continue;
    }
    if (/^\s*>/.test(line)) {
      const q: string[] = [];
      while (i < lines.length && /^\s*>/.test(lines[i])) q.push(lines[i++].replace(/^\s*>\s?/, ""));
      out.push(`<blockquote>${inline(q.join(" "))}</blockquote>`);
      continue;
    }
    if (RULE.test(line)) {
      out.push("<hr>");
      i++;
      continue;
    }
    if (!line.trim()) {
      i++;
      continue;
    }
    // Always take this line: it matched no block above (say "#hashtag", which
    // isn't a heading), so stopping on it again would loop forever.
    const para: string[] = [lines[i++]];
    while (i < lines.length && lines[i].trim() && !startsBlock(lines, i)) para.push(lines[i++]);
    out.push(`<p>${inline(para.join(" "))}</p>`);
  }
  return out.join("\n");
}
