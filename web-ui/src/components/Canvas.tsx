import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import { Maximize2, Minus, Plus } from "lucide-react";
import { api } from "@/lib/api";
import { dragLayers } from "@/lib/dragSprite";
import { useStudio } from "@/store";
import { Button } from "@/components/ui/button";
import { LayoutOverlay } from "@/workspaces/layout/LayoutOverlay";
import { useLayout } from "@/workspaces/layout/layoutStore";
import {
  addObject,
  addImage,
  moveNodeBy,
  updateNode,
  transformNodePage,
  deleteSelected,
  duplicateSelected,
  groupSelected,
  ungroupSelected,
  reorderSelected,
} from "@/lib/actions";
import {
  apply,
  findNode,
  hitTest,
  isScene,
  multiply,
  nodeMatrix,
  pageBounds,
  pageOf,
  penPathData,
  type Matrix,
  type PenPoint,
  type Rect,
} from "@/lib/model";

type Point = { x: number; y: number };
type Handle = "nw" | "n" | "ne" | "e" | "se" | "s" | "sw" | "w";
const HANDLES: Handle[] = ["nw", "n", "ne", "e", "se", "s", "sw", "w"];

type Drag =
  | { mode: "move"; start: Point; id: string }
  | { mode: "resize"; start: Point; id: string; handle: Handle; box: Rect }
  | { mode: "create"; start: Point }
  | { mode: "pen"; index: number; start: Point }
  | { mode: "pan"; x: number; y: number; left: number; top: number };

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
  const { doc, preview, pageId, view, zoom, setZoom, notify, tool, selectedId, layerId } = s;
  const [url, setUrl] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);
  const [box, setBox] = useState({ w: 800, h: 600 });
  const [draft, setDraft] = useState<PenPoint[]>([]);
  const [hover, setHover] = useState<Point | null>(null);
  const [ghost, setGhost] = useState<Rect | null>(null);
  const [live, setLive] = useState<Matrix | null>(null);
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
  const fit = Math.min((box.w - 64) / canvas.width, (box.h - 64) / canvas.height, 4);
  const scale = Math.max(0.02, fit * zoom);
  const found = useMemo(() => findNode(page, selectedId), [page, selectedId]);
  const selectedBox = useMemo(() => (found ? pageBounds(found) : null), [found]);
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
  // Committed edits publish a bounded proxy first, then replace it with the
  // full preview. This keeps camera and material controls responsive without
  // changing the native reference renderer used for either pass.
  useEffect(() => {
    const shown = preview ?? doc;
    if (!active || !shown || view.kind !== "document") return;
    const mine = ++token.current;
    const timer = window.setTimeout(() => {
      setLoading(true);
      const publish = (blob: Blob) => {
        if (mine !== token.current) return false;
        setUrl((old) => {
          if (old) URL.revokeObjectURL(old);
          return URL.createObjectURL(blob);
        });
        if (!drag.current) clearSprite();
        return true;
      };
      const renderPage = isScene(shown) ? pageId : undefined;
      api
        .renderPng(shown, renderPage, preview ? 1024 : 768)
        .then(async (blob) => {
          if (!publish(blob) || preview) return;
          publish(await api.renderPng(shown, renderPage, 2048));
        })
        .catch((error: unknown) => {
          if (mine !== token.current) return;
          notify(error instanceof Error ? error.message : "Preview failed.", true);
        })
        .finally(() => {
          if (mine === token.current) setLoading(false);
        });
    }, preview ? 40 : 30);
    return () => { window.clearTimeout(timer); if (token.current === mine) token.current += 1; };
  }, [doc, preview, pageId, view.kind, notify, active]);

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

  const commitMove = moveNodeBy;
  const commitResize = transformNodePage;

  const onPointerDown = (e: React.PointerEvent<SVGSVGElement>) => {
    if (e.button === 1 || tool === "hand" || spaceDown.current) {
      const el = scroller.current!;
      drag.current = { mode: "pan", x: e.clientX, y: e.clientY, left: el.scrollLeft, top: el.scrollTop };
      svg.current!.setPointerCapture(e.pointerId);
      return;
    }
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
      if (handle && found && selectedBox) {
        drag.current = { mode: "resize", start: p, id: found.node.id, handle, box: selectedBox };
        clearSprite();
        return;
      }
      const hit = hitTest(page, p.x, p.y);
      s.select(hit ? hit.node.id : null);
      const hitPanelId = hit?.node.dieline_panel_id ?? hit?.node.dieline_panel_label;
      if (typeof hitPanelId === "string") {
        useStudio.getState().setSelectedPackagePanel(hitPanelId);
      }
      if (hit) {
        drag.current = { mode: "move", start: p, id: hit.node.id };
        clearSprite();
        startSprite(hit.node.id);
      }
      return;
    }
    if (tool === "text") {
      const id = addObject("text", { at: p });
      if (id) s.setTool("select");
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
      return;
    }
    if (d.mode === "pan") {
      const el = scroller.current!;
      el.scrollLeft = d.left - (e.clientX - d.x);
      el.scrollTop = d.top - (e.clientY - d.y);
    } else if (d.mode === "move") {
      setLive([1, 0, 0, 1, p.x - d.start.x, p.y - d.start.y]);
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
    try {
      svg.current?.releasePointerCapture(e.pointerId);
    } catch {
      // not captured
    }
    if (!d) return;
    const p = toPage(e);
    if (d.mode === "move") {
      const dx = p.x - d.start.x;
      const dy = p.y - d.start.y;
      setLive(null);
      if (Math.hypot(dx, dy) * scale >= 2) {
        setRest({ x: dx, y: dy });
        commitMove(d.id, dx, dy);
      } else {
        clearSprite();
      }
    } else if (d.mode === "resize") {
      const m = live;
      setLive(null);
      if (m) commitResize(d.id, m);
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
          e.preventDefault();
          duplicateSelected();
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
      if (e.key === "Enter" && draft.length) {
        e.preventDefault();
        finishPen(false);
      } else if (e.key === "Escape") {
        setDraft([]);
        setHover(null);
        setGhost(null);
        s.select(null);
        s.setTool("select");
      } else if (e.key === "Delete" || e.key === "Backspace") {
        if (selectedId) {
          e.preventDefault();
          deleteSelected();
        }
      } else if (e.key.startsWith("Arrow") && selectedId) {
        e.preventDefault();
        const step = e.shiftKey ? 10 : 1;
        const dx = e.key === "ArrowLeft" ? -step : e.key === "ArrowRight" ? step : 0;
        const dy = e.key === "ArrowUp" ? -step : e.key === "ArrowDown" ? step : 0;
        commitMove(selectedId, dx, dy);
      } else {
        const map: Record<string, Parameters<typeof s.setTool>[0]> = { v: "select", p: "pen", r: "rect", o: "ellipse", l: "line", t: "text", h: "hand" };
        const next = map[key];
        if (next) s.setTool(next);
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
  }, [s, draft, selectedId, page, active]);

  // Paste: clipboard images become embedded images, text becomes a text object.
  useEffect(() => {
    const onPaste = (e: ClipboardEvent) => {
      if (!active || e.defaultPrevented) return;
      const t = e.target as HTMLElement | null;
      if (t && /input|select|textarea/i.test(t.tagName)) return;
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
    window.addEventListener("paste", onPaste);
    return () => window.removeEventListener("paste", onPaste);
  }, [canvas.width, canvas.height, active]);

  const onDrop = (e: React.DragEvent) => {
    e.preventDefault();
    for (const file of Array.from(e.dataTransfer.files)) {
      if (file.type.startsWith("image/")) void addImage(file, file.name);
      else notify(`Unsupported file: ${file.name}. Use File > Import .pen for documents.`, true);
    }
  };

  const src = view.kind === "image" ? view.url : url;
  const W = canvas.width * scale;
  const H = canvas.height * scale;
  const px = 1 / scale;
  const liveBox = useMemo(() => {
    if (!selectedBox) return null;
    if (!live) return selectedBox;
    const [x0, y0] = apply(live, selectedBox.x, selectedBox.y);
    const [x1, y1] = apply(live, selectedBox.x + selectedBox.width, selectedBox.y + selectedBox.height);
    return { x: Math.min(x0, x1), y: Math.min(y0, y1), width: Math.abs(x1 - x0), height: Math.abs(y1 - y0) };
  }, [selectedBox, live]);
  const cursor = tool === "hand" ? "grab" : tool === "select" ? "default" : "crosshair";
  const look = s.look;

  return (
    <div className="relative flex min-h-0 flex-1 flex-col bg-canvas">
      <div
        ref={scroller}
        onScroll={(event) => { if (active) scrollPosition.current = { left: event.currentTarget.scrollLeft, top: event.currentTarget.scrollTop }; }}
        className="flex-1 select-none overflow-auto [&_img]:pointer-events-none [&_img]:select-none [-webkit-user-drag:none]"
        onDragOver={(e) => e.preventDefault()}
        onDrop={onDrop}
        onWheel={(e) => {
          if (e.ctrlKey) {
            e.preventDefault();
            setZoom(zoom * (e.deltaY < 0 ? 1.1 : 1 / 1.1));
          }
        }}
      >
        <div className="flex min-h-full min-w-full items-center justify-center p-8">
          {view.kind === "image" ? (
            src && <img src={src} alt={view.label} className="block bg-white shadow-lg" style={{ width: W, maxWidth: "none" }} draggable={false} />
          ) : (
            <div className="relative shrink-0 shadow-lg" style={{ width: W, height: H, background: "#fff" }}>
              {src ? (
                <img src={sprite ? sprite.backdrop : src} alt="Page preview" className="absolute inset-0 block h-full w-full" draggable={false} />
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
              <svg
                ref={svg}
                viewBox={`0 0 ${canvas.width} ${canvas.height}`}
                className="absolute inset-0 h-full w-full touch-none"
                style={{ cursor }}
                onMouseDown={(e) => e.preventDefault()}
                onPointerDown={onPointerDown}
                onPointerMove={onPointerMove}
                onPointerUp={onPointerUp}
                onPointerCancel={() => {
                  drag.current = null;
                  clearSprite();
                  setLive(null);
                  setGhost(null);
                }}
                onDoubleClick={() => {
                  if (tool === "pen") finishPen(false);
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
            </div>
          )}
        </div>
      </div>
      <div className="absolute bottom-3 left-1/2 flex -translate-x-1/2 items-center gap-1 rounded-md border bg-card p-1 shadow">
        <Button variant="ghost" size="icon" aria-label="Zoom out" onClick={() => setZoom(zoom / 1.25)}>
          <Minus size={14} />
        </Button>
        <span className="w-12 text-center text-xs tabular-nums">{Math.round(scale * 100)}%</span>
        <Button variant="ghost" size="icon" aria-label="Zoom in" onClick={() => setZoom(zoom * 1.25)}>
          <Plus size={14} />
        </Button>
        <Button variant="ghost" size="icon" aria-label="Fit" onClick={() => setZoom(1)}>
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
