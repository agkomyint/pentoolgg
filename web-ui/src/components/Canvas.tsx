import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import { Maximize2, Minus, Plus } from "lucide-react";
import { api } from "@/lib/api";
import { Superseded } from "@/lib/wasmCore";
import { dragLayers } from "@/lib/dragSprite";
import { useStudio } from "@/store";
import { Button } from "@/components/ui/button";
import { LayoutOverlay } from "@/workspaces/layout/LayoutOverlay";
import { RemoteActivity } from "@/components/Presence";
import { PaintOverlay } from "@/workspaces/PaintOverlay";
import type { RasterTool } from "@/workspaces/rasterStore";
import { startPainting } from "@/workspaces/paintLayer";
import { PHOTO_DRAG_TYPE, usePhoto } from "@/workspaces/photoStore";
import type { Doc } from "@/lib/api";
import { useLayout } from "@/workspaces/layout/layoutStore";
import {
  addObject,
  addImage,
  moveNodesBy,
  updateNode,
  transformNodesPage,
  duplicateSelectionBy,
  selectionRoots,
  copySelectionText,
  pasteSelectionText,
  deleteSelected,
  duplicateSelected,
  groupSelected,
  ungroupSelected,
  reorderSelected,
  selectAllOnPage,
  canvasView,
} from "@/lib/actions";
import {
  apply,
  findNode,
  hitChain,
  nodeKind,
  pickFromChain,
  isScene,
  multiply,
  nodeMatrix,
  pageBounds,
  pageOf,
  penPathData,
  topLevelIn,
  unite,
  type Matrix,
  type PenPoint,
  type Rect,
} from "@/lib/model";

type Point = { x: number; y: number };
type Handle = "nw" | "n" | "ne" | "e" | "se" | "s" | "sw" | "w";
const HANDLES: Handle[] = ["nw", "n", "ne", "e", "se", "s", "sw", "w"];

type Drag =
  | { mode: "move"; start: Point; ids: string[]; box: Rect | null; snaps: Snaps }
  | { mode: "resize"; start: Point; ids: string[]; handle: Handle; box: Rect }
  | { mode: "marquee"; start: Point; additive: boolean }
  | { mode: "create"; start: Point }
  | { mode: "pen"; index: number; start: Point };

/** Edges and centres other objects and the page offer to a dragged selection. */
type Snaps = { xs: number[]; ys: number[] };
type Guide = { axis: "x" | "y"; at: number; from: number; to: number };

function snapTargets(page: ReturnType<typeof pageOf>, skip: Set<string>, canvas: { width: number; height: number }): Snaps {
  const xs = [0, canvas.width / 2, canvas.width];
  const ys = [0, canvas.height / 2, canvas.height];
  for (const id of topLevelIn(page, { x: -1e9, y: -1e9, width: 2e9, height: 2e9 })) {
    if (skip.has(id)) continue;
    const f = findNode(page, id);
    const b = f ? pageBounds(f) : null;
    if (!b) continue;
    xs.push(b.x, b.x + b.width / 2, b.x + b.width);
    ys.push(b.y, b.y + b.height / 2, b.y + b.height);
  }
  return { xs, ys };
}

/** Nudge a moved box onto the nearest edge or centre within `tol`; returns the correction and guides. */
function snapBox(box: Rect, snaps: Snaps, tol: number): { dx: number; dy: number; guides: Guide[] } {
  const best = (mine: number[], targets: number[]) => {
    let pick: { d: number; at: number } | null = null;
    for (const m of mine) for (const t of targets) {
      const d = t - m;
      if (Math.abs(d) <= tol && (!pick || Math.abs(d) < Math.abs(pick.d))) pick = { d, at: t };
    }
    return pick;
  };
  const sx = best([box.x, box.x + box.width / 2, box.x + box.width], snaps.xs);
  const sy = best([box.y, box.y + box.height / 2, box.y + box.height], snaps.ys);
  const guides: Guide[] = [];
  const top = box.y + (sy?.d ?? 0);
  const left = box.x + (sx?.d ?? 0);
  if (sx) guides.push({ axis: "x", at: sx.at, from: top, to: top + box.height });
  if (sy) guides.push({ axis: "y", at: sy.at, from: left, to: left + box.width });
  return { dx: sx?.d ?? 0, dy: sy?.d ?? 0, guides };
}

/** Preview sizes the server is asked for; bucketing keeps zooming from re-rendering on every step. */
const PREVIEW_EDGES = [512, 768, 1024, 1536, 2048, 3072, 4096, 6144, 8192];
/** Canvas margin: the page sits in a padded box (`p-8`) centred in the scroller. */
const MARGIN = 32;

function previewEdge(cssEdge: number): number {
  const want = Math.ceil(cssEdge * (window.devicePixelRatio || 1));
  return PREVIEW_EDGES.find((edge) => edge >= want) ?? PREVIEW_EDGES.at(-1)!;
}

/** Offset of the page inside the scroll content for a given scroller and page size. */
function pageOffset(view: number, size: number): number {
  return (Math.max(view, size + MARGIN * 2) - size) / 2;
}

function handlePoint(box: Rect, h: Handle): Point {
  const x = h.includes("w") ? box.x : h.includes("e") ? box.x + box.width : box.x + box.width / 2;
  const y = h.includes("n") ? box.y : h.includes("s") ? box.y + box.height : box.y + box.height / 2;
  return { x, y };
}

function resizedBox(box: Rect, h: Handle, p: Point, keepAspect: boolean): Rect {
  let left = box.x;
  let top = box.y;
  let right = box.x + box.width;
  let bottom = box.y + box.height;
  if (h.includes("w")) left = Math.min(p.x, right - 1);
  if (h.includes("e")) right = Math.max(p.x, left + 1);
  if (h.includes("n")) top = Math.min(p.y, bottom - 1);
  if (h.includes("s")) bottom = Math.max(p.y, top + 1);
  if (keepAspect && box.width > 0 && box.height > 0) {
    const ratio = box.width / box.height;
    const w = right - left;
    const hgt = bottom - top;
    if (w / hgt > ratio) {
      const next = w / ratio;
      if (h.includes("n")) top = bottom - next;
      else bottom = top + next;
    } else {
      const next = hgt * ratio;
      if (h.includes("w")) left = right - next;
      else right = left + next;
    }
  }
  return { x: left, y: top, width: right - left, height: bottom - top };
}

