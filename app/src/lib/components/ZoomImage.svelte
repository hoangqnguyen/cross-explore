<script lang="ts">
  // An image you can zoom and pan: trackpad pinch (WebKit gesture events or
  // Chromium ctrl+wheel), two-finger scroll to pan, drag to pan, double-click
  // to toggle 1:1, touch pinch on phones, and +/−/0 keys inside Quick Look.
  let { src, alt, onerror }: { src: string; alt: string; onerror?: () => void } = $props();

  const MIN = 1;
  const MAX = 16;

  let box = $state<HTMLDivElement>();
  let img = $state<HTMLImageElement>();
  let scale = $state(1);
  let x = $state(0);
  let y = $state(0);
  let animate = $state(false);
  let badge = $state(false);
  let badgeTimer = 0;

  let zoomed = $derived(scale > 1.001);

  /** The image's fitted (scale 1) size inside the box. */
  function fitted() {
    const b = box!.getBoundingClientRect();
    const nw = img?.naturalWidth || b.width;
    const nh = img?.naturalHeight || b.height;
    const f = Math.min(1, b.width / nw, b.height / nh);
    return { bw: b.width, bh: b.height, w: nw * f, h: nh * f, natural: 1 / f };
  }

  /** Keep the image covering the box (no drifting off into empty space). */
  function clamp() {
    if (!box) return;
    const { bw, bh, w, h } = fitted();
    const mx = Math.max(0, (w * scale - bw) / 2);
    const my = Math.max(0, (h * scale - bh) / 2);
    x = Math.min(mx, Math.max(-mx, x));
    y = Math.min(my, Math.max(-my, y));
  }

  /** Zoom to `next`, keeping the point under (cx, cy) (client coords) fixed. */
  function zoomTo(next: number, cx?: number, cy?: number, smooth = false) {
    if (!box) return;
    next = Math.min(MAX, Math.max(MIN, next));
    const b = box.getBoundingClientRect();
    const px = (cx ?? b.left + b.width / 2) - (b.left + b.width / 2);
    const py = (cy ?? b.top + b.height / 2) - (b.top + b.height / 2);
    const k = next / scale;
    x = px - (px - x) * k;
    y = py - (py - y) * k;
    scale = next;
    animate = smooth;
    clamp();
    showBadge();
  }

  function reset(smooth = true) {
    animate = smooth;
    scale = 1;
    x = 0;
    y = 0;
    showBadge();
  }

  function showBadge() {
    badge = true;
    clearTimeout(badgeTimer);
    badgeTimer = window.setTimeout(() => (badge = false), 900);
  }

  // A new picture starts fitted.
  $effect(() => {
    void src;
    scale = 1;
    x = 0;
    y = 0;
  });

  // Listeners need passive: false so the page itself doesn't zoom or scroll.
  $effect(() => {
    const el = box;
    if (!el) return;
    let gestureBase = 1;
    let gesturing = false;

    const wheel = (e: WheelEvent) => {
      if (e.ctrlKey || e.metaKey) {
        // Pinch on a trackpad arrives as ctrl+wheel in Chromium/WebView2.
        e.preventDefault();
        if (gesturing) return;
        const unit = e.deltaMode === 1 ? 16 : 1;
        zoomTo(scale * Math.exp((-e.deltaY * unit) / 100), e.clientX, e.clientY);
      } else if (zoomed) {
        e.preventDefault();
        e.stopPropagation();
        animate = false;
        x -= e.deltaX;
        y -= e.deltaY;
        clamp();
      }
    };
    // Safari / WKWebView trackpad pinch.
    type GestureEvent = UIEvent & { scale: number; clientX: number; clientY: number };
    const gstart = (e: Event) => {
      e.preventDefault();
      gesturing = true;
      gestureBase = scale;
    };
    const gchange = (e: Event) => {
      e.preventDefault();
      const g = e as GestureEvent;
      zoomTo(gestureBase * g.scale, g.clientX, g.clientY);
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

  // Drag to pan (mouse) and one/two-finger pan + pinch (touch).
  const pointers = new Map<number, { x: number; y: number }>();
  let pinchStart: { dist: number; scale: number } | null = null;

  function spread() {
    const [a, b] = [...pointers.values()];
    return { dist: Math.hypot(a.x - b.x, a.y - b.y), cx: (a.x + b.x) / 2, cy: (a.y + b.y) / 2 };
  }

  function onpointerdown(e: PointerEvent) {
    if (e.button !== 0) return;
    if (e.pointerType === "mouse" && !zoomed) return;
    box?.setPointerCapture(e.pointerId);
    pointers.set(e.pointerId, { x: e.clientX, y: e.clientY });
    if (pointers.size === 2) pinchStart = { dist: spread().dist, scale };
    e.preventDefault();
  }
  function onpointermove(e: PointerEvent) {
    const prev = pointers.get(e.pointerId);
    if (!prev) return;
    const before = pointers.size === 2 ? spread() : null;
    pointers.set(e.pointerId, { x: e.clientX, y: e.clientY });
    animate = false;
    if (pointers.size === 2 && pinchStart && before) {
      const now = spread();
      x += now.cx - before.cx;
      y += now.cy - before.cy;
      zoomTo((pinchStart.scale * now.dist) / pinchStart.dist, now.cx, now.cy);
    } else if (pointers.size === 1 && zoomed) {
      x += e.clientX - prev.x;
      y += e.clientY - prev.y;
      clamp();
    }
  }
  function onpointerup(e: PointerEvent) {
    pointers.delete(e.pointerId);
    if (pointers.size < 2) pinchStart = null;
    if (scale < 1.02 && pointers.size === 0 && (x || y)) reset();
  }

  function ondblclick(e: MouseEvent) {
    if (zoomed) reset();
    else zoomTo(Math.max(2, fitted().natural), e.clientX, e.clientY, true);
  }

  // Keys only while Quick Look has focus, so +/- keep their list meaning elsewhere.
  $effect(() => {
    const key = (e: KeyboardEvent) => {
      if (!box || !(document.activeElement as HTMLElement | null)?.closest?.(".ql")) return;
      if (!box.closest(".ql")) return;
      const k = e.key;
      if (k === "+" || k === "=") zoomTo(scale * 1.25, undefined, undefined, true);
      else if (k === "-" || k === "_") zoomTo(scale / 1.25, undefined, undefined, true);
      else if (k === "0") reset();
      else if (k === "1" && !e.metaKey && !e.ctrlKey) zoomTo(fitted().natural, undefined, undefined, true);
      else return;
      e.preventDefault();
      e.stopPropagation();
    };
    window.addEventListener("keydown", key, true);
    return () => window.removeEventListener("keydown", key, true);
  });
</script>

<!-- svelte-ignore a11y_no_static_element_interactions -->
<div
  class="zoom"
  class:zoomed
  bind:this={box}
  {onpointerdown}
  {onpointermove}
  {onpointerup}
  onpointercancel={onpointerup}
  {ondblclick}
>
  <img
    bind:this={img}
    {src}
    {alt}
    {onerror}
    draggable="false"
    class:animate
    style:transform="translate({x}px, {y}px) scale({scale})"
    ontransitionend={() => (animate = false)}
  />
  {#if badge}<div class="badge">{Math.round(scale * 100)}%</div>{/if}
</div>

<style>
  .zoom {
    position: relative;
    width: 100%;
    height: 100%;
    display: flex;
    align-items: center;
    justify-content: center;
    overflow: hidden;
    touch-action: none;
    cursor: zoom-in;
  }
  .zoom.zoomed {
    cursor: grab;
  }
  .zoom.zoomed:active {
    cursor: grabbing;
  }
  img {
    max-width: 100%;
    max-height: 100%;
    object-fit: contain;
    border-radius: 6px;
    transform-origin: center center;
    will-change: transform;
    user-select: none;
    -webkit-user-drag: none;
  }
  .zoomed img {
    border-radius: 0;
  }
  img.animate {
    transition: transform 160ms ease-out;
  }
  .badge {
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
  }
</style>
