import type { Entry } from "./api";

export function extOf(name: string): string {
  const i = name.lastIndexOf(".");
  return i > 0 ? name.slice(i + 1).toLowerCase() : "";
}

export function formatSize(bytes: number): string {
  if (bytes < 1024) return `${bytes} bytes`;
  const units = ["KB", "MB", "GB", "TB", "PB"];
  let v = bytes / 1024;
  let u = 0;
  while (v >= 1024 && u < units.length - 1) {
    v /= 1024;
    u++;
  }
  return `${v < 10 ? v.toFixed(1) : Math.round(v).toLocaleString()} ${units[u]}`;
}

const time = new Intl.DateTimeFormat(undefined, { hour: "numeric", minute: "2-digit" });
const weekday = new Intl.DateTimeFormat(undefined, { weekday: "long" });
const full = new Intl.DateTimeFormat(undefined, { year: "numeric", month: "short", day: "numeric" });
const precise = new Intl.DateTimeFormat(undefined, { dateStyle: "full", timeStyle: "medium" });

/** Friendly relative dates like Finder: "Just now", "Today 14:02", "Yesterday 09:10". */
export function formatDate(ms: number | null, now = Date.now()): string {
  if (ms == null) return "";
  const d = new Date(ms);
  const diff = now - ms;
  if (diff >= 0 && diff < 60_000) return "Just now";
  if (diff >= 0 && diff < 3_600_000) {
    const m = Math.floor(diff / 60_000);
    return `${m} min ago`;
  }
  const today = new Date(now);
  today.setHours(0, 0, 0, 0);
  const day = 86_400_000;
  if (ms >= today.getTime()) return `Today ${time.format(d)}`;
  if (ms >= today.getTime() - day) return `Yesterday ${time.format(d)}`;
  if (ms >= today.getTime() - 6 * day) return `${weekday.format(d)} ${time.format(d)}`;
  return `${full.format(d)} ${time.format(d)}`;
}

export const formatDateFull = (ms: number | null) => (ms == null ? "" : precise.format(new Date(ms)));

export type Category = "folder" | "image" | "video" | "audio" | "archive" | "code" | "doc" | "sheet" | "slides" | "pdf" | "text" | "app" | "font" | "disk" | "file";

const categories: Record<string, Category> = {};
const add = (c: Category, exts: string) => exts.split(" ").forEach((e) => (categories[e] = c));
add("image", "png jpg jpeg gif webp heic heif bmp tiff tif svg ico raw cr2 nef arw dng psd avif");
add("video", "mp4 mov mkv avi webm m4v wmv flv mpg mpeg 3gp");
add("audio", "mp3 wav flac aac m4a ogg opus aiff wma alac");
add("archive", "zip rar 7z tar gz tgz bz2 xz zst lz4 cab");
add("code", "js mjs cjs ts tsx jsx rs go py rb java kt swift c h cc cpp hpp cs php sh zsh bash ps1 lua dart scala svelte vue html css scss json yaml yml toml xml sql gradle");
add("doc", "doc docx rtf odt pages");
add("sheet", "xls xlsx csv ods numbers tsv");
add("slides", "ppt pptx key odp");
add("pdf", "pdf");
add("text", "txt md markdown log ini cfg conf env");
add("app", "app exe msi pkg deb rpm appimage apk ipa bat cmd");
add("font", "ttf otf woff woff2");
add("disk", "dmg iso img vhd vhdx vmdk");

export function categoryOf(e: Pick<Entry, "name" | "isDir">): Category {
  if (e.isDir) return extOf(e.name) === "app" ? "app" : "folder";
  return categories[extOf(e.name)] ?? "file";
}

const typeNames: Partial<Record<Category, string>> = {
  image: "Image",
  video: "Video",
  audio: "Audio",
  archive: "Archive",
  doc: "Document",
  sheet: "Spreadsheet",
  slides: "Presentation",
  pdf: "PDF Document",
  text: "Text Document",
  app: "Application",
  font: "Font",
  disk: "Disk Image",
};

export function typeLabel(e: Entry): string {
  if (e.isDir) return categoryOf(e) === "app" ? "Application" : e.kind === "symlink" ? "Folder alias" : "File folder";
  const ext = extOf(e.name);
  const cat = categoryOf(e);
  const base = typeNames[cat];
  if (cat === "code" || cat === "file" || !base) return ext ? `${ext.toUpperCase()} File` : "File";
  return `${ext.toUpperCase()} ${base === "PDF Document" ? "Document" : base}`;
}

/** Range of the name to preselect when renaming: the stem, not the extension. */
export function stemRange(name: string, isDir: boolean): [number, number] {
  const i = name.lastIndexOf(".");
  return !isDir && i > 0 ? [0, i] : [0, name.length];
}
