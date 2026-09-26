// A tiny syntax highlighter for previews: comments, strings, numbers and
// keywords of common languages. Output is HTML with escaped text.

const KEYWORDS = new Set(
  (
    "as async await break case catch class const continue crate def default defer del do else elif enum export extends extern false fi fn for from func function go if impl import in interface is lambda let loop match mod module mut new nil none not null or package pass private protected pub public raise return self static struct super switch then this throw trait true try type typeof union unsafe use var void where while with yield " +
    "int long float double char bool boolean string str u8 u16 u32 u64 i8 i16 i32 i64 usize isize f32 f64 echo local export source"
  ).split(" "),
);

export function escapeHtml(s: string) {
  return s.replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[c]!);
}

const TOKEN =
  /(\/\/[^\n]*|#(?![{[])[^\n]*|--[^\n]*|\/\*[\s\S]*?\*\/|<!--[\s\S]*?-->)|("(?:\\.|[^"\\\n])*"|'(?:\\.|[^'\\\n])*'|`(?:\\.|[^`\\])*`)|\b(0x[\da-f]+|\d+(?:\.\d+)?(?:e[+-]?\d+)?)\b|([A-Za-z_][\w]*)/gi;

const hashComments = new Set(["py", "sh", "bash", "zsh", "rb", "yaml", "yml", "toml", "conf", "ini", "r", "pl", "ps1", "dockerfile", "makefile", "env"]);
const dashComments = new Set(["sql", "lua", "hs"]);

export function highlight(code: string, lang: string | null): string {
  const l = (lang ?? "").toLowerCase();
  if (!l || ["txt", "text", "log", "csv", "tsv"].includes(l)) return escapeHtml(code);
  let out = "";
  let last = 0;
  for (const m of code.matchAll(TOKEN)) {
    const [tok, comment, str, num, word] = m;
    const i = m.index!;
    let cls: string | null = null;
    if (comment) {
      const isHash = comment.startsWith("#");
      const isDash = comment.startsWith("--");
      if ((isHash && !hashComments.has(l)) || (isDash && !dashComments.has(l))) {
        continue;
      }
      cls = "c";
    } else if (str) cls = "s";
    else if (num) cls = "n";
    else if (word && KEYWORDS.has(word)) cls = "k";
    else if (word && /^[A-Z][a-z]/.test(word)) cls = "t";
    if (!cls) continue;
    out += escapeHtml(code.slice(last, i)) + `<span class="tok-${cls}">${escapeHtml(tok)}</span>`;
    last = i + tok.length;
  }
  return out + escapeHtml(code.slice(last));
}
