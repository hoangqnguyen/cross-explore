// What a file's icon looks like: a page with a type glyph, an Office-style
// letter tile, a language badge, or (for programs) an app tile. Kept apart
// from the component so the table is easy to extend and test.
import { extOf } from "./format";

/** One mark in a 24×24 box: a filled or stroked path, or a bit of text. */
export type Mark = { d: string; fill?: boolean; width?: number; knock?: boolean } | { text: string; size: number; y?: number; weight?: number };

export interface IconSpec {
  /** "page": document with a glyph; "tile": rounded app square (programs). */
  shape: "page" | "tile";
  color: string;
  /** Drawn on the page (in `color`) or on the tile (in white). */
  marks?: Mark[];
  /** Office-style letter tile over the page's lower left, or a code badge. */
  badge?: { text: string; bg: string; fg?: string };
  /** Tint of the page itself (very light), so families read at a glance. */
  tint?: string;
}

const circ = (cx: number, cy: number, r: number) => `M${cx - r} ${cy}a${r} ${r} 0 1 0 ${2 * r} 0a${r} ${r} 0 1 0 ${-2 * r} 0`;

// Glyphs sit in the page's free area, roughly x 7–17, y 10–19.
const G = {
  image: [{ d: "M7.3 18.6l3.1-3.9 2.1 2.5 1.6-1.9 2.8 3.3z", fill: true }, { d: circ(14.7, 11.9, 1.35), fill: true }],
  video: [{ d: "M7.4 11.6a1 1 0 0 1 1-1h7.2a1 1 0 0 1 1 1v6.3a1 1 0 0 1-1 1H8.4a1 1 0 0 1-1-1z", fill: true }, { d: "M10.9 12.6v4.3l3.4-2.15z", fill: true, knock: true }],
  audio: [{ d: "M10.6 17.4v-6.2l5-1.1v5.9", width: 1.3 }, { d: circ(9.4, 17.4, 1.3), fill: true }, { d: circ(14.4, 16, 1.3), fill: true }],
  zip: [
    { d: "M11.1 3h1.6v1.4h-1.6zM12.7 4.4h1.6v1.4h-1.6zM11.1 5.8h1.6v1.4h-1.6zM12.7 7.2h1.6v1.4h-1.6zM11.1 8.6h1.6v1.4h-1.6z", fill: true },
    { d: "M10.7 10.8h3.8v4.6a.8.8 0 0 1-.8.8h-2.2a.8.8 0 0 1-.8-.8z", fill: true },
  ],
  code: [{ d: "M10.2 11.4l-2.7 3.1 2.7 3.1M13.8 11.4l2.7 3.1-2.7 3.1M12.8 10.8l-1.6 7.4", width: 1.3 }],
  braces: [{ d: "M10.3 10.8c-1.3 0-1.3.8-1.3 1.8s-.2 1.9-1.3 1.9c1.1 0 1.3.9 1.3 1.9s0 1.8 1.3 1.8M13.7 10.8c1.3 0 1.3.8 1.3 1.8s.2 1.9 1.3 1.9c-1.1 0-1.3.9-1.3 1.9s0 1.8-1.3 1.8", width: 1.2 }],
  lines: [{ d: "M8 11h8M8 13.4h8M8 15.8h8M8 18.2h5", width: 1.1 }],
  markdown: [{ d: "M7.4 18.3v-6.6l2.3 3 2.3-3v6.6", width: 1.3 }, { d: "M15 11.7v6M13.3 16.2l1.7 2 1.7-2", width: 1.3 }],
  grid: [{ d: "M7.5 10.7h9v8h-9zM7.5 13.4h9M7.5 16h9M10.5 10.7v8", width: 1.1 }],
  font: [{ text: "Aa", size: 7.2, y: 17.6, weight: 600 }],
  disc: [{ d: circ(12, 14.6, 4.3), width: 1.3 }, { d: circ(12, 14.6, 1.3), fill: true }],
  seal: [{ d: circ(12, 13, 2.9), fill: true }, { d: "M10.3 15.2l-1 4 2.7-1.4 2.7 1.4-1-4", fill: true }],
  key: [{ d: circ(9.7, 14.6, 2.3), width: 1.3 }, { d: "M12 14.6h5M15.3 14.6v2.1M17 14.6v1.6", width: 1.3 }],
  model: [
    { d: "M8.6 11.4l6.8 3.2M8.6 17.8l6.8-3.2M8.6 11.4v6.4", width: 1 },
    { d: circ(8.6, 11.4, 1.4), fill: true },
    { d: circ(8.6, 17.8, 1.4), fill: true },
    { d: circ(15.4, 14.6, 1.6), fill: true },
  ],
  database: [{ d: "M8 11.4c0-.9 1.8-1.5 4-1.5s4 .6 4 1.5-1.8 1.5-4 1.5-4-.6-4-1.5zM8 11.4v6.4c0 .9 1.8 1.5 4 1.5s4-.6 4-1.5v-6.4M8 14.6c0 .9 1.8 1.5 4 1.5s4-.6 4-1.5", width: 1.1 }],
  gear: [
    { d: circ(12, 14.6, 2.6), width: 1.3 },
    { d: "M12 10.2v1.6M12 17.4v1.6M7.6 14.6h1.6M14.8 14.6h1.6M8.9 11.5l1.1 1.1M14 16.6l1.1 1.1M8.9 17.7l1.1-1.1M14 12.6l1.1-1.1", width: 1.3 },
  ],
  globe: [{ d: `${circ(12, 14.6, 4.2)}M7.8 14.6h8.4M12 10.4c-1.4 1.2-2 2.6-2 4.2s.6 3 2 4.2c1.4-1.2 2-2.6 2-4.2s-.6-3-2-4.2`, width: 1.1 }],
  calendar: [{ d: "M7.5 11.4h9v7.3h-9z", width: 1.1 }, { d: "M7.5 11.4h9v2h-9z", fill: true }, { d: "M9.7 10v2M14.3 10v2", width: 1.1 }],
  contact: [{ d: circ(12, 12.6, 1.9), fill: true }, { d: "M8.7 18.6a3.3 3.3 0 0 1 6.6 0z", fill: true }],
  mail: [{ d: "M7.5 11.6h9v6.6h-9zM7.5 11.9l4.5 3.4 4.5-3.4", width: 1.1 }],
  link: [{ d: "M11 16.2l-.9.9a1.9 1.9 0 0 1-2.7-2.7l1.3-1.3a1.9 1.9 0 0 1 2.7 0M13 13l.9-.9a1.9 1.9 0 0 1 2.7 2.7l-1.3 1.3a1.9 1.9 0 0 1-2.7 0M10.6 15.8l2.8-2.8", width: 1.2 }],
  cube: [{ d: "M12 10.3l4.1 2.3v4.6L12 19.5l-4.1-2.3v-4.6zM7.9 12.6l4.1 2.3 4.1-2.3M12 14.9v4.6", width: 1.1 }],
  pen: [{ d: "M12 10.4l3.6 4.1-3.6 4.6-3.6-4.6z", width: 1.2 }, { d: circ(12, 14.8, 0.9), fill: true }],
  book: [{ d: "M7.8 11.2c1.5-.6 3.1-.6 4.2.3 1.1-.9 2.7-.9 4.2-.3v7.4c-1.5-.6-3.1-.6-4.2.3-1.1-.9-2.7-.9-4.2-.3zM12 11.5v7.4", width: 1.1 }],
  captions: [{ d: "M7.3 11.3h9.4v6.8H7.3z", width: 1.1 }, { text: "CC", size: 4.2, y: 16.2, weight: 700 }],
  download: [{ d: "M12 10.4v5.6M9.6 13.8l2.4 2.4 2.4-2.4M8 18.6h8", width: 1.3 }],
  lock: [{ d: "M8.6 14h6.8v5H8.6z", fill: true }, { d: "M10 14v-1.6a2 2 0 0 1 4 0V14", width: 1.3 }],
  shell: [{ d: "M8 12l2.6 2.4L8 16.8M11.8 17.4h4.2", width: 1.4 }],
  // Tile glyphs (drawn white on a colored square).
  terminal: [{ d: "M7.2 9.6l3.4 2.9-3.4 2.9M11.8 16.2h5", width: 1.7 }],
  window: [{ d: "M6.6 7.8h10.8v8.8H6.6z", width: 1.4 }, { d: "M6.6 7.8h10.8v2.2H6.6z", fill: true }, { d: "M9 12.6h6M9 14.6h4", width: 1.2 }],
  box: [{ d: "M12 6.8l5.6 2.9v6.5L12 19.1l-5.6-2.9V9.7zM6.4 9.7l5.6 2.9 5.6-2.9M12 12.6v6.5", width: 1.3 }],
  cog: [
    { d: circ(12, 12.5, 2.8), width: 1.6 },
    { d: "M12 7.4v1.8M12 15.8v1.8M6.9 12.5h1.8M15.3 12.5h1.8M8.4 8.9l1.3 1.3M14.3 14.8l1.3 1.3M8.4 16.1l1.3-1.3M14.3 10.2l1.3-1.3", width: 1.6 },
  ],
} satisfies Record<string, Mark[]>;

