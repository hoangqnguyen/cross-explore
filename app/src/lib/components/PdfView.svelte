<script lang="ts">
  // PDF viewer for Quick Look, drawn with pdf.js so trackpad gestures work:
  // pinch to zoom (WebKit gesture events / ctrl+wheel), two-finger scroll to
  // pan, drag to pan when zoomed, double-click to zoom, +/−/0 keys, touch
  // pinch on phones. Pages render lazily and re-render sharp after zooming.
  import { onDestroy, tick } from "svelte";
  import type { PDFDocumentLoadingTask, PDFDocumentProxy, PDFPageProxy, RenderTask } from "pdfjs-dist";

  let { src, onerror }: { src: string; onerror?: () => void } = $props();

  const MIN = 0.25;
  const MAX = 8;
  const GAP = 12;
  /** WebKit refuses canvases larger than this many pixels. */
  const MAX_PIXELS = 16_000_000;

  let scroller = $state<HTMLDivElement>();
  let content = $state<HTMLDivElement>();
  let sizes = $state<{ w: number; h: number }[]>([]);
  let fitWidth = $state(600);
  let zoom = $state(1);
  let current = $state(1);
  let badge = $state<string | null>(null);
  let failed = $state(false);

  let doc: PDFDocumentProxy | null = null;
  let task: PDFDocumentLoadingTask | null = null;
  const pages = new Map<number, PDFPageProxy>();
  const rendered = new Map<number, number>(); // page → scale it was drawn at
  const renders = new Map<number, RenderTask>();
  const visible = new Set<number>();
  let canvases: HTMLCanvasElement[] = $state([]);
  let observer: IntersectionObserver | null = null;
  let settle = 0;
  let badgeTimer = 0;

  /** CSS pixels per PDF point at the current zoom (1 = the widest page fits). */
  let base = $derived(sizes.length ? fitWidth / Math.max(...sizes.map((s) => s.w)) : 1);
  let scale = $derived(base * zoom);

  async function load(url: string) {
    const pdfjs = await import("pdfjs-dist");
    if (!pdfjs.GlobalWorkerOptions.workerSrc) {
      pdfjs.GlobalWorkerOptions.workerSrc = (await import("pdfjs-dist/build/pdf.worker.min.mjs?url")).default;
    }
    const asset = (dir: string) => new URL(`/pdfjs/${dir}/`, location.href).href;
    task = pdfjs.getDocument({
      url,
      cMapUrl: asset("cmaps"),
      cMapPacked: true,
      standardFontDataUrl: asset("standard_fonts"),
      wasmUrl: asset("wasm"),
      iccUrl: asset("iccs"),
    });
    doc = await task.promise;
    const first = await doc.getPage(1);
    pages.set(1, first);
    const v = first.getViewport({ scale: 1 });
    // Assume every page matches the first until each is measured.
    sizes = Array.from({ length: doc.numPages }, () => ({ w: v.width, h: v.height }));
    await tick();
    observe();
    for (let n = 2; n <= doc.numPages && doc; n++) {
      const p = await doc.getPage(n);
      pages.set(n, p);
      const pv = p.getViewport({ scale: 1 });
      if (Math.abs(pv.width - sizes[n - 1].w) > 0.5 || Math.abs(pv.height - sizes[n - 1].h) > 0.5) sizes[n - 1] = { w: pv.width, h: pv.height };
    }
  }

  function measure() {
    if (scroller) fitWidth = Math.max(100, scroller.clientWidth - 2 * GAP);
  }

  function observe() {
    observer?.disconnect();
    if (!scroller) return;
    observer = new IntersectionObserver(
      (items) => {
        for (const it of items) {
          const n = Number((it.target as HTMLElement).dataset.page);
          if (it.isIntersecting) visible.add(n);
          else visible.delete(n);
        }
        drawVisible();
        updateCurrent();
      },
      { root: scroller, rootMargin: "400px 0px" },
    );
    for (const c of canvases) if (c) observer.observe(c);
  }

  function drawVisible() {
    for (const n of visible) void draw(n);
  }

  async function draw(n: number) {
    const target = scale * devicePixelRatio;
    if (rendered.get(n) === target) return;
    const page = pages.get(n) ?? (await doc?.getPage(n));
    const canvas = canvases[n - 1];
    if (!page || !canvas || !doc) return;
    pages.set(n, page);
    renders.get(n)?.cancel();
    let viewport = page.getViewport({ scale: target });
    const px = viewport.width * viewport.height;
    if (px > MAX_PIXELS) viewport = page.getViewport({ scale: target * Math.sqrt(MAX_PIXELS / px) });
    // Draw off-screen so the old (blurry) page stays up until the sharp one is ready.
    const off = document.createElement("canvas");
    off.width = Math.floor(viewport.width);
    off.height = Math.floor(viewport.height);
    const job = page.render({ canvas: off, viewport });
    renders.set(n, job);
    try {
      await job.promise;
    } catch {
      return; // cancelled by a newer zoom
    }
    if (renders.get(n) !== job) return;
    renders.delete(n);
    canvas.width = off.width;
    canvas.height = off.height;
    canvas.getContext("2d")?.drawImage(off, 0, 0);
    rendered.set(n, target);
  }

  function updateCurrent() {
    if (!scroller) return;
    const mid = scroller.getBoundingClientRect().top + scroller.clientHeight / 3;
    for (let i = 0; i < canvases.length; i++) {
      const r = canvases[i]?.getBoundingClientRect();
      if (r && r.bottom >= mid) {
        current = i + 1;
        return;
      }
    }
  }

  function showBadge(text: string) {
    badge = text;
    clearTimeout(badgeTimer);
    badgeTimer = window.setTimeout(() => (badge = null), 900);
  }

  /** Zoom to `next`, keeping the document point under (cx, cy) in place. */
  async function zoomTo(next: number, cx?: number, cy?: number) {
    if (!scroller || !content) return;
    next = Math.min(MAX, Math.max(MIN, next));
    if (Math.abs(next - zoom) < 1e-4) return;
    const view = scroller.getBoundingClientRect();
    const x = cx ?? view.left + view.width / 2;
    const y = cy ?? view.top + view.height / 2;
    const before = content.getBoundingClientRect();
    const k = next / zoom;
    const px = (x - before.left) * k;
    const py = (y - before.top) * k;
    zoom = next;
    await tick();
    const after = content.getBoundingClientRect();
    scroller.scrollLeft += after.left + px - x;
    scroller.scrollTop += after.top + py - y;
    showBadge(`${Math.round(zoom * 100)}%`);
    // Stretch now, redraw sharp once the gesture settles.
    clearTimeout(settle);
    settle = window.setTimeout(drawVisible, 140);
  }

  $effect(() => {
    const url = src;
    failed = false;
    load(url).catch((e) => {
      console.warn("pdf preview", e);
      failed = true;
      onerror?.();
    });
    return () => void teardown();
  });

  async function teardown() {
    observer?.disconnect();
    for (const r of renders.values()) r.cancel();
    renders.clear();
    rendered.clear();
    pages.clear();
    visible.clear();
    const t = task;
    task = null;
    doc = null;
    await t?.destroy();
  }
  onDestroy(() => clearTimeout(settle));

  // Re-fit when the window resizes.
  $effect(() => {
    const el = scroller;
    if (!el) return;
    measure();
    const ro = new ResizeObserver(() => {
      measure();
      clearTimeout(settle);
      settle = window.setTimeout(drawVisible, 140);
    });
    ro.observe(el);
    return () => ro.disconnect();
  });

  // Gesture listeners must be non-passive so the window itself doesn't zoom.
  $effect(() => {
    const el = scroller;
    if (!el) return;
    let base = 1;
    let gesturing = false;
    const wheel = (e: WheelEvent) => {
      if (!(e.ctrlKey || e.metaKey)) return; // plain scroll = native panning
      e.preventDefault();
      if (gesturing) return;
      const unit = e.deltaMode === 1 ? 16 : 1;
      void zoomTo(zoom * Math.exp((-e.deltaY * unit) / 100), e.clientX, e.clientY);
    };
    type GestureEvent = UIEvent & { scale: number; clientX: number; clientY: number };
    const gstart = (e: Event) => {
      e.preventDefault();
      gesturing = true;
      base = zoom;
    };
    const gchange = (e: Event) => {
      e.preventDefault();
      const g = e as GestureEvent;
      void zoomTo(base * g.scale, g.clientX, g.clientY);
    };
    const gend = (e: Event) => {
      e.preventDefault();
      gesturing = false;
    };
    el.addEventListener("wheel", wheel, { passive: false });
    el.addEventListener("gesturestart", gstart, { passive: false });
    el.addEventListener("gesturechange", gchange, { passive: false });
    el.addEventListener("gestureend", gend, { passive: false });
    return () => {
      el.removeEventListener("wheel", wheel);
      el.removeEventListener("gesturestart", gstart);
      el.removeEventListener("gesturechange", gchange);
      el.removeEventListener("gestureend", gend);
    };
  });

  // Mouse: drag to pan. Touch: two-finger pinch (one finger scrolls natively).
  const pointers = new Map<number, { x: number; y: number }>();
  let pinch: { dist: number; zoom: number } | null = null;
  let dragging = $state(false);

  function spread() {
    const [a, b] = [...pointers.values()];
    return { dist: Math.hypot(a.x - b.x, a.y - b.y), cx: (a.x + b.x) / 2, cy: (a.y + b.y) / 2 };
  }
  function onpointerdown(e: PointerEvent) {
    if (e.button !== 0) return;
    pointers.set(e.pointerId, { x: e.clientX, y: e.clientY });
    if (e.pointerType === "mouse") {
      dragging = true;
      scroller?.setPointerCapture(e.pointerId);
    } else if (pointers.size === 2) {
      pinch = { dist: spread().dist, zoom };
    }
  }
  function onpointermove(e: PointerEvent) {
    const prev = pointers.get(e.pointerId);
    if (!prev || !scroller) return;
    pointers.set(e.pointerId, { x: e.clientX, y: e.clientY });
    if (e.pointerType === "mouse" && dragging) {
      scroller.scrollLeft -= e.clientX - prev.x;
      scroller.scrollTop -= e.clientY - prev.y;
    } else if (pinch && pointers.size === 2) {
      const s = spread();
      void zoomTo((pinch.zoom * s.dist) / pinch.dist, s.cx, s.cy);
    }
  }
  function onpointerup(e: PointerEvent) {
    pointers.delete(e.pointerId);
    if (pointers.size < 2) pinch = null;
    if (e.pointerType === "mouse") dragging = false;
  }

  function ondblclick(e: MouseEvent) {
    void zoomTo(zoom > 1.05 ? 1 : 2, e.clientX, e.clientY);
  }

  // +/−/0 while Quick Look has focus.
  $effect(() => {
    const key = (e: KeyboardEvent) => {
      if (!scroller?.closest(".ql") || !(document.activeElement as HTMLElement | null)?.closest?.(".ql")) return;
      if (e.key === "+" || e.key === "=") void zoomTo(zoom * 1.25);
      else if (e.key === "-" || e.key === "_") void zoomTo(zoom / 1.25);
      else if (e.key === "0") void zoomTo(1);
      else return;
      e.preventDefault();
      e.stopPropagation();
    };
    window.addEventListener("keydown", key, true);
    return () => window.removeEventListener("keydown", key, true);
  });

  // New canvases (page count known) need observing.
  $effect(() => {
    void sizes.length;
    void canvases.length;
    tick().then(observe);
  });
