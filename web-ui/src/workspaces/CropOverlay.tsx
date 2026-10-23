import { useEffect, useRef } from "react";
import { useStudio } from "@/store";
import { findNode, nodeWorldTransform, pageOf } from "@/lib/model";
import type { SceneNode } from "@/lib/api";
import { applyCrop, cropBlocker, cropLimits, dragCrop, pureOffset, useCrop, type CropHandle, type CropWindow } from "./cropStore";

const HANDLES: CropHandle[] = ["nw", "n", "ne", "e", "se", "s", "sw", "w"];

export function cropOf(node: SceneNode): CropWindow {
  const c = node.crop;
  return Array.isArray(c) && c.length === 4 ? ([c[0], c[1], c[2], c[3]].map(Number) as CropWindow) : [0, 0, 1, 1];
}

/** Commit the current crop rectangle as one undoable edit. Returns false when nothing could be applied. */
export function commitCrop(): boolean {
  const { nodeId, draft, stop } = useCrop.getState();
  const { doc, pageId, edit } = useStudio.getState();
  const found = doc && nodeId ? findNode(pageOf(doc, pageId), nodeId) : null;
  if (!found || !draft) return false;
  const offset = pureOffset(nodeWorldTransform(found));
  const n = found.node;
  if (!offset) return false;
  const frame = { x: Number(n.x), y: Number(n.y), width: Number(n.width), height: Number(n.height) };
  const next = applyCrop(frame, cropOf(n), offset, draft);
  stop();
  edit("Crop image", (d, { pageId: id }) => {
    const target = findNode(pageOf(d, id), n.id)?.node;
    if (!target) return;
    target.x = next.frame.x;
    target.y = next.frame.y;
    target.width = next.frame.width;
    target.height = next.frame.height;
    target.crop = next.crop;
  });
  return true;
}

/**
 * Crop handles for one image, drawn over the page. Dragging moves the visible window
 * around the image, so the pixels stay where they are; Apply writes frame and crop together.
 */
export function CropOverlay({ canvas, px }: { canvas: { width: number; height: number }; px: number }) {
  const { doc, pageId } = useStudio();
  const crop = useCrop();
  const svg = useRef<SVGSVGElement>(null);
  const drag = useRef<CropHandle | null>(null);
  const found = doc && crop.nodeId ? findNode(pageOf(doc, pageId), crop.nodeId) : null;
  const blocked = found ? cropBlocker(found.node, nodeWorldTransform(found)) : crop.nodeId ? "gone" : null;

  useEffect(() => {
    if (!crop.nodeId) return;
    const onKey = (e: KeyboardEvent) => {
      const el = e.target as HTMLElement | null;
      if (el && (el.tagName === "INPUT" || el.tagName === "TEXTAREA" || el.tagName === "SELECT" || el.isContentEditable)) return;
      if (e.key === "Enter") {
        e.preventDefault();
        commitCrop();
      } else if (e.key === "Escape") {
        e.preventDefault();
        useCrop.getState().stop();
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [crop.nodeId]);

  // Leave the tool when the image goes away or stops being croppable.
  useEffect(() => {
    if (crop.nodeId && blocked) useCrop.getState().stop();
  }, [crop.nodeId, blocked]);

  if (!found || !crop.draft || blocked) return null;
  const offset = pureOffset(nodeWorldTransform(found))!;
  const n = found.node;
  const frame = { x: Number(n.x), y: Number(n.y), width: Number(n.width), height: Number(n.height) };
  const limits = cropLimits(frame, cropOf(n), offset);
  const d = crop.draft;

  const toPage = (e: React.PointerEvent) => {
    const m = svg.current?.getScreenCTM()?.inverse();
    if (!m) return null;
    const p = new DOMPoint(e.clientX, e.clientY).matrixTransform(m);
    return { x: p.x, y: p.y };
  };
  const move = (e: React.PointerEvent) => {
    const handle = drag.current;
    const p = toPage(e);
    if (!handle || !p) return;
    crop.setDraft(dragCrop(useCrop.getState().draft ?? d, handle, p, limits, crop.ratio));
  };
  const cut = `M0 0H${canvas.width}V${canvas.height}H0Z M${d.x} ${d.y}V${d.y + d.height}H${d.x + d.width}V${d.y}Z`;
  const third = (i: number) => ({ x: d.x + (d.width * i) / 3, y: d.y + (d.height * i) / 3 });

  return (
    <svg ref={svg} viewBox={`0 0 ${canvas.width} ${canvas.height}`} className="absolute inset-0 h-full w-full touch-none" style={{ pointerEvents: "none" }} aria-label="Crop">
      <path d={cut} fillRule="evenodd" fill="#000" fillOpacity={0.5} pointerEvents="none" />
      {[1, 2].map((i) => (
        <g key={i} pointerEvents="none" stroke="#fff" strokeOpacity={0.6} strokeWidth={px * 0.75}>
          <line x1={third(i).x} y1={d.y} x2={third(i).x} y2={d.y + d.height} />
          <line x1={d.x} y1={third(i).y} x2={d.x + d.width} y2={third(i).y} />
        </g>
      ))}
      <rect x={d.x} y={d.y} width={d.width} height={d.height} fill="none" stroke="#fff" strokeWidth={px * 1.5} pointerEvents="none" />
      {HANDLES.map((h) => {
        const x = h.includes("w") ? d.x : h.includes("e") ? d.x + d.width : d.x + d.width / 2;
        const y = h.includes("n") ? d.y : h.includes("s") ? d.y + d.height : d.y + d.height / 2;
        return (
          <rect
            key={h}
            data-crop-handle={h}
            x={x - 5 * px}
            y={y - 5 * px}
            width={10 * px}
            height={10 * px}
            fill="#fff"
            stroke="#2563eb"
            strokeWidth={px}
            style={{ cursor: `${h}-resize`, pointerEvents: "all" }}
            onPointerDown={(e) => {
              e.stopPropagation();
              (e.currentTarget as SVGRectElement).setPointerCapture(e.pointerId);
              drag.current = h;
            }}
            onPointerMove={move}
            onPointerUp={() => {
              drag.current = null;
            }}
            onPointerCancel={() => {
              drag.current = null;
            }}
          />
        );
      })}
      <g pointerEvents="none">
        <rect x={d.x + d.width / 2 - 34 * px} y={d.y + d.height + 6 * px} width={68 * px} height={16 * px} rx={3 * px} fill="#2563eb" />
        <text x={d.x + d.width / 2} y={d.y + d.height + 17.5 * px} fontSize={11 * px} fill="#fff" textAnchor="middle" fontFamily="system-ui, sans-serif">
          {Math.round(d.width)} × {Math.round(d.height)}
        </text>
      </g>
    </svg>
  );
}