const page = (color: string, marks: Mark[], tint?: string): IconSpec => ({ shape: "page", color, marks, tint });
const badge = (text: string, bg: string, fg = "#fff", tint?: string): IconSpec => ({ shape: "page", color: bg, badge: { text, bg, fg }, marks: G.lines, tint });
const tile = (color: string, marks: Mark[]): IconSpec => ({ shape: "tile", color, marks });

const table = new Map<string, IconSpec>();
const def = (exts: string, spec: IconSpec) => exts.split(" ").forEach((e) => table.set(e, spec));

// Documents: Office-style letter tiles.
def("doc docx docm dot dotx odt ott rtf pages wpd", badge("W", "#2b5fc4", "#fff", "#eef3fc"));
def("xls xlsx xlsm xlsb xlt xltx ods ots numbers", badge("X", "#1d8a4c", "#fff", "#edf7f1"));
def("ppt pptx pptm pps ppsx pot potx odp otp key", badge("P", "#d0602a", "#fff", "#fcf1ec"));
def("pdf", badge("PDF", "#d33b2f", "#fff", "#fcefee"));
def("csv tsv", page("#1d8a4c", G.grid));
def("txt text rtfd", page("#6b7686", G.lines));
def("log out", page("#8a6d3b", G.lines));
def("md markdown mdx", page("#3d4652", G.markdown));
def("epub mobi azw azw3 djvu fb2 ibooks", page("#b2562c", G.book));

