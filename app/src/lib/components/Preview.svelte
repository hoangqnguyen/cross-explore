<script lang="ts">
  // Renders a file for Quick Look, the preview pane and the gallery.
  import { fileUrl, listDir, previewOffice, previewText, thumbUrl, type Entry, type Item, type OfficeView, type TextPreview } from "../api";
  import { extOf } from "../format";
  import { highlight } from "../highlight";
  import { renderMarkdown } from "../markdown";
  import { pinchZoom, pinchZoomFrame } from "../pinchZoom";
  import { builtInKind, mimeFor, previewKind, previewLabel } from "../previewKinds";
  import { settings, type PreviewAs } from "../stores/settings.svelte";
  import FileIcon from "./FileIcon.svelte";
  import Icon from "./Icon.svelte";
  import CodeView from "./CodeView.svelte";
  import PdfView from "./PdfView.svelte";
  import ZoomImage from "./ZoomImage.svelte";

  // `as`: a way to preview picked for this look ("none" = just the icon).
  let { entry, uri, large = false, as = null }: { entry: Item; uri: string; large?: boolean; as?: PreviewAs | "none" | null } = $props();

  let ext = $derived(extOf(entry.name));
  let kind = $derived(previewKind(entry, as));
  // Picked by the user for an extension we don't know: tell the web view what the bytes are.
  let mime = $derived(mimeFor(entry, kind));
  /** Shown a way the user picked (not the file type's own): say so if it fails. */
  let picked = $derived(kind !== "other" && kind !== builtInKind(entry));
  let src = $derived(fileUrl(uri, mime));

  /** A real HTML file (unlike cx-office's converted HTML) can show as a
   * rendered page or as source. Each file starts the way Settings says. */
  let htmlMode = $state<"preview" | "raw">(settings.data.previewHtml === "code" ? "raw" : "preview");

  let text = $state<TextPreview | null>(null);
  let textError = $state<string | null>(null);
  let children = $state<Entry[] | null>(null);
  let fontFamily = $state<string | null>(null);
  let imageFailed = $state(false);
  let office = $state<OfficeView | null>(null);
  let pdfFailed = $state(false);
  let officeFailed = $state(false);
  let mediaEl = $state<HTMLMediaElement>();
  /** The cursor has rested here long enough to load heavy previews. */
  let settled = $state(false);

  // Every call site keys this component by uri, so one instance ever shows
  // one file: destroy (navigating away, picking a different file, closing
  // Quick Look) always unmounts it. But removing a playing <video>/<audio>
  // from the DOM doesn't reliably stop it — the webview can keep decoding
  // and playing audio from what's already buffered — so pause it explicitly.
  $effect(() => {
    return () => mediaEl?.pause();
  });

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
    htmlMode = settings.data.previewHtml === "code" ? "raw" : "preview";
    settled = false;
    let stale = false;
    let font: FontFace | null = null;
    const start = () => {
      settled = true;
      if (k === "text" || k === "html") {
        previewText(u, large ? 1024 * 1024 : settings.data.previewTextKB * 1024)
          .then((t) => !stale && (text = t))
          .catch(() => !stale && (textError = "No preview available"));
      } else if ((k === "folder" || k === "archive") && settings.data.previewFolders) {
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
        new FontFace(family, `url("${fileUrl(u, mime)}")`)
          .load()
          .then((f) => {
            if (stale) return;
            font = f;
            document.fonts.add(f);
            fontFamily = family;
          })
          .catch(() => !stale && (textError = "No preview available"));
      }
    };
    // The pane and the gallery follow the cursor: wait until it settles so
    // holding an arrow key doesn't read (or list) every file it passes.
    const timer = setTimeout(start, large ? 0 : settings.data.previewDelayMs);
    return () => {
      stale = true;
      clearTimeout(timer);
      if (font) document.fonts.delete(font);
    };
  });

  // Pinch to zoom documents, web pages and Markdown, like images and PDFs.
  let frame = $state<HTMLIFrameElement>();
  let mdBox = $state<HTMLDivElement>();
  let mdBody = $state<HTMLDivElement>();
  let zoomBadge = $state<string | null>(null);
  let badgeTimer = 0;
  function showZoom(z: number) {
    zoomBadge = `${Math.round(z * 100)}%`;
    clearTimeout(badgeTimer);
    badgeTimer = window.setTimeout(() => (zoomBadge = null), 900);
  }
  $effect(() => {
    if (frame) return pinchZoomFrame(frame, { keys: large, forwardKeys: large, onzoom: showZoom });
  });
  $effect(() => {
    if (mdBox && mdBody) return pinchZoom({ events: mdBox, scroller: mdBox, content: mdBody, keys: large, onzoom: showZoom });
  });
  $effect(() => () => clearTimeout(badgeTimer));

  let markdown = $derived((ext === "md" || ext === "markdown") && settings.data.previewMarkdown);
  let html = $derived(text ? (markdown ? renderMarkdown(text.text) : highlight(text.text, ext)) : "");
</script>

