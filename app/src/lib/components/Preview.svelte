<script lang="ts">
  // Renders a file for Quick Look, the preview pane and the gallery.
  import { fileUrl, listDir, previewOffice, previewText, thumbUrl, type Entry, type Item, type OfficeView, type TextPreview } from "../api";
  import { categoryOf, extOf } from "../format";
  import { highlight } from "../highlight";
  import { renderMarkdown } from "../markdown";
  import { isArchive } from "../workspace.svelte";
  import FileIcon from "./FileIcon.svelte";
  import PdfView from "./PdfView.svelte";
  import ZoomImage from "./ZoomImage.svelte";

  let { entry, uri, large = false }: { entry: Item; uri: string; large?: boolean } = $props();

  const textExts = new Set("txt md markdown log csv tsv json yaml yml toml xml ini cfg conf env sh zsh bash ps1 bat js mjs cjs ts tsx jsx rs go py rb java kt swift c h cc cpp hpp cs php lua dart scala svelte vue html css scss sql gradle gitignore dockerfile makefile".split(" "));

  const officeExts = new Set("docx docm dotx dotm doc xlsx xlsm xltx xlsb xls ods pptx pptm potx ppsx ppt odt ott odp otp rtf csv tsv".split(" "));

  let cat = $derived(categoryOf(entry));
  let ext = $derived(extOf(entry.name));
  let kind = $derived.by(() => {
    if (entry.isDir) return "folder";
    if (isArchive(entry.name)) return "archive";
    if (cat === "image") return "image";
    if (cat === "video") return "video";
    if (cat === "audio") return "audio";
    if (cat === "pdf") return "pdf";
    if (cat === "font") return "font";
    if (officeExts.has(ext)) return "office";
    if (textExts.has(ext) || cat === "code" || cat === "text" || !ext) return "text";
    return "other";
  });

  let text = $state<TextPreview | null>(null);
  let textError = $state<string | null>(null);
  let children = $state<Entry[] | null>(null);
  let fontFamily = $state<string | null>(null);
  let imageFailed = $state(false);
  let office = $state<OfficeView | null>(null);
  let pdfFailed = $state(false);
  let officeFailed = $state(false);

  $effect(() => {
    const u = uri;
    const k = kind;
    text = null;
    textError = null;
    children = null;
    fontFamily = null;
    imageFailed = false;
    office = null;
    officeFailed = false;
    pdfFailed = false;
    let stale = false;
    if (k === "text") {
      previewText(u, large ? 1024 * 1024 : 64 * 1024)
        .then((t) => !stale && (text = t))
        .catch(() => !stale && (textError = "No preview available"));
    } else if (k === "folder" || k === "archive") {
      const list: Entry[] = [];
      const target = k === "archive" ? `archive://${u}!/` : u;
      listDir(target, (ev) => ev.type === "batch" && list.push(...ev.entries))
        .then(() => !stale && (children = list.sort((a, b) => Number(b.isDir) - Number(a.isDir) || a.name.localeCompare(b.name))))
        .catch(() => !stale && (children = []));
    } else if (k === "office") {
      previewOffice(u)
        .then((o) => !stale && (office = o))
        .catch(() => !stale && (officeFailed = true));
    } else if (k === "font") {
      const family = `cx-preview-${Math.random().toString(36).slice(2)}`;
      new FontFace(family, `url("${fileUrl(u)}")`)
        .load()
        .then((f) => {
          document.fonts.add(f);
          if (!stale) fontFamily = family;
        })
        .catch(() => {});
    }
    return () => (stale = true);
  });

  let html = $derived(text ? (ext === "md" || ext === "markdown" ? renderMarkdown(text.text) : highlight(text.text, text.language ?? ext)) : "");
</script>