// Media.
def("png jpg jpeg jfif gif webp heic heif bmp tif tiff ico icns avif jxl insp raw cr2 cr3 nef arw dng orf rw2 raf", page("#2fa66a", G.image));
def("svg", page("#e3a008", G.pen));
def("psd psb ai eps sketch fig xd afdesign afphoto kra xcf", page("#d23f86", G.pen));
def("mp4 mov mkv avi webm m4v wmv flv mpg mpeg 3gp mts m2ts ts vob ogv insv lrv 360 braw r3d", page("#d8434f", G.video));
def("mp3 wav flac aac m4a ogg oga opus aiff aif wma alac mid midi amr", page("#d6479b", G.audio));
def("srt vtt ass ssa sub sbv", page("#5b6fd6", G.captions));
def("ttf otf woff woff2 ttc fon", page("#8e5bd0", G.font));
def("stl obj fbx blend glb gltf 3ds dae usdz usd step stp igs iges skp 3mf", page("#0f8b8d", G.cube));

// Archives and disks.
def("zip rar 7z tar gz tgz bz2 tbz xz txz zst lz4 lz lzma cab arj z", page("#b7791f", G.zip));
def("dmg iso img vhd vhdx vmdk qcow2 toast bin cue", page("#5d6b7c", G.disc));

// Code: language badges; data and config get glyphs.
const lang: [string, string, string, string?][] = [
  ["js mjs cjs jsx", "JS", "#f0c419", "#1f1f1f"],
  ["ts mts cts tsx", "TS", "#3178c6"],
  ["py pyw pyi", "PY", "#3572a5"],
  ["ipynb", "NB", "#f37626"],
  ["rs", "RS", "#b7410e"],
  ["go", "GO", "#00a7d0"],
  ["java class", "JV", "#b07219"],
  ["kt kts", "KT", "#7f52ff"],
  ["swift", "SW", "#f05138"],
  ["c", "C", "#555d6b"],
  ["h hpp hh hxx", "H", "#6e7681"],
  ["cpp cc cxx c++", "C++", "#d6336c"],
  ["cs", "C#", "#178600"],
  ["php", "PHP", "#777bb4"],
  ["rb", "RB", "#cc342d"],
  ["lua", "LUA", "#000080"],
  ["dart", "DT", "#00a4b4"],
  ["scala sc", "SC", "#c22d40"],
  ["r rmd", "R", "#276dc3"],
  ["m mm", "OC", "#438eff"],
  ["css", "CSS", "#663399"],
  ["scss sass less", "CSS", "#c6538c"],
  ["svelte", "SV", "#ff3e00"],
  ["vue", "VUE", "#41b883"],
  ["zig", "ZIG", "#d9822b"],
  ["ex exs", "EX", "#6e4a7e"],
  ["hs", "HS", "#5e5086"],
];
for (const [exts, text, bg, fg] of lang) def(exts, { shape: "page", color: bg, badge: { text, bg, fg: fg ?? "#fff" }, marks: G.code });
def("html htm xhtml", page("#e34c26", G.globe));
def("json jsonc json5 geojson webmanifest", page("#c58a00", G.braces));
def("xml plist xsd xsl xslt svgz rss atom", page("#e37933", G.code));
def("yaml yml toml ini cfg conf config env properties editorconfig gradle cmake mk makefile dockerfile nix", page("#5c6b7a", G.gear));
def("sh bash zsh fish ksh csh", page("#2f7d32", G.shell));
def("sql", page("#336791", G.database));
def("db sqlite sqlite3 db3 mdb accdb realm", page("#336791", G.database));
def("parquet feather arrow avro orc npy npz mat hdf5 h5 nc pkl pickle joblib", page("#4c5bd4", G.database));
def("pth pt onnx safetensors ckpt tflite mlmodel mlpackage pb gguf ggml keras engine", page("#e8710a", G.model));
def("lock", page("#6b7686", G.lock));