<div class="preview" class:large>
  {#if kind === "image" && !imageFailed && large}
    <ZoomImage {src} alt={entry.name} onerror={() => (imageFailed = true)} />
  {:else if kind === "image" && !imageFailed}
    <img src={mime ? src : thumbUrl(uri, 640, entry.modified)} alt={entry.name} onerror={() => (imageFailed = true)} />
  {:else if kind === "video"}
    <!-- svelte-ignore a11y_media_has_caption -->
    <video bind:this={mediaEl} {src} controls autoplay={large && settings.data.previewAutoplay} preload="metadata" poster={thumbUrl(uri, 640, entry.modified)}></video>
  {:else if kind === "audio"}
    <div class="audio">
      <FileIcon name={entry.name} isDir={false} size={96} />
      <audio bind:this={mediaEl} {src} controls autoplay={large && settings.data.previewAutoplay}></audio>
    </div>
  {:else if kind === "pdf" && !pdfFailed}
    <!-- pdf.js, not a thumbnail: a remote PDF has no fast native thumbnail
         (unlike images), so the small preview pane used to just go blank. -->
    <div class="pdfwrap">{#if settled}<PdfView {src} onerror={() => (pdfFailed = true)} />{/if}</div>
  {:else if kind === "pdf"}
    <iframe {src} title={entry.name}></iframe>
  {:else if kind === "office" && office?.kind === "html"}
    <!-- Script-free HTML from cx-office; the sandbox also blocks scripts, forms and
         navigation. allow-same-origin (never with allow-scripts) only lets this
         page reach in to handle pinch zoom. -->
    <iframe bind:this={frame} class="office" class:small={!large} sandbox="allow-same-origin" srcdoc={office.html} title={office.title ?? entry.name}></iframe>
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
  {:else if kind === "html" && text}
    <div class="htmlview">
      {#if htmlMode === "preview"}
        <!-- The page itself, sandboxed: no scripts, forms or navigation — the
             same posture as cx-office's converted-document preview. -->
        <iframe bind:this={frame} sandbox="allow-same-origin" srcdoc={text.text} title={entry.name}></iframe>
      {:else}
        <CodeView text={text.text} {html} lang={ext} {large} onzoom={showZoom} />
      {/if}
      <div class="mode-switch" role="tablist" aria-label="Preview or source">
        <button type="button" role="tab" aria-selected={htmlMode === "preview"} class:active={htmlMode === "preview"} onclick={() => (htmlMode = "preview")}>
          <Icon name="eye" size={12} /> Preview
        </button>
        <button type="button" role="tab" aria-selected={htmlMode === "raw"} class:active={htmlMode === "raw"} onclick={() => (htmlMode = "raw")}>
          <Icon name="code" size={12} /> Code
        </button>
      </div>
    </div>
    {#if text.truncated}<div class="note">Preview shows the beginning of the file</div>{/if}
  {:else if kind === "text" && text}
    {#if markdown}
      <div class="md" bind:this={mdBox}><div bind:this={mdBody}>{@html html}</div></div>
    {:else}
      <CodeView text={text.text} {html} lang={ext} {large} onzoom={showZoom} />
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
      {#if picked && (textError || imageFailed)}<p>This file doesn't open as {previewLabel(kind as PreviewAs)}.</p>
      {:else if textError && large}<p>{textError}</p>{/if}
    </div>
  {/if}
  {#if zoomBadge}<div class="zoom-badge">{zoomBadge}</div>{/if}
</div>

<style>
  .preview {
    position: relative;
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
  /* Wide tables scroll sideways in their own box instead of widening the page. */
  .md :global(.table) {
    overflow-x: auto;
    margin: 0.6em 0;
  }
  .md :global(table) {
    border-collapse: collapse;
    font-size: 12.5px;
    line-height: 1.45;
  }
  .md :global(th),
  .md :global(td) {
    padding: 5px 10px;
    border: 1px solid var(--stroke-strong);
    text-align: left;
    vertical-align: top;
  }
  .md :global(th) {
    font-weight: 600;
    background: var(--hover);
  }
  .md :global(tbody tr:nth-child(even)) {
    background: var(--layer-2);
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
  .zoom-badge {
    position: absolute;
    bottom: 12px;
    left: 50%;
    transform: translateX(-50%);
    padding: 3px 10px;
    border-radius: 999px;
    background: rgb(0 0 0 / 0.6);
    color: #fff;
    font-size: 12px;
    font-variant-numeric: tabular-nums;
    pointer-events: none;
    z-index: 4;
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
  .htmlview {
    position: relative;
    width: 100%;
    height: 100%;
    min-height: 0;
    display: flex;
  }
  .htmlview iframe {
    background: #fff;
  }
  .mode-switch {
    position: absolute;
    top: 8px;
    left: 8px;
    z-index: 3;
    display: flex;
    gap: 2px;
    padding: 2px;
    border-radius: 999px;
    background: var(--flyout, var(--layer-2));
    box-shadow: 0 1px 4px rgb(0 0 0 / 0.2), 0 0 0 1px var(--stroke);
  }
  .mode-switch button {
    display: flex;
    align-items: center;
    gap: 4px;
    height: 22px;
    padding: 0 9px;
    border-radius: 999px;
    color: var(--text-2);
    font-size: 11px;
  }
  .mode-switch button:hover {
    background: var(--hover);
  }
  .mode-switch button.active {
    background: var(--accent);
    color: var(--accent-text);
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