/** Interactive page: Rust-rendered preview under an SVG layer for selection and drawing. */
export function Canvas({ dragToUv = false, active = true }: { dragToUv?: boolean; active?: boolean } = {}) {
  const s = useStudio();
  const { doc, preview, pageId, view, zoom, notify, tool, selectedId, layerId, paintMode } = s;
  const [url, setUrl] = useState<string | null>(null);
  // The document behind the image now on screen; paint strokes wait for it.
  const [rendered, setRendered] = useState<{ url: string; doc: Doc } | null>(null);
  const [renderedDoc, setRenderedDoc] = useState<Doc | null>(null);
  // Zoom keeps a page point under a viewport point; applied once the new size is laid out.
  const anchor = useRef<{ px: number; py: number; cx: number; cy: number } | null>(null);
  const [loading, setLoading] = useState(false);
  const [box, setBox] = useState({ w: 800, h: 600 });
  const [draft, setDraft] = useState<PenPoint[]>([]);
  const [hover, setHover] = useState<Point | null>(null);
  const [ghost, setGhost] = useState<Rect | null>(null);
  const [live, setLive] = useState<Matrix | null>(null);
  const [marquee, setMarquee] = useState<Rect | null>(null);
  const hoverId = useStudio((st) => st.hoverId);
  const setHoverId = useStudio((st) => st.setHover);
  const [guides, setGuides] = useState<Guide[]>([]);
  const [menu, setMenu] = useState<{ x: number; y: number } | null>(null);
  // Text being edited in place; `fresh` text that ends up empty is removed.
  const [textEdit, setTextEdit] = useState<{ id: string; fresh: boolean } | null>(null);
  // Last clipboard write, for the context menu's Paste (reading the system clipboard needs permission).
  const lastCopy = useRef<string | null>(null);
  const drag = useRef<Drag | null>(null);
  const scroller = useRef<HTMLDivElement>(null);
  const scrollPosition = useRef({ left: 0, top: 0 });
  useLayoutEffect(() => {
    if (!active) return;
    const frame = requestAnimationFrame(() => scroller.current?.scrollTo(scrollPosition.current.left, scrollPosition.current.top));
    return () => cancelAnimationFrame(frame);
  }, [active]);
  const svg = useRef<SVGSVGElement>(null);
  const token = useRef(0);
  // While an object is dragged, the page is shown as two server-rendered
  // layers (backdrop without the object, object alone) so the object follows
  // the pointer instantly. `rest` keeps the final offset until the real render lands.
  const [sprite, setSprite] = useState<{ backdrop: string; object: string; id: string } | null>(null);
  const [rest, setRest] = useState<Point | null>(null);
  const spriteToken = useRef(0);
  const spaceDown = useRef(false);
  const facePickPending = useRef(false);
  const facePreviewToken = useRef(0);
  const [faceHighlight, setFaceHighlight] = useState<{ node: string; panel: string; png: string } | null>(null);

  const page = pageOf(doc, pageId);
  const flatDielineId = page?.dieline_layout?.dieline;
  const flatDieline = flatDielineId && doc ? (doc.dielines as Record<string, { scene3d_node?: string; panels?: { id: string }[] }> | undefined)?.[flatDielineId] : undefined;
  const packageNodeId = flatDieline?.scene3d_node;
  const packageLocation = packageNodeId && doc?.pages
    ? doc.pages.map((candidate) => ({ page: candidate, found: findNode(candidate, packageNodeId) })).find((item) => item.found?.node.kind === "scene3d" && item.found.node.dieline === flatDielineId)
    : undefined;
  const canvas = page?.canvas ?? { width: 1200, height: 800 };
  const fit = Math.min((box.w - MARGIN * 2) / canvas.width, (box.h - MARGIN * 2) / canvas.height, 4);
  const scale = Math.max(0.02, fit * zoom);
  const W = canvas.width * scale;
  const H = canvas.height * scale;
  const edge = previewEdge(Math.max(W, H));
  const lastDoc = useRef<{ doc: Doc | null; page: string | undefined }>({ doc: null, page: undefined });
  const setScale = s.setScale;
  useEffect(() => {
    if (active) setScale(scale);
  }, [scale, active, setScale]);
  const found = useMemo(() => findNode(page, selectedId), [page, selectedId]);
  const selectedBox = useMemo(() => (found ? pageBounds(found) : null), [found]);
  const selectedIds = s.selectedIds;
  // Handles wrap every selected object; `selectedBox` stays the primary object's box.
  const selectionBox = useMemo(() => {
    if (selectedIds.length < 2) return selectedBox;
    let union: Rect | null = null;
    for (const id of selectedIds) {
      const f = findNode(page, id);
      const b = f ? pageBounds(f) : null;
      if (b) union = unite(union, b);
    }
    return union;
  }, [selectedIds, selectedBox, page]);
  const selectedPackagePanel = s.selectedPackagePanel;
  const selectedDielineNode = s.workspaceId === "3d" && found?.node.kind === "scene3d" && typeof found.node.dieline === "string" ? found.node : null;

  useEffect(() => {
    const token = ++facePreviewToken.current;
    if (!active) return;
    if (!doc || !page || !selectedDielineNode || !selectedPackagePanel) {
      setFaceHighlight(null);
      return;
    }
    setFaceHighlight(null);
    void api.scene3dPackFacePreview({ document: doc, page: page.id, node: selectedDielineNode.id, panel: selectedPackagePanel })
      .then((result) => {
        if (token === facePreviewToken.current) setFaceHighlight({ node: result.node, panel: result.panel, png: result.png_base64 });
      })
      .catch((cause: unknown) => {
        if (token === facePreviewToken.current) useStudio.getState().notify(cause instanceof Error ? cause.message : "The selected package face could not be highlighted.", true);
      });
    return () => { if (token === facePreviewToken.current) facePreviewToken.current += 1; };
  }, [doc, page, selectedDielineNode, selectedPackagePanel, active]);

  useLayoutEffect(() => {
    const el = scroller.current;
    if (!el) return;
    const observe = () => { if (el.clientWidth > 0 && el.clientHeight > 0) setBox({ w: el.clientWidth, h: el.clientHeight }); };
    observe();
    const ro = new ResizeObserver(observe);
    ro.observe(el);
    return () => ro.disconnect();
  }, []);

  const clearSprite = useCallback(() => {
    spriteToken.current += 1;
    setRest(null);
    setSprite((old) => {
      if (old) {
        URL.revokeObjectURL(old.backdrop);
        URL.revokeObjectURL(old.object);
      }
      return null;
    });
  }, []);

  const startSprite = useCallback(
    (id: string) => {
      if (!doc) return;
      const mine = ++spriteToken.current;
      const parts = dragLayers(doc, pageId, id);
      const edge = 2048;
      const renderPage = isScene(doc) ? pageId : undefined;
      void Promise.all([api.renderPng(parts.backdrop, renderPage, edge), api.renderPng(parts.sprite, renderPage, edge)])
        .then(([backdrop, object]) => {
          if (mine !== spriteToken.current) return;
          setSprite({ backdrop: URL.createObjectURL(backdrop), object: URL.createObjectURL(object), id });
        })
        .catch(() => undefined);
    },
    [doc, pageId],
  );

  // Rust render, debounced so a drag-heavy session does not queue renders.
  // The server renders at the size Studio displays (`edge`), so zooming in
  // stays sharp. A document edit publishes a quick proxy first and then the
  // full size; a zoom-only change keeps the current image until the sharper
  // one arrives.
  useEffect(() => {
    const shown = preview ?? doc;
    if (!active || !shown || view.kind !== "document") return;
    const mine = ++token.current;
    const zoomOnly = lastDoc.current.doc === shown && lastDoc.current.page === pageId;
    lastDoc.current = { doc: shown, page: pageId };
    const timer = window.setTimeout(() => {
      setLoading(true);
      const publish = (blob: Blob) => {
        if (mine !== token.current) return false;
        const next = URL.createObjectURL(blob);
        setUrl((old) => {
          if (old) window.setTimeout(() => URL.revokeObjectURL(old), 1000);
          return next;
        });
        setRendered({ url: next, doc: shown });
        if (!drag.current) clearSprite();
        return true;
      };
      const renderPage = isScene(shown) ? pageId : undefined;
      const first = zoomOnly ? edge : Math.min(edge, preview ? 1024 : 768);
      api
        .renderPng(shown, renderPage, first, "canvas")
        .then(async (blob) => {
          if (!publish(blob) || preview || first >= edge) return;
          publish(await api.renderPng(shown, renderPage, edge, "canvas"));
        })
        .catch((error: unknown) => {
          if (mine !== token.current || error instanceof Superseded) return;
          notify(error instanceof Error ? error.message : "Preview failed.", true);
        })
        .finally(() => {
          if (mine === token.current) setLoading(false);
        });
    }, zoomOnly ? 140 : preview ? 40 : 30);
    return () => { window.clearTimeout(timer); if (token.current === mine) token.current += 1; };
  }, [doc, preview, pageId, view.kind, notify, active, edge, clearSprite]);

  // Zoom around a viewport point (cursor, or the centre for buttons and keys).
  const zoomAt = useCallback((next: number, cx?: number, cy?: number) => {
    const el = scroller.current;
    const st = useStudio.getState();
    if (!el) return st.setZoom(next);
    const x = cx ?? el.clientWidth / 2;
    const y = cy ?? el.clientHeight / 2;
    const current = Math.max(0.02, fit * st.zoom);
    const px = (el.scrollLeft + x - pageOffset(el.clientWidth, canvas.width * current)) / current;
    const py = (el.scrollTop + y - pageOffset(el.clientHeight, canvas.height * current)) / current;
    anchor.current = { px, py, cx: x, cy: y };
    st.setZoom(next);
  }, [fit, canvas.width, canvas.height]);
  useLayoutEffect(() => {
    const el = scroller.current;
    const a = anchor.current;
    if (!el || !a) return;
    anchor.current = null;
    el.scrollLeft = pageOffset(el.clientWidth, W) + a.px * scale - a.cx;
    el.scrollTop = pageOffset(el.clientHeight, H) + a.py * scale - a.cy;
    // W and H follow scale.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [scale]);
  const zoomToFit = useCallback(() => {
    anchor.current = null;
    useStudio.getState().setZoom(1);
  }, []);
  const zoomToActual = useCallback(() => zoomAt(1 / fit), [zoomAt, fit]);
  const zoomToSelection = useCallback(() => {
    const el = scroller.current;
    if (!el || !selectedBox || selectedBox.width <= 0 || selectedBox.height <= 0) return zoomToFit();
    const target = Math.min((el.clientWidth - MARGIN * 2) / selectedBox.width, (el.clientHeight - MARGIN * 2) / selectedBox.height);
    anchor.current = { px: selectedBox.x + selectedBox.width / 2, py: selectedBox.y + selectedBox.height / 2, cx: el.clientWidth / 2, cy: el.clientHeight / 2 };
    useStudio.getState().setZoom(target / fit);
  }, [selectedBox, fit, zoomToFit]);
  // Let the command palette and menus outside the canvas drive the view.
  useEffect(() => {
    if (!active) return;
    canvasView.fit = zoomToFit;
    canvasView.actual = zoomToActual;
    canvasView.selection = zoomToSelection;
    canvasView.zoomBy = (factor) => zoomAt(useStudio.getState().zoom * factor);
    return () => {
      canvasView.fit = canvasView.actual = canvasView.selection = canvasView.zoomBy = undefined;
    };
  }, [active, zoomAt, zoomToFit, zoomToActual, zoomToSelection]);

  // Ctrl/Cmd+wheel and trackpad pinch zoom at the cursor. React's wheel
  // handler is passive, so the browser would zoom the whole page instead.
  useEffect(() => {
    const el = scroller.current;
    if (!el || !active) return;
    const onWheel = (event: WheelEvent) => {
      if (!event.ctrlKey && !event.metaKey) return;
      event.preventDefault();
      const rect = el.getBoundingClientRect();
      const delta = event.deltaMode === 1 ? event.deltaY * 16 : event.deltaY;
      const factor = Math.exp(-Math.max(-200, Math.min(200, delta)) * 0.0015);
      zoomAt(useStudio.getState().zoom * factor, event.clientX - rect.left, event.clientY - rect.top);
    };
    el.addEventListener("wheel", onWheel, { passive: false });
    return () => el.removeEventListener("wheel", onWheel);
  }, [zoomAt, active]);

  // Panning follows the pointer anywhere on screen, not only over the page.
  const beginPan = useCallback((e: React.PointerEvent): boolean => {
    const st = useStudio.getState();
    const hand = !st.paintMode && st.tool === "hand";
    if (!(e.button === 1 || (e.button === 0 && (spaceDown.current || hand)))) return false;
    const el = scroller.current;
    if (!el) return false;
    e.preventDefault();
    const start = { x: e.clientX, y: e.clientY, left: el.scrollLeft, top: el.scrollTop };
    document.body.style.cursor = "grabbing";
    const move = (ev: PointerEvent) => {
      el.scrollLeft = start.left - (ev.clientX - start.x);
      el.scrollTop = start.top - (ev.clientY - start.y);
    };
    const up = () => {
      document.body.style.cursor = "";
      window.removeEventListener("pointermove", move);
      window.removeEventListener("pointerup", up);
      window.removeEventListener("pointercancel", up);
    };
    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", up);
    window.addEventListener("pointercancel", up);
    return true;
  }, []);

  const toPage = useCallback(
    (e: { clientX: number; clientY: number }): Point => {
      const r = svg.current!.getBoundingClientRect();
      return { x: ((e.clientX - r.left) / r.width) * canvas.width, y: ((e.clientY - r.top) / r.height) * canvas.height };
    },
    [canvas.width, canvas.height],
  );

  const finishPen = useCallback(
    (closed: boolean) => {
      if (draft.length > 1) {
        const look = useStudio.getState().look;
        addObject("pen", { d: penPathData(draft, closed), closed });
        void look;
      }
      setDraft([]);
      setHover(null);
    },
    [draft],
  );

  // Leaving the pen tool commits the open path, like the classic editor.
  useEffect(() => {
    if (tool !== "pen" && draft.length) finishPen(false);
  }, [tool, draft.length, finishPen]);

  const commitMove = moveNodesBy;
  const commitResize = transformNodesPage;

  const onPointerDown = (e: React.PointerEvent<SVGSVGElement>) => {
    if (beginPan(e)) return;
    if (e.button !== 0) return;
    const p = toPage(e);
    if (useStudio.getState().workspaceId === "layout" && useLayout.getState().loaded) {
      void useLayout.getState().placeLoaded(p);
      return;
    }
    if (tool === "select" && useStudio.getState().workspaceId === "3d" && doc && page && found?.node.kind === "scene3d" && typeof found.node.dieline === "string" && selectedBox && p.x >= selectedBox.x && p.x <= selectedBox.x + selectedBox.width && p.y >= selectedBox.y && p.y <= selectedBox.y + selectedBox.height) {
      const [a, b, c, d, e0, f0] = found.world;
      const determinant = a * d - b * c;
      const localX = (d * (p.x - e0) - c * (p.y - f0)) / determinant;
      const localY = (-b * (p.x - e0) + a * (p.y - f0)) / determinant;
      const width = Number(found.node.width);
      const height = Number(found.node.height);
      const pickX = (localX - Number(found.node.x ?? 0)) / width;
      const pickY = (localY - Number(found.node.y ?? 0)) / height;
      if (!facePickPending.current && Math.abs(determinant) > 1e-12 && width > 0 && height > 0 && pickX >= 0 && pickX <= 1 && pickY >= 0 && pickY <= 1) {
        facePickPending.current = true;
        e.preventDefault();
        void api.scene3dUvFacePick({
          document: doc,
          page: page.id,
          node: found.node.id,
          x: pickX,
          y: pickY,
        }).then((result) => {
          if (result.panel) useStudio.getState().setSelectedPackagePanel(result.panel);
        }).catch((cause: unknown) => {
          useStudio.getState().notify(cause instanceof Error ? cause.message : "Package face could not be picked.", true);
        }).finally(() => {
          facePickPending.current = false;
        });
        return;
      }
    }
    const target = e.target as SVGElement;
    svg.current!.setPointerCapture(e.pointerId);
    if (tool === "select") {
      const handle = target.dataset.handle as Handle | undefined;
      if (handle && found && selectionBox) {
        drag.current = { mode: "resize", start: p, ids: selectionRoots(), handle, box: selectionBox };
        clearSprite();
        return;
      }
      const chain = hitChain(page, p.x, p.y, 4 / scale);
      const hit = chain ? pickFromChain(chain, useStudio.getState().selectedId, e.ctrlKey || e.metaKey) : null;
      // Shift-click adds or removes an object without starting a move.
      if (e.shiftKey && hit) {
        s.select(hit.node.id, "toggle");
        return;
      }
      // Pressing on an already selected object drags the whole selection.
      if (!hit || !useStudio.getState().selectedIds.includes(hit.node.id)) s.select(hit ? hit.node.id : null);
      const hitPanelId = hit?.node.dieline_panel_id ?? hit?.node.dieline_panel_label;
      if (typeof hitPanelId === "string") {
        useStudio.getState().setSelectedPackagePanel(hitPanelId);
      }
      if (hit) {
        const ids = selectionRoots();
        const st = useStudio.getState();
        let box: Rect | null = null;
        for (const id of ids) {
          const f = findNode(page, id);
          const bb = f ? pageBounds(f) : null;
          if (bb) box = unite(box, bb);
        }
        drag.current = { mode: "move", start: p, ids, box, snaps: snapTargets(page, new Set(st.selectedIds), canvas) };
        clearSprite();
        if (ids.length === 1) startSprite(ids[0]!);
      } else {
        drag.current = { mode: "marquee", start: p, additive: e.shiftKey };
      }
      return;
    }
    if (tool === "text") {
      const id = addObject("text", { at: p });
      if (id) {
        s.setTool("select");
        setTextEdit({ id, fresh: true });
      }
      return;
    }
    if (tool === "pen") {
      const first = draft[0];
      if (draft.length > 1 && first && Math.hypot(first.x - p.x, first.y - p.y) < 10 / scale) {
        finishPen(true);
        return;
      }
      let point = p;
      const last = draft.at(-1);
      if (e.shiftKey && last) {
        const angle = Math.round(Math.atan2(p.y - last.y, p.x - last.x) / (Math.PI / 4)) * (Math.PI / 4);
        const length = Math.hypot(p.x - last.x, p.y - last.y);
        point = { x: last.x + Math.cos(angle) * length, y: last.y + Math.sin(angle) * length };
      }
      drag.current = { mode: "pen", index: draft.length, start: point };
      setDraft([...draft, point]);
      return;
    }
    drag.current = { mode: "create", start: p };
    setGhost({ x: p.x, y: p.y, width: 0, height: 0 });
  };

  const onPointerMove = (e: React.PointerEvent<SVGSVGElement>) => {
    const d = drag.current;
    const p = toPage(e);
    if (!d) {
      if (tool === "pen" && draft.length) setHover(p);
      if (tool === "select") {
        const chain = hitChain(page, p.x, p.y, 4 / scale);
        const next = chain ? pickFromChain(chain, useStudio.getState().selectedId, e.ctrlKey || e.metaKey).node.id : null;
        if (next !== hoverId) setHoverId(next);
      }
      return;
    }
    if (d.mode === "marquee") {
      setMarquee({ x: Math.min(p.x, d.start.x), y: Math.min(p.y, d.start.y), width: Math.abs(p.x - d.start.x), height: Math.abs(p.y - d.start.y) });
    } else if (d.mode === "move") {
      let dx = p.x - d.start.x;
      let dy = p.y - d.start.y;
      // Shift keeps the move on one axis.
      if (e.shiftKey) {
        if (Math.abs(dx) > Math.abs(dy)) dy = 0;
        else dx = 0;
      }
      // Snap to other objects and the page; Ctrl/Cmd moves freely.
      if (d.box && !(e.ctrlKey || e.metaKey)) {
        const snap = snapBox({ ...d.box, x: d.box.x + dx, y: d.box.y + dy }, d.snaps, 6 / scale);
        if (!e.shiftKey || dy === 0) dx += snap.dx;
        if (!e.shiftKey || dx === 0) dy += snap.dy;
        setGuides(snap.guides.filter((g) => !e.shiftKey || (g.axis === "x" ? dy === 0 : dx === 0)));
      } else setGuides([]);
      setLive([1, 0, 0, 1, dx, dy]);
    } else if (d.mode === "resize") {
      const nb = resizedBox(d.box, d.handle, p, e.shiftKey);
      const sx = d.box.width ? nb.width / d.box.width : 1;
      const sy = d.box.height ? nb.height / d.box.height : 1;
      const ax = d.handle.includes("w") ? d.box.x + d.box.width : d.handle.includes("e") ? d.box.x : d.box.x;
      const ay = d.handle.includes("n") ? d.box.y + d.box.height : d.box.y;
      setLive([sx, 0, 0, sy, ax * (1 - sx), ay * (1 - sy)]);
    } else if (d.mode === "create") {
      let w = p.x - d.start.x;
      let h = p.y - d.start.y;
      if (e.shiftKey && tool !== "line") {
        const m = Math.max(Math.abs(w), Math.abs(h));
        w = Math.sign(w || 1) * m;
        h = Math.sign(h || 1) * m;
      }
      setGhost({ x: w < 0 ? d.start.x + w : d.start.x, y: h < 0 ? d.start.y + h : d.start.y, width: Math.abs(w), height: Math.abs(h) });
      if (tool === "line") setHover({ x: d.start.x + w, y: d.start.y + h });
    } else if (d.mode === "pen") {
      const dx = p.x - d.start.x;
      const dy = p.y - d.start.y;
      setDraft((points) =>
        points.map((pt, i) =>
          i === d.index ? { ...pt, out: { x: pt.x + dx, y: pt.y + dy }, in: { x: pt.x - dx, y: pt.y - dy } } : pt,
        ),
      );
    }
  };

  const onPointerUp = (e: React.PointerEvent<SVGSVGElement>) => {
    const d = drag.current;
    drag.current = null;
    setGuides([]);
    try {
      svg.current?.releasePointerCapture(e.pointerId);
    } catch {
      // not captured
    }
    if (!d) return;
    const p = toPage(e);
    if (d.mode === "marquee") {
      const r = marquee;
      setMarquee(null);
      if (!r || (r.width * scale < 3 && r.height * scale < 3)) {
        if (!d.additive) s.select(null);
        return;
      }
      const touched = topLevelIn(page, r);
      const current = useStudio.getState().selectedIds;
      s.selectMany(d.additive ? [...current, ...touched.filter((id) => !current.includes(id))] : touched);
    } else if (d.mode === "move") {
      const dx = live ? live[4] : 0;
      const dy = live ? live[5] : 0;
      setLive(null);
      if (Math.hypot(dx, dy) * scale >= 2) {
        // Alt+drag leaves the originals and moves copies, as in Figma and Illustrator.
        if (e.altKey) {
          clearSprite();
          duplicateSelectionBy(dx, dy);
        } else {
          if (d.ids.length === 1) setRest({ x: dx, y: dy });
          else clearSprite();
          commitMove(d.ids, dx, dy);
        }
      } else {
        clearSprite();
      }
    } else if (d.mode === "resize") {
      const m = live;
      setLive(null);
      if (m) commitResize(d.ids, m);
    } else if (d.mode === "create") {
      const r = ghost;
      setGhost(null);
      setHover(null);
      if (tool === "line") {
        const dx = p.x - d.start.x;
        const dy = p.y - d.start.y;
        if (Math.hypot(dx, dy) * scale > 3) {
          addObject("line", { line: [d.start.x, d.start.y, d.start.x + dx, d.start.y + dy] });
          s.setTool("select");
        }
      } else if (r && (tool === "rect" || tool === "ellipse")) {
        const tiny = r.width * scale < 3 && r.height * scale < 3;
        addObject(tool, { rect: tiny ? { x: d.start.x, y: d.start.y, width: 120, height: 120 } : r });
        s.setTool("select");
      }
    }
  };

  // Keyboard shortcuts; text inputs keep their own behaviour.
  useEffect(() => {
    const typing = (t: EventTarget | null) => /input|select|textarea/i.test((t as HTMLElement | null)?.tagName ?? "") || (t as HTMLElement | null)?.isContentEditable;
    const onKey = (e: KeyboardEvent) => {
      if (!active || e.defaultPrevented || typing(e.target)) return;
      const ctrl = e.ctrlKey || e.metaKey;
      const key = e.key.toLowerCase();
      if (e.code === "Space") {
        spaceDown.current = true;
        return;
      }
      if (ctrl && !e.altKey && ["0", "1", "=", "+", "-"].includes(e.key)) {
        // Keep the browser from zooming the whole page.
        e.preventDefault();
        if (e.key === "0") zoomToFit();
        else if (e.key === "1") zoomToActual();
        else zoomAt(useStudio.getState().zoom * (e.key === "-" ? 1 / 1.25 : 1.25));
        return;
      }
      if (ctrl) {
        if (key === "z") {
          e.preventDefault();
          if (e.shiftKey) s.redo();
          else s.undo();
        } else if (key === "y") {
          e.preventDefault();
          s.redo();
        } else if (key === "s") {
          e.preventDefault();
          void s.save();
        } else if (key === "d") {
          // In paint mode the paint overlay owns Ctrl+D (deselect).
          if (!useStudio.getState().paintMode) {
            e.preventDefault();
            duplicateSelected();
          }
        } else if (key === "a" && !useStudio.getState().paintMode) {
          e.preventDefault();
          selectAllOnPage();
        } else if (key === "g") {
          e.preventDefault();
          if (e.shiftKey) ungroupSelected();
          else groupSelected();
        } else if (e.key === "]") {
          e.preventDefault();
          reorderSelected(e.shiftKey ? "front" : "forward");
        } else if (e.key === "[") {
          e.preventDefault();
          reorderSelected(e.shiftKey ? "back" : "backward");
        }
        return;
      }
      if (e.shiftKey && (e.code === "Digit1" || e.code === "Digit2")) {
        e.preventDefault();
        if (e.code === "Digit1") zoomToFit();
        else zoomToSelection();
        return;
      }
      if (e.key === "Enter" && draft.length) {
        e.preventDefault();
        finishPen(false);
      } else if (e.key === "Enter" && found && nodeKind(found.node, found.layer) === "text" && !found.layer.locked && useStudio.getState().selectedIds.length === 1) {
        e.preventDefault();
        setTextEdit({ id: found.node.id, fresh: false });
      } else if (e.key === "Escape") {
        setMenu(null);
        setDraft([]);
        setHover(null);
        setGhost(null);
        s.select(null);
        s.setTool("select");
      } else if (e.key === "Delete" || e.key === "Backspace") {
        if (selectedId && !useStudio.getState().paintMode) {
          e.preventDefault();
          deleteSelected();
        }
      } else if (e.key.startsWith("Arrow") && selectedId) {
        e.preventDefault();
        const step = e.shiftKey ? 10 : 1;
        const dx = e.key === "ArrowLeft" ? -step : e.key === "ArrowRight" ? step : 0;
        const dy = e.key === "ArrowUp" ? -step : e.key === "ArrowDown" ? step : 0;
        commitMove(selectionRoots(), dx, dy);
      } else if (!e.altKey && !e.shiftKey) {
        const map: Record<string, Parameters<typeof s.setTool>[0]> = { v: "select", p: "pen", r: "rect", o: "ellipse", l: "line", t: "text", h: "hand" };
        const paint: Record<string, RasterTool> = { b: "brush", e: "eraser", m: "marquee", w: "wand", s: "clone", j: "heal", q: "quickmask" };
        const next = map[key];
        if (next) s.setTool(next);
        else if (paint[key] && isScene(useStudio.getState().doc)) startPainting(paint[key]);
      }
    };
    const onUp = (e: KeyboardEvent) => {
      if (e.code === "Space") spaceDown.current = false;
    };
    window.addEventListener("keydown", onKey);
    window.addEventListener("keyup", onUp);
    return () => {
      window.removeEventListener("keydown", onKey);
      window.removeEventListener("keyup", onUp);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [s, draft, selectedId, page, active, zoomAt, zoomToFit, zoomToActual, zoomToSelection]);

  // Paste: clipboard images become embedded images, text becomes a text object.
  useEffect(() => {
    const onPaste = (e: ClipboardEvent) => {
      if (!active || e.defaultPrevented) return;
      const t = e.target as HTMLElement | null;
      if (t && /input|select|textarea/i.test(t.tagName)) return;
      const marked = e.clipboardData?.getData("text/plain") ?? "";
      if (pasteSelectionText(marked)) {
        e.preventDefault();
        return;
      }
      const items = Array.from(e.clipboardData?.items ?? []);
      const image = items.find((item) => item.kind === "file" && item.type.startsWith("image/"));
      const file = image?.getAsFile();
      if (file) {
        e.preventDefault();
        void addImage(file, "pasted image");
        return;
      }
      const text = e.clipboardData?.getData("text/plain");
      if (text?.trim()) {
        e.preventDefault();
        const id = addObject("text", { at: { x: canvas.width / 4, y: canvas.height / 3 } });
        if (id) updateNode(id, (node) => {
          node.content = text;
        });
      }
    };
    // Copy and cut put the selected objects on the clipboard as marked JSON.
    const onCopy = (e: ClipboardEvent, cut: boolean) => {
      if (!active || e.defaultPrevented || useStudio.getState().paintMode) return;
      const t = e.target as HTMLElement | null;
      if (t && (/input|select|textarea/i.test(t.tagName) || t.isContentEditable)) return;
      if (window.getSelection()?.toString()) return;
      const text = copySelectionText();
      if (!text) return;
      e.preventDefault();
      e.clipboardData?.setData("text/plain", text);
      lastCopy.current = text;
      if (cut) deleteSelected();
    };
    const copy = (e: ClipboardEvent) => onCopy(e, false);
    const cut = (e: ClipboardEvent) => onCopy(e, true);
    window.addEventListener("paste", onPaste);
    window.addEventListener("copy", copy);
    window.addEventListener("cut", cut);
    return () => {
      window.removeEventListener("paste", onPaste);
      window.removeEventListener("copy", copy);
      window.removeEventListener("cut", cut);
    };
  }, [canvas.width, canvas.height, active]);

  const onDrop = (e: React.DragEvent) => {
    e.preventDefault();
    const photos = e.dataTransfer.getData(PHOTO_DRAG_TYPE);
    if (photos) {
      // Photos from the strip land centred where they are dropped, with their edits.
      if (view.kind !== "image" && svg.current) void usePhoto.getState().place(JSON.parse(photos) as string[], { at: toPage(e) });
      return;
    }
    for (const file of Array.from(e.dataTransfer.files)) {
      if (file.type.startsWith("image/")) void addImage(file, file.name);
      else notify(`Unsupported file: ${file.name}. Use File > Import .pen for documents.`, true);
    }
  };

  const src = view.kind === "image" ? view.url : url;
  const px = 1 / scale;
  const liveBox = useMemo(() => {
    if (!selectionBox) return null;
    if (!live) return selectionBox;
    const [x0, y0] = apply(live, selectionBox.x, selectionBox.y);
    const [x1, y1] = apply(live, selectionBox.x + selectionBox.width, selectionBox.y + selectionBox.height);
    return { x: Math.min(x0, x1), y: Math.min(y0, y1), width: Math.abs(x1 - x0), height: Math.abs(y1 - y0) };
  }, [selectionBox, live]);
  const cursor = tool === "hand" ? "grab" : tool === "select" ? "default" : "crosshair";
  const look = s.look;

  return (
    <div className="relative flex min-h-0 flex-1 flex-col bg-canvas">
      <div
        ref={scroller}
        onScroll={(event) => { if (active) scrollPosition.current = { left: event.currentTarget.scrollLeft, top: event.currentTarget.scrollTop }; }}
        className="flex-1 select-none overflow-auto [&_img]:pointer-events-none [&_img]:select-none [-webkit-user-drag:none]"
        onDragOver={(e) => {
          e.preventDefault();
          if (e.dataTransfer.types.includes(PHOTO_DRAG_TYPE)) e.dataTransfer.dropEffect = "copy";
        }}
        onDrop={onDrop}
      >
        <div className="flex min-h-full min-w-full items-center justify-center p-8">
          {view.kind === "image" ? (
            src && <img src={src} alt={view.label} className="block bg-white shadow-lg" style={{ width: W, maxWidth: "none" }} draggable={false} />
          ) : (
            <div className="relative shrink-0 shadow-lg" style={{ width: W, height: H, background: "#fff" }}>
              {src ? (
                <img
                  src={sprite ? sprite.backdrop : src}
                  alt="Page preview"
                  className="absolute inset-0 block h-full w-full"
                  draggable={false}
                  onLoad={(event) => {
                    if (rendered && event.currentTarget.src === rendered.url) setRenderedDoc(rendered.doc);
                  }}
                />
              ) : (
                <p className="absolute inset-0 grid place-items-center text-muted-foreground">{loading ? "Rendering..." : "No preview yet."}</p>
              )}
              {sprite && (
                <img
                  src={sprite.object}
                  alt=""
                  className="pointer-events-none absolute inset-0 block h-full w-full"
                  style={{
                    transform: `translate(${((live ? live[4] : (rest?.x ?? 0)) * scale).toFixed(2)}px, ${((live ? live[5] : (rest?.y ?? 0)) * scale).toFixed(2)}px)`,
                  }}
                  draggable={false}
                />
              )}
              {textEdit && (() => {
                const f = findNode(page, textEdit.id);
                const b = f ? pageBounds(f) : null;
                if (!f || !b) return null;
                const n = f.node;
                const m = multiply(f.world, nodeMatrix(n));
                const k = Math.sqrt(Math.abs(m[0] * m[3] - m[1] * m[2])) || 1;
                const size = Number(n.font_size ?? 16) * k * scale;
                const fill = typeof n.fill === "string" ? n.fill : (n.style as { fill?: { fallback?: unknown } } | undefined)?.fill?.fallback;
                const finish = (value: string) => {
                  setTextEdit(null);
                  if (!value.trim() && textEdit.fresh) {
                    s.select(textEdit.id);
                    deleteSelected();
                  } else if (value !== String(n.content ?? "")) updateNode(textEdit.id, (node) => void (node.content = value));
                };
                return (
                  <textarea
                    key={textEdit.id}
                    autoFocus
                    aria-label="Edit text"
                    defaultValue={String(n.content ?? "")}
                    spellCheck={false}
                    onFocus={(e) => e.currentTarget.select()}
                    onBlur={(e) => finish(e.currentTarget.value)}
                    onKeyDown={(e) => {
                      // Esc or Ctrl+Enter ends editing and keeps the text, like Figma.
                      if (e.key === "Escape" || (e.key === "Enter" && (e.ctrlKey || e.metaKey))) {
                        e.preventDefault();
                        e.stopPropagation();
                        e.currentTarget.blur();
                      }
                    }}
                    className="absolute z-10 resize-none overflow-hidden whitespace-pre rounded-sm border-0 p-0 outline outline-2 outline-[#2563eb]"
                    style={{
                      left: b.x * scale,
                      top: b.y * scale,
                      minWidth: Math.max(b.width * scale, size),
                      minHeight: b.height * scale,
                      fieldSizing: "content",
                      fontSize: size,
                      lineHeight: Number(n.line_height ?? 1.2),
                      fontFamily: `"${String(n.font_family ?? "sans-serif")}", system-ui, sans-serif`,
                      fontWeight: Number(n.font_weight ?? 400),
                      fontStyle: n.italic ? "italic" : undefined,
                      letterSpacing: Number(n.letter_spacing ?? 0) * k * scale || undefined,
                      textAlign: (String(n.align ?? "left") as "left" | "center" | "right"),
                      color: typeof fill === "string" ? fill : "#000",
                      background: "rgba(255,255,255,0.92)",
                    } as React.CSSProperties}
                  />
                );
              })()}
              <svg
                ref={svg}
                viewBox={`0 0 ${canvas.width} ${canvas.height}`}
                className="absolute inset-0 h-full w-full touch-none"
                style={{ cursor, pointerEvents: paintMode ? "none" : undefined }}
                onMouseDown={(e) => e.preventDefault()}
                onPointerDown={(e) => {
                  // The overlay keeps focus on mouse down, so end text editing explicitly.
                  if (textEdit) (document.activeElement as HTMLElement | null)?.blur();
                  onPointerDown(e);
                }}
                onPointerMove={onPointerMove}
                onPointerUp={onPointerUp}
                onPointerCancel={() => {
                  drag.current = null;
                  clearSprite();
                  setLive(null);
                  setGhost(null);
                  setMarquee(null);
                }}
                onPointerLeave={() => setHoverId(null)}
                onContextMenu={(e) => {
                  if (tool !== "select") return;
                  e.preventDefault();
                  const p = toPage(e);
                  const chain = hitChain(page, p.x, p.y, 4 / scale);
                  const hit = chain ? pickFromChain(chain, useStudio.getState().selectedId, e.ctrlKey || e.metaKey) : null;
                  if (!hit) s.select(null);
                  else if (!useStudio.getState().selectedIds.includes(hit.node.id)) s.select(hit.node.id);
                  setMenu({ x: e.clientX, y: e.clientY });
                }}
                onDoubleClick={(e) => {
                  if (tool === "pen") finishPen(false);
                  if (tool !== "select") return;
                  // Double-click steps into the group under the pointer.
                  const p = toPage(e);
                  const chain = hitChain(page, p.x, p.y, 4 / scale);
                  const at = chain?.findIndex((f) => f.node.id === useStudio.getState().selectedId) ?? -1;
                  const leaf = chain?.at(-1);
                  if (chain && at >= 0 && at < chain.length - 1) s.select(chain[at + 1]!.node.id);
                  else if (leaf && at === chain!.length - 1 && nodeKind(leaf.node, leaf.layer) === "text" && !leaf.layer.locked) setTextEdit({ id: leaf.node.id, fresh: false });
                }}
              >
                <LayoutOverlay px={px} />
                {faceHighlight && selectedDielineNode && found?.node.id === faceHighlight.node && selectedPackagePanel === faceHighlight.panel && (() => {
                  const transform = multiply(found.world, nodeMatrix(found.node));
                  return <g transform={`matrix(${transform.join(" ")})`} pointerEvents="none" aria-hidden="true">
                    {/* The parent transform already includes the node's page position. */}
                    <image href={`data:image/png;base64,${faceHighlight.png}`} x={0} y={0} width={Number(found.node.width)} height={Number(found.node.height)} preserveAspectRatio="none" />
                  </g>;
                })()}
                {tool === "select" && selectedIds.length > 1 && selectedIds.map((id) => {
                  const f = findNode(page, id);
                  const b = f ? pageBounds(f) : null;
                  return b && <rect key={id} {...b} transform={live ? `matrix(${live.join(" ")})` : undefined} fill="none" stroke="#2563eb" strokeWidth={px} strokeOpacity={0.6} pointerEvents="none" />;
                })}
                {tool === "select" && !drag.current && hoverId && !selectedIds.includes(hoverId) && (() => {
                  const f = findNode(page, hoverId);
                  if (!f) return null;
                  const m = multiply(f.world, nodeMatrix(f.node));
                  const k = nodeKind(f.node, f.layer);
                  const outline = { fill: "none", stroke: "#2563eb", strokeWidth: 1.5, vectorEffect: "non-scaling-stroke" as const, pointerEvents: "none" as const, "data-hover": hoverId };
                  if (k === "path" && typeof f.node.d === "string") return <path d={f.node.d} transform={`matrix(${m.join(" ")})`} {...outline} />;
                  if (k === "ellipse") return <ellipse cx={Number(f.node.cx ?? 0)} cy={Number(f.node.cy ?? 0)} rx={Number(f.node.radius_x ?? 0)} ry={Number(f.node.radius_y ?? 0)} transform={`matrix(${m.join(" ")})`} {...outline} />;
                  const b = pageBounds(f);
                  return b && <rect {...b} {...outline} />;
                })()}
                <RemoteActivity page={page} px={px} />
                {guides.map((g, i) => g.axis === "x"
                  ? <line key={i} x1={g.at} x2={g.at} y1={Math.min(0, g.from)} y2={Math.max(canvas.height, g.to)} stroke="#ec4899" strokeWidth={px} pointerEvents="none" />
                  : <line key={i} y1={g.at} y2={g.at} x1={Math.min(0, g.from)} x2={Math.max(canvas.width, g.to)} stroke="#ec4899" strokeWidth={px} pointerEvents="none" />)}
                {(() => {
                  // Size or position label under the box, like Figma's dimension badge.
                  const b = ghost && (tool === "rect" || tool === "ellipse") ? ghost : tool === "select" ? liveBox : null;
                  if (!b) return null;
                  const moving = drag.current?.mode === "move" && live;
                  const text = moving ? `X ${Math.round(b.x)}  Y ${Math.round(b.y)}` : `${Math.round(b.width)} × ${Math.round(b.height)}`;
                  const w = (text.length * 6.2 + 10) * px;
                  return (
                    <g pointerEvents="none" aria-label="Selection size">
                      <rect x={b.x + b.width / 2 - w / 2} y={b.y + b.height + 6 * px} width={w} height={16 * px} rx={3 * px} fill="#2563eb" />
                      <text x={b.x + b.width / 2} y={b.y + b.height + 17.5 * px} fontSize={11 * px} fill="#fff" textAnchor="middle" fontFamily="system-ui, sans-serif">{text}</text>
                    </g>
                  );
                })()}
                {marquee && (
                  <rect {...marquee} fill="#2563eb" fillOpacity={0.08} stroke="#2563eb" strokeWidth={px} pointerEvents="none" />
                )}
                {liveBox && tool === "select" && (
                  <g>
                    <rect {...liveBox} fill="none" stroke="#2563eb" strokeWidth={px * 1.5} pointerEvents="none" />
                    {HANDLES.map((h) => {
                      const hp = handlePoint(liveBox, h);
                      return (
                        <rect
                          key={h}
                          data-handle={h}
                          x={hp.x - 4 * px}
                          y={hp.y - 4 * px}
                          width={8 * px}
                          height={8 * px}
                          fill="#fff"
                          stroke="#2563eb"
                          strokeWidth={px}
                          style={{ cursor: `${h}-resize` }}
                        />
                      );
                    })}
                  </g>
                )}
                {ghost && (tool === "rect" || tool === "ellipse") && (
                  <rect {...ghost} rx={tool === "ellipse" ? ghost.width / 2 : 0} ry={tool === "ellipse" ? ghost.height / 2 : 0} fill="none" stroke="#2563eb" strokeWidth={px} strokeDasharray={`${4 * px} ${4 * px}`} pointerEvents="none" />
                )}
                {tool === "line" && ghost && hover && (
                  <line x1={ghost.x} y1={ghost.y} x2={hover.x} y2={hover.y} stroke="#2563eb" strokeWidth={px} pointerEvents="none" />
                )}
                {draft.length > 0 && (
                  <g pointerEvents="none">
                    <path d={penPathData(draft)} fill="none" stroke={look.stroke} strokeWidth={look.strokeWidth} strokeDasharray={`${6 * px} ${5 * px}`} />
                    {hover && <path d={penPathData([...draft, hover])} fill="none" stroke="#2563eb" strokeWidth={px} strokeDasharray={`${4 * px} ${4 * px}`} />}
                    {draft.map((pt, i) => (
                      <g key={i}>
                        {[pt.in, pt.out].filter(Boolean).map((h, k) => (
                          <g key={k}>
                            <line x1={pt.x} y1={pt.y} x2={h!.x} y2={h!.y} stroke="#2563eb" strokeWidth={px} />
                            <circle cx={h!.x} cy={h!.y} r={3 * px} fill="#fff" stroke="#2563eb" strokeWidth={px} />
                          </g>
                        ))}
                        <rect x={pt.x - 4 * px} y={pt.y - 4 * px} width={8 * px} height={8 * px} fill={i === draft.length - 1 ? "#2563eb" : "#fff"} stroke="#2563eb" strokeWidth={px} />
                      </g>
                    ))}
                  </g>
                )}
              </svg>
              {paintMode && isScene(doc) && (
                <PaintOverlay canvas={canvas} cssWidth={W} cssHeight={H} renderedDoc={renderedDoc} onPanStart={beginPan} />
              )}
            </div>
          )}
        </div>
      </div>
      <div className="absolute bottom-3 left-1/2 flex -translate-x-1/2 items-center gap-1 rounded-md border bg-card p-1 shadow">
        <Button variant="ghost" size="icon" aria-label="Zoom out" title="Zoom out (Ctrl+-)" onClick={() => zoomAt(zoom / 1.25)}>
          <Minus size={14} />
        </Button>
        <button type="button" className="w-14 rounded text-center text-xs tabular-nums hover:bg-accent" title="Actual size (Ctrl+1)" aria-label="Zoom to 100%" onClick={zoomToActual}>
          {scale >= 0.1 ? Math.round(scale * 100) : Math.round(scale * 1000) / 10}%
        </button>
        <Button variant="ghost" size="icon" aria-label="Zoom in" title="Zoom in (Ctrl+=)" onClick={() => zoomAt(zoom * 1.25)}>
          <Plus size={14} />
        </Button>
        <Button variant="ghost" size="icon" aria-label="Fit" title="Fit page (Ctrl+0, Shift+1)" onClick={zoomToFit}>
          <Maximize2 size={14} />
        </Button>
      </div>
      {flatDielineId && flatDieline && packageLocation && selectedPackagePanel && flatDieline.panels?.some((panel) => panel.id === selectedPackagePanel) && (
        <div className="absolute bottom-3 right-3 flex items-center gap-2 rounded-md border bg-card p-1.5 shadow" role="group" aria-label="Selected package panel">
          <span className="px-1 text-xs text-muted-foreground">Panel <span className="font-medium text-foreground">{selectedPackagePanel}</span></span>
          <Button type="button" variant="outline" className="min-h-10 focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2" onClick={() => {
            const state = useStudio.getState();
            state.setPage(packageLocation.page.id);
            state.select(packageLocation.found!.node.id);
            state.setWorkspace("3d");
          }}>View folded mockup</Button>
        </div>
      )}
      {dragToUv && found && found.node.kind !== "scene3d" && page && (
        <button
          type="button"
          draggable={!found.layer.locked}
          disabled={found.layer.locked}
          className="absolute right-3 top-3 min-h-10 rounded-md border bg-card px-3 text-xs font-medium text-foreground shadow focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2 active:cursor-grabbing"
          aria-label={found.layer.locked
            ? `Unlock the layer containing ${found.node.name || found.node.id} to drag it to the UV canvas`
            : `Drag selected artwork ${found.node.name || found.node.id} to the UV canvas`}
          title={found.layer.locked
            ? "Unlock this layer before linking its artwork to a 3D surface"
            : "Drag this selected page object onto the UV canvas to link it as a decal"}
          onDragStart={(event) => {
            event.dataTransfer.effectAllowed = "copy";
            event.dataTransfer.setData("application/x-pentool-uv-source", JSON.stringify({
              kind: "node",
              page: page.id,
              node: found.node.id,
            }));
            event.dataTransfer.setData("text/plain", found.node.name || found.node.id);
          }}
        >
          {found.layer.locked ? "Unlock artwork to drag" : "Drag selected art to UV"}
        </button>
      )}
      {menu && (() => {
        const st = useStudio.getState();
        const any = st.selectedIds.length > 0;
        const many = selectionRoots().length > 1;
        const group = found?.node.kind === "group";
        const clip = (cut: boolean) => {
          const text = copySelectionText();
          if (!text) return;
          lastCopy.current = text;
          void navigator.clipboard?.writeText(text).catch(() => undefined);
          if (cut) deleteSelected();
        };
        const items: ({ label: string; keys?: string; run: () => void; disabled?: boolean; danger?: boolean } | "-")[] = [
          { label: "Cut", keys: "Ctrl+X", run: () => clip(true), disabled: !any },
          { label: "Copy", keys: "Ctrl+C", run: () => clip(false), disabled: !any },
          { label: "Paste", keys: "Ctrl+V", run: () => lastCopy.current && pasteSelectionText(lastCopy.current), disabled: !lastCopy.current },
          { label: "Duplicate", keys: "Ctrl+D", run: duplicateSelected, disabled: !any },
          { label: "Delete", keys: "Del", run: deleteSelected, disabled: !any, danger: true },
          "-",
          { label: "Bring to front", keys: "Ctrl+Shift+]", run: () => reorderSelected("front"), disabled: !any || many },
          { label: "Bring forward", keys: "Ctrl+]", run: () => reorderSelected("forward"), disabled: !any || many },
          { label: "Send backward", keys: "Ctrl+[", run: () => reorderSelected("backward"), disabled: !any || many },
          { label: "Send to back", keys: "Ctrl+Shift+[", run: () => reorderSelected("back"), disabled: !any || many },
          "-",
          { label: "Group", keys: "Ctrl+G", run: groupSelected, disabled: !any || !isScene(doc) },
          { label: "Ungroup", keys: "Ctrl+Shift+G", run: ungroupSelected, disabled: !group || many },
          "-",
          { label: "Select all", keys: "Ctrl+A", run: selectAllOnPage },
          { label: "Zoom to selection", keys: "Shift+2", run: zoomToSelection, disabled: !any },
          { label: "Fit page", keys: "Shift+1", run: zoomToFit },
        ];
        return (
          <>
            <div className="fixed inset-0 z-40" onPointerDown={() => setMenu(null)} onContextMenu={(e) => { e.preventDefault(); setMenu(null); }} />
            <div role="menu" aria-label="Canvas actions" className="fixed z-50 grid min-w-56 gap-0.5 rounded-md border bg-card p-1 text-xs shadow-lg"
              style={{ left: Math.min(menu.x, window.innerWidth - 240), top: Math.min(menu.y, window.innerHeight - 440) }}
              onKeyDown={(e) => e.key === "Escape" && setMenu(null)}>
              {items.map((item, i) => item === "-"
                ? <div key={i} role="separator" className="my-0.5 h-px bg-border" />
                : (
                  <button key={item.label} type="button" role="menuitem" disabled={item.disabled} autoFocus={i === 0}
                    className={`flex items-center justify-between gap-4 rounded px-2 py-1.5 text-left hover:bg-accent disabled:opacity-40 ${item.danger ? "text-destructive" : ""}`}
                    onClick={() => { setMenu(null); item.run(); }}>
                    <span>{item.label}</span>
                    {item.keys && <kbd className="text-[10px] text-muted-foreground">{item.keys}</kbd>}
                  </button>
                ))}
            </div>
          </>
        );
      })()}
      {view.kind === "image" && (
        <div className="absolute left-3 top-3 rounded-md border bg-card px-2 py-1 text-xs">{view.label}</div>
      )}
      {loading && view.kind === "document" && src && (
        <div className="absolute right-3 top-3 rounded-md border bg-card px-2 py-1 text-xs">Updating...</div>
      )}
      {!isScene(doc) && doc && (
        <div className="absolute left-3 top-3 rounded-md border bg-card px-2 py-1 text-xs">Legacy v{doc.version} document</div>
      )}
      {layerId === undefined && null}
    </div>
  );
}