<div class="preview" class:large>
  {#if kind === "image" && !imageFailed && large}
    <ZoomImage src={fileUrl(uri)} alt={entry.name} onerror={() => (imageFailed = true)} />
  {:else if kind === "image" && !imageFailed}
    <img src={thumbUrl(uri, 640, entry.modified)} alt={entry.name} onerror={() => (imageFailed = true)} />
  {:else if kind === "video"}
    <!-- svelte-ignore a11y_media_has_caption -->
    <video src={fileUrl(uri)} controls autoplay={large} preload="metadata" poster={thumbUrl(uri, 640, entry.modified)}></video>
  {:else if kind === "audio"}
    <div class="audio">
      <FileIcon name={entry.name} isDir={false} size={96} />
      <audio src={fileUrl(uri)} controls autoplay={large}></audio>
    </div>
  {:else if kind === "pdf"}
    {#if large && !pdfFailed}
      <div class="pdfwrap"><PdfView src={fileUrl(uri)} onerror={() => (pdfFailed = true)} /></div>
    {:else if large}
      <iframe src={fileUrl(uri)} title={entry.name}></iframe>
    {:else}
      <img src={thumbUrl(uri, 640, entry.modified)} alt={entry.name} onerror={(e) => ((e.currentTarget as HTMLElement).style.display = "none")} />
    {/if}
  {:else if kind === "office" && office?.kind === "html"}
    <!-- Script-free HTML from cx-office; the empty sandbox also blocks scripts, forms and navigation. -->
    <iframe class="office" class:small={!large} sandbox="" srcdoc={office.html} title={office.title ?? entry.name}></iframe>
  {:else if kind === "office" && office?.kind === "pdf"}
    <div class="pdfwrap"><PdfView src={fileUrl(office.uri)} /></div>
  {:else if kind === "office" && !officeFailed}
    <div class="loading muted">Rendering {entry.name}…</div>
  {:else if kind === "font" && fontFamily}
    <div class="font" style:font-family={fontFamily}>
      <div class="big">Aa Bb Cc</div>
      <div>The quick brown fox jumps over the lazy dog</div>
      <div class="small">ABCDEFGHIJKLMNOPQRSTUVWXYZ abcdefghijklmnopqrstuvwxyz 0123456789 !?&amp;</div>
    </div>
  {:else if kind === "text" && text}
    {#if ext === "md" || ext === "markdown"}
      <div class="md">{@html html}</div>
    {:else}
      <pre class="code"><code>{@html html}</code></pre>
    {/if}
    {#if text.truncated}<div class="note">Preview shows the beginning of the file</div>{/if}
  {:else if (kind === "folder" || kind === "archive") && children}
    <div class="listing">
      <div class="count">{children.length} {children.length === 1 ? "item" : "items"}</div>
      {#each children.slice(0, large ? 500 : 60) as c (c.name)}
        <div class="child"><FileIcon name={c.name} isDir={c.isDir} executable={c.executable} size={16} /><span>{c.name}</span></div>
      {/each}
    </div>
  {:else}
    <div class="fallback">
      <FileIcon name={entry.name} isDir={entry.isDir} executable={entry.executable} size={large ? 128 : 88} />
      {#if textError && large}<p>{textError}</p>{/if}
    </div>
  {/if}
</div>

<style>
  .preview {
    display: flex;
    align-items: center;
    justify-content: center;
    width: 100%;
    height: 100%;
    min-height: 0;
    overflow: hidden;
  }
  img,
  video {
    max-width: 100%;
    max-height: 100%;
    object-fit: contain;
    border-radius: 6px;
  }
  video {
    background: #000;
    width: 100%;
  }
  iframe {
    width: 100%;
    height: 100%;
    border: 0;
    background: #fff;
    border-radius: 6px;
  }
  .audio {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 20px;
    width: 100%;
  }
  audio {
    width: min(100%, 420px);
  }
  .font {
    display: flex;
    flex-direction: column;
    gap: 12px;
    padding: 16px;
    color: var(--text);
    overflow: hidden;
  }
  .font .big {
    font-size: 64px;
    line-height: 1;
  }
  .large .font .big {
    font-size: 120px;
  }
  .font .small {
    font-size: 13px;
    color: var(--text-2);
  }
  .code,
  .md {
    align-self: stretch;
    width: 100%;
    margin: 0;
    overflow: auto;
    padding: 12px 14px;
    background: var(--layer-2);
    border-radius: 6px;
    box-shadow: inset 0 0 0 1px var(--stroke);
    user-select: text;
    -webkit-user-select: text;
    cursor: text;
  }
  .code {
    font-family: ui-monospace, "SF Mono", Menlo, Consolas, "Cascadia Code", monospace;
    font-size: 11.5px;
    line-height: 1.55;
    tab-size: 4;
    white-space: pre;
  }
  .large .code {
    font-size: 13px;
  }
  .md {
    line-height: 1.6;
    font-size: 13px;
  }
  .md :global(h1),
  .md :global(h2),
  .md :global(h3) {
    margin: 0.6em 0 0.3em;
    line-height: 1.25;
  }
  .md :global(code) {
    font-family: ui-monospace, Menlo, Consolas, monospace;
    font-size: 0.9em;
    padding: 1px 4px;
    border-radius: 4px;
    background: var(--hover);
  }
  .md :global(pre code) {
    display: block;
    padding: 10px;
    white-space: pre;
    overflow: auto;
  }
  .md :global(blockquote) {
    margin: 0;
    padding-left: 12px;
    border-left: 3px solid var(--stroke-strong);
    color: var(--text-2);
  }
  .md :global(li.task) {
    list-style: none;
    margin-left: -18px;
  }
  .md :global(a) {
    color: var(--accent);
  }
  :global(.tok-c) {
    color: #6a9955;
  }
  :global(.tok-s) {
    color: #c2410c;
  }
  :global(.tok-n) {
    color: #0e7490;
  }
  :global(.tok-k) {
    color: #7c3aed;
    font-weight: 500;
  }
  :global(.tok-t) {
    color: #0369a1;
  }
  @media (prefers-color-scheme: dark) {
    :global(.tok-s) {
      color: #f0a577;
    }
    :global(.tok-n) {
      color: #7dd3fc;
    }
    :global(.tok-k) {
      color: #c4a5ff;
    }
    :global(.tok-t) {
      color: #67c1f5;
    }
  }
  .note {
    position: absolute;
    bottom: 8px;
    font-size: 11px;
    color: var(--text-3);
  }
  .listing {
    align-self: stretch;
    width: 100%;
    overflow: auto;
    padding: 8px;
  }
  .count {
    font-size: 12px;
    color: var(--text-3);
    padding: 0 4px 6px;
  }
  .child {
    display: flex;
    align-items: center;
    gap: 8px;
    height: 24px;
    padding: 0 4px;
    font-size: 12.5px;
    white-space: nowrap;
    overflow: hidden;
  }
  .child span {
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .pdfwrap {
    position: relative;
    width: 100%;
    height: 100%;
    border-radius: 6px;
    overflow: hidden;
    background: var(--pdf-bg, rgb(128 128 128 / 0.18));
  }
  iframe.office {
    box-shadow: 0 0 0 1px var(--border);
  }
  /* The preview pane shows a zoomed-out page, like a thumbnail you can scroll. */
  iframe.office.small {
    width: 200%;
    height: 200%;
    flex: none;
    transform: scale(0.5);
    transform-origin: center;
  }
  .loading {
    font-size: 13px;
  }
  .fallback {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 10px;
    color: var(--text-3);
  }
</style>
