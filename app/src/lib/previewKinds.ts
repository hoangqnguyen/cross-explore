// How a file is previewed: by its type, unless the user picked a way for
// its extension (remembered in settings) or for this one look.
import type { Item } from "./api";
import type { IconName } from "./components/Icon.svelte";
import { categoryOf, extOf } from "./format";
import { settings, type PreviewAs } from "./stores/settings.svelte";
import { isArchive } from "./workspace.svelte";

export type PreviewKind = PreviewAs | "office" | "folder" | "archive" | "other";

/** The ways the user can pick, with the type the file is then served as. */
export const PREVIEW_AS: { kind: PreviewAs; label: string; icon: IconName; mime?: string }[] = [
  { kind: "text", label: "Text", icon: "code" },
  { kind: "image", label: "Image", icon: "pictures", mime: "image/png" },
  { kind: "video", label: "Video", icon: "videos", mime: "video/mp4" },
  { kind: "audio", label: "Audio", icon: "music", mime: "audio/mpeg" },
  { kind: "pdf", label: "PDF", icon: "documents", mime: "application/pdf" },
  { kind: "font", label: "Font", icon: "rename", mime: "font/otf" },
  { kind: "html", label: "Web page", icon: "eye" },
];

export const previewLabel = (kind: PreviewAs) => PREVIEW_AS.find((p) => p.kind === kind)?.label ?? kind;

const htmlExts = new Set("html htm".split(" "));
const textExts = new Set("txt md markdown log csv tsv json yaml yml toml xml ini cfg conf env sh zsh bash ps1 bat js mjs cjs ts tsx jsx rs go py rb java kt swift c h cc cpp hpp cs php lua dart scala svelte vue css scss sql gradle gitignore dockerfile makefile".split(" "));
const officeExts = new Set("docx docm dotx dotm doc xlsx xlsm xltx xlsb xls ods pptx pptm potx ppsx ppt odt ott odp otp rtf csv tsv".split(" "));

/** What the file's type alone calls for. */
export function builtInKind(entry: Item): PreviewKind {
  if (entry.isDir) return "folder";
  if (isArchive(entry.name)) return "archive";
  const cat = categoryOf(entry);
  const ext = extOf(entry.name);
  if (cat === "image" || cat === "video" || cat === "audio" || cat === "pdf" || cat === "font") return cat;
  if (htmlExts.has(ext)) return "html";
  if (officeExts.has(ext)) return "office";
  if (textExts.has(ext) || cat === "code" || cat === "text" || !ext) return "text";
  return "other";
}

/** The way remembered for the file's extension, if any. */
export function rememberedAs(entry: Item): PreviewAs | undefined {
  const ext = extOf(entry.name);
  return entry.isDir || !ext ? undefined : settings.data.previewAs[ext];
}

/** How to preview: `as` (picked for this look; "none" = just the icon), else remembered, else by type. */
export function previewKind(entry: Item, as?: PreviewAs | "none" | null): PreviewKind {
  if (entry.isDir) return "folder";
  if (as === "none") return "other";
  return as ?? rememberedAs(entry) ?? builtInKind(entry);
}

/** The type to serve the bytes as when the extension doesn't tell the web view. */
export function mimeFor(entry: Item, kind: PreviewKind): string | undefined {
  if (kind === builtInKind(entry)) return undefined;
  return PREVIEW_AS.find((p) => p.kind === kind)?.mime;
}