</script>

<!-- svelte-ignore a11y_no_static_element_interactions -->
<div
  class="pdf"
  class:dragging
  bind:this={scroller}
  onscroll={updateCurrent}
  {onpointerdown}
  {onpointermove}
  {onpointerup}
  onpointercancel={onpointerup}
  {ondblclick}
>
  <div class="pages" bind:this={content} style:padding="{GAP}px" style:gap="{GAP}px">
    {#each sizes as s, i (i)}
      <canvas bind:this={canvases[i]} data-page={i + 1} style:width="{s.w * scale}px" style:height="{s.h * scale}px"></canvas>
    {/each}
  </div>
</div>
{#if failed}<div class="note">Can't show this PDF</div>{/if}
{#if sizes.length > 1}<div class="pageno">{current} / {sizes.length}</div>{/if}
{#if badge}<div class="badge">{badge}</div>{/if}

<style>
  .pdf {
    position: absolute;
    inset: 0;
    overflow: auto;
    touch-action: pan-x pan-y;
    overscroll-behavior: contain;
    cursor: grab;
  }
  .pdf.dragging {
    cursor: grabbing;
  }
  .pages {
    display: flex;
    flex-direction: column;
    align-items: center;
    width: max-content;
    min-width: 100%;
    box-sizing: border-box;
  }
  canvas {
    display: block;
    flex: none;
    background: #fff;
    box-shadow: 0 1px 4px rgb(0 0 0 / 0.25);
  }
  .pageno,
  .badge,
  .note {
    position: absolute;
    bottom: 12px;
    padding: 3px 10px;
    border-radius: 999px;
    background: rgb(0 0 0 / 0.6);
    color: #fff;
    font-size: 12px;
    font-variant-numeric: tabular-nums;
    pointer-events: none;
  }
  .pageno {
    right: 16px;
  }
  .badge,
  .note {
    left: 50%;
    transform: translateX(-50%);
  }
</style>