// Security.
def("cer crt der p7b p7c pem mobileprovision provisionprofile p12 pfx csr", page("#0f8b6a", G.seal));
def("key pub asc gpg sig ppk keystore jks", page("#a07d12", G.key));

// Web and personal data.
def("url webloc desktop website", page("#2878d6", G.link));
def("ics ical vcs", page("#e0443e", G.calendar));
def("vcf vcard", page("#3f7fbf", G.contact));
def("eml msg mbox emlx", page("#2878d6", G.mail));
def("torrent", page("#4c8f2f", G.download));

// Programs: app tiles, not pages.
def("exe com scr", tile("#6453d6", G.window));
def("app", tile("#2f7fe0", G.window));
def("bat cmd ps1 psm1 command tool run", tile("#2b3137", G.terminal));
def("msi msix appx appxbundle", tile("#2f6fd6", G.box));
def("pkg mpkg", tile("#b7791f", G.box));
def("deb", tile("#c7194f", G.box));
def("rpm", tile("#c42020", G.box));
def("apk aab xapk", tile("#2e9d5b", G.box));
def("ipa", tile("#1f7ae0", G.box));
def("appimage flatpak flatpakref snap", tile("#3b6fb6", G.box));
def("jar war ear", tile("#e76f00", G.box));
def("vsix crx xpi", tile("#0a7cc9", G.box));
def("dll so dylib sys drv ocx ko", tile("#5d6b7c", G.cog));
def("lnk", tile("#6b7686", G.link));

const fallback: IconSpec = page("#8a94a3", G.lines);
/** Shell scripts and binaries you can run (execute bit, or no extension at all). */
const runnable = tile("#2b3137", G.terminal);
const scriptExts = new Set("sh bash zsh fish ksh csh py pl rb js php lua tcl command run bin out elf x86_64 arm64 aarch64".split(" "));

/** Well-known names without an extension. */
const byName: Record<string, IconSpec> = {
  dockerfile: table.get("dockerfile")!,
  makefile: table.get("makefile")!,
  license: page("#6b7686", G.seal),
  readme: table.get("md")!,
  ".gitignore": table.get("gitignore") ?? page("#f05033", G.gear),
  ".env": table.get("env")!,
};

export function iconSpec(name: string, executable = false): IconSpec {
  const ext = extOf(name);
  const lower = name.toLowerCase();
  // An execute bit only means "program" for scripts and extension-less files:
  // FAT/exFAT/SMB mounts mark every file executable.
  if (executable && (!ext || scriptExts.has(ext))) return runnable;
  return table.get(ext) ?? byName[lower] ?? byName[lower.replace(/\.(md|txt)$/, "")] ?? fallback;
}

const winAppExts = new Set("exe msi lnk com scr cpl appx msix appref-ms".split(" "));

/** Local programs whose real icon the OS can hand us (see cx-thumbs). */
export function hasOsIcon(name: string, isDir: boolean, uri: string, platform: string | undefined): boolean {
  if (!uri.startsWith("file:")) return false;
  const ext = extOf(name);
  return isDir ? platform === "macos" && ext === "app" : platform === "windows" && winAppExts.has(ext);
}
