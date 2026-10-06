// Pinch to zoom for previews drawn as a page rather than a picture: office
// documents and web pages (inside their sandboxed iframe), Markdown and
// code. Trackpad pinch arrives as WebKit gesture events (macOS, Linux) or as
// ctrl+wheel (Chromium/WebView2); ⌘/Ctrl+scroll and +/−/0 in Quick Look do
// the same. The content gets CSS zoom, so it stays real text: sharp at any
// size, selectable, and scrolled natively.

const MIN = 0.5;
const MAX = 5;

type GestureEvent = UIEvent & { scale: number; clientX: number; clientY: number };

export interface PinchZoom {
  /** Where the gestures land: the scrolling element, or an iframe's document. */
  events: HTMLElement | Document;
  /** What scrolls (an iframe's root scroller included). */
  scroller: Element;
  /** What gets zoomed: inside `scroller`, so the scroll range follows. */
  content: HTMLElement;
  /** Also +/−/0 from the keyboard (Quick Look). */
  keys?: boolean;
  /** The element on this page that holds the preview (the iframe), for the focus check. */
  host?: Element;
  onzoom?: (zoom: number) => void;
}

const isDocument = (t: HTMLElement | Document): t is Document => t.nodeType === 9;

export function pinchZoom({ events, scroller, content, keys = false, host = scroller, onzoom }: PinchZoom): () => void {
  let zoom = Number(content.style.zoom) || 1;
  let gestureBase = 1;
  let gesturing = false;
  const doc = scroller.ownerDocument;
  const win = doc.defaultView ?? window;

  /** Zoom to `next`, keeping the point under (x, y) where it is. */
  function zoomTo(next: number, x?: number, y?: number) {
    next = Math.min(MAX, Math.max(MIN, next));
    if (Math.abs(next - zoom) < 1e-3) return;
    const view = scroller === doc.scrollingElement ? { left: 0, top: 0, width: win.innerWidth, height: win.innerHeight } : scroller.getBoundingClientRect();
    const px = x === undefined ? view.width / 2 : x - view.left;
    const py = y === undefined ? view.height / 2 : y - view.top;
    const k = next / zoom;
    const { scrollLeft, scrollTop } = scroller;
    zoom = next;
    content.style.zoom = String(next);
    scroller.scrollLeft = (scrollLeft + px) * k - px;
    scroller.scrollTop = (scrollTop + py) * k - py;
    onzoom?.(zoom);
  }

  // Non-passive, so the web view itself doesn't zoom or scroll instead.
  const wheel = (e: WheelEvent) => {
    if (!(e.ctrlKey || e.metaKey)) return; // a plain scroll pans, natively
    e.preventDefault();
    if (gesturing) return;
    const unit = e.deltaMode === 1 ? 16 : 1;
    zoomTo(zoom * Math.exp((-e.deltaY * unit) / 100), e.clientX, e.clientY);
  };
  const gstart = (e: Event) => {
    e.preventDefault();
    gesturing = true;
    gestureBase = zoom;
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

  // Keys come from the window while Quick Look (which holds this preview)
  // has focus, and from the iframe's own document once that has it.
  const inside = isDocument(events) ? events : null;
  const key = (e: Event) => {
    const k = e as KeyboardEvent;
    if (k.metaKey || k.ctrlKey || k.altKey) return;
    if (e.currentTarget === window && !(document.activeElement as HTMLElement | null)?.closest?.(".ql")?.contains(host)) return;
    if (k.key === "+" || k.key === "=") zoomTo(zoom * 1.25);
    else if (k.key === "-" || k.key === "_") zoomTo(zoom / 1.25);
    else if (k.key === "0") zoomTo(1);
    else return;
    k.preventDefault();
    k.stopPropagation();
  };

  events.addEventListener("wheel", wheel as EventListener, { passive: false });
  events.addEventListener("gesturestart", gstart, { passive: false });
  events.addEventListener("gesturechange", gchange, { passive: false });
  events.addEventListener("gestureend", gend, { passive: false });
  if (keys) {
    window.addEventListener("keydown", key, true);
    inside?.addEventListener("keydown", key, true);
  }
  return () => {
    events.removeEventListener("wheel", wheel as EventListener);
    events.removeEventListener("gesturestart", gstart);
    events.removeEventListener("gesturechange", gchange);
    events.removeEventListener("gestureend", gend);
    window.removeEventListener("keydown", key, true);
    inside?.removeEventListener("keydown", key, true);
  };
}

/** Quick Look's own keys, sent on from inside a focused iframe. */
const QL_KEYS = new Set(["Escape", " ", "F3", "Enter", "ArrowLeft", "ArrowRight", "ArrowUp", "ArrowDown", "Home", "End"]);

/**
 * Pinch zoom inside an iframe's document, reattached on every load. The
 * iframe needs `sandbox="allow-same-origin"` (and still no `allow-scripts`)
 * so this page can reach in; the document itself still runs nothing.
 * `forwardKeys`: hand Quick Look's keys (close, next file…) back to it,
 * since a focused iframe otherwise keeps every key press to itself.
 */
export function pinchZoomFrame(iframe: HTMLIFrameElement, opts: { keys?: boolean; forwardKeys?: boolean; onzoom?: (zoom: number) => void }): () => void {
  let detach = () => {};
  const attach = () => {
    detach();
    const doc = iframe.contentDocument;
    if (!doc?.body || !doc.scrollingElement) return; // not loaded, or navigated off to another origin
    const stop = pinchZoom({ events: doc, scroller: doc.scrollingElement, content: doc.body, keys: opts.keys, host: iframe, onzoom: opts.onzoom });
    const forward = (e: KeyboardEvent) => {
      if (e.defaultPrevented || e.metaKey || e.ctrlKey || e.altKey || !QL_KEYS.has(e.key)) return;
      const copy = new KeyboardEvent("keydown", { key: e.key, code: e.code, shiftKey: e.shiftKey, repeat: e.repeat, bubbles: true, cancelable: true });
      if (!iframe.dispatchEvent(copy)) e.preventDefault();
    };
    if (opts.forwardKeys) doc.addEventListener("keydown", forward);
    detach = () => {
      stop();
      doc.removeEventListener("keydown", forward);
    };
  };
  iframe.addEventListener("load", attach);
  attach();
  return () => {
    iframe.removeEventListener("load", attach);
    detach();
  };
}
