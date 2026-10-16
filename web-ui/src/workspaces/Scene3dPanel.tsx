import { useEffect, useMemo, useRef, useState, type PointerEvent } from "react";
import { Box, Loader2, Plus, Trash2 } from "lucide-react";
import { api, type SceneNode } from "@/lib/api";
import { updateNode } from "@/lib/actions";
import { clone, collectIds, findNode, layerNodes, pageOf, uid } from "@/lib/model";
import { useStudio } from "@/store";
import { Button } from "@/components/ui/button";
import { Check, Empty, NumberField, Row } from "@/components/ui/kit";
import { ARTWORK_SOURCE_DRAG_TYPE, parseArtworkSourceDrag } from "@/lib/artworkDrag";

const heading = "text-[11px] font-semibold uppercase tracking-wide text-muted-foreground";

function embeddedData(file: File): Promise<string> {
  return new Promise((resolve, reject) => {
    const reader = new FileReader();
    reader.onerror = () => reject(reader.error ?? new Error("Could not read the model."));
    reader.onload = () => resolve(String(reader.result).split(",", 2)[1] ?? "");
    reader.readAsDataURL(file);
  });
}

function camera(node: SceneNode): Record<string, unknown> {
  return node.camera && typeof node.camera === "object" && !Array.isArray(node.camera) ? node.camera as Record<string, unknown> : {};
}

function vector(value: unknown): [number, number, number] {
  return Array.isArray(value) && value.length === 3 ? [Number(value[0]) || 0, Number(value[1]) || 0, Number(value[2]) || 0] : [0, 0, 0];
}

type Light = { kind: "directional" | "point" | "spot"; color: string; intensity: number; direction?: number[]; position?: number[]; inner_deg?: number; outer_deg?: number };
type GroundPlane = { enabled: boolean; color?: string; size_multiplier?: number; offset?: number; mode?: "opaque" | "shadow-catcher"; opacity?: number };
type SurfaceGraphic = { id: string; source: { kind: "page" | "node"; page: string; node?: string }; slot: string; island?: string; material?: { metallic?: number; roughness?: number; opacity?: number }; mode: "decal" | "fill"; projection: "uv" | "planar" | "box" | "cylindrical" | "spherical" | "triplanar"; axis?: "x" | "y" | "z"; fit: "contain" | "cover" | "stretch"; wrap: "clamp" | "repeat" | "mirror"; blend: string; opacity: number; scale?: number[]; offset?: number[]; rotate?: number; crop?: number[] };
type PanelArtworkTransform = { scale: [number, number]; offset: [number, number]; rotate: number };
const defaultPanelArtworkTransform: PanelArtworkTransform = { scale: [1, 1], offset: [0.5, 0.5], rotate: 0 };
type ProductForm = { kind: "box" | "pouch" | "cylinder" | "tube" | "lathe-bottle"; generator: number; parameters: Record<string, unknown>; material_slots?: string[] };
type ProductFormKind = ProductForm["kind"];

const formDefaults: Record<ProductFormKind, { parameters: Record<string, unknown>; slots: string[] }> = {
  box: { parameters: { length_mm: 120, width_mm: 80, height_mm: 180, bevel_mm: 1 }, slots: ["front", "back", "sides", "top", "bottom"] },
  pouch: { parameters: { width_mm: 140, height_mm: 220, gusset_mm: 65 }, slots: ["front", "back", "gusset", "seals"] },
  cylinder: { parameters: { height_mm: 140, outer_diameter_mm: 70, radial_segments: 48, capped: true }, slots: ["body", "cap"] },
  tube: { parameters: { height_mm: 140, outer_diameter_mm: 70, inner_diameter_mm: 64, radial_segments: 48 }, slots: ["tube"] },
  "lathe-bottle": { parameters: { height_mm: 180, radial_segments: 64, profile_mm: [[24, 0], [26, 12], [26, 105], [23, 120], [11, 132], [11, 169], [8, 180]] }, slots: ["glass-body", "label-band"] },
};

function documentNodes(doc: NonNullable<ReturnType<typeof useStudio.getState>["doc"]>) {
  const result: { page: string; node: SceneNode }[] = [];
  const visit = (page: string, nodes: SceneNode[]) => nodes.forEach((node) => { result.push({ page, node }); visit(page, Array.isArray(node.children) ? node.children : []); });
  for (const page of doc.pages ?? []) for (const layer of page.layers ?? []) visit(page.id, layerNodes(layer));
  return result;
}

function FlatDielineImage({ svg, alt }: { svg: string; alt: string }) {
  const [url, setUrl] = useState<string | null>(null);
  useEffect(() => {
    const next = URL.createObjectURL(new Blob([svg], { type: "image/svg+xml" }));
    setUrl(next);
    return () => URL.revokeObjectURL(next);
  }, [svg]);
  return url
    ? <img className="h-full w-full" src={url} alt={alt} />
    : <div className="h-full w-full animate-pulse rounded bg-muted" role="status">Preparing flat dieline…</div>;
}

function BottleProfileEditor({
  profile,
  tangents,
  height,
  disabled,
  onCommit,
}: {
  profile: unknown[];
  tangents: unknown;
  height: number;
  disabled: boolean;
  onCommit: (profile: number[][], tangents: number[]) => void;
}) {
  const [draft, setDraft] = useState<{ points: number[][]; tangents: number[] } | null>(null);
  const [selectedIndex, setSelectedIndex] = useState(1);
  const drag = useRef<{ pointerId: number; index: number; mode: "point" | "tangent"; points: number[][]; tangents: number[]; radiusMax: number } | null>(null);
  const source = (draft?.points ?? profile.map((point) => Array.isArray(point) ? [Number(point[0]), Number(point[1])] : []))
    .filter((point) => point.length === 2 && point.every(Number.isFinite));
  if (source.length < 2 || !Number.isFinite(height) || height <= 0) return null;
  const automaticTangents = source.map(([radius, mm], index) => {
    const slope = index === 0
      ? (source[1][0] - radius) / (source[1][1] - mm)
      : index === source.length - 1
        ? (radius - source[index - 1][0]) / (mm - source[index - 1][1])
        : 0.5 * (source[index + 1][0] - source[index - 1][0]) / (source[index + 1][1] - source[index - 1][1]);
    return Math.max(-20, Math.min(20, slope));
  });
  const storedTangents = Array.isArray(tangents) && tangents.length === source.length && tangents.every((value) => Number.isFinite(Number(value)))
    ? tangents.map(Number)
    : automaticTangents;
  const tangentSource = draft?.tangents ?? storedTangents;
  const radiusMax = drag.current?.radiusMax ?? Math.max(1, ...source.map(([radius]) => radius)) * 1.35;
  const x = (radius: number) => 24 + Math.max(0, Math.min(radiusMax, radius)) / radiusMax * 268;
  const y = (mm: number) => 164 - Math.max(0, Math.min(height, mm)) / height * 140;
  const handle = (index: number) => {
    const rise = 24 * height / 140;
    const [radius, mm] = source[index];
    return { x: x(radius + tangentSource[index] * rise), y: y(mm + rise) };
  };
  const curve: number[][] = [[...source[0]]];
  for (let index = 0; index < source.length - 1; index += 1) {
    const [r1, y1] = source[index];
    const [r2, y2] = source[index + 1];
    const deltaY = y2 - y1;
    const m1 = tangentSource[index] * deltaY;
    const m2 = tangentSource[index + 1] * deltaY;
    for (let step = 1; step <= 4; step += 1) {
      const t = step / 4;
      const t2 = t * t;
      const t3 = t2 * t;
      const radius = (2 * t3 - 3 * t2 + 1) * r1 + (t3 - 2 * t2 + t) * m1
        + (-2 * t3 + 3 * t2) * r2 + (t3 - t2) * m2;
      curve.push([Math.max(Math.min(r1, r2), Math.min(Math.max(r1, r2), radius)), y1 + deltaY * t]);
    }
  }
  const profilePath = curve.map(([radius, mm], index) => `${index === 0 ? "M" : "L"} ${x(radius)} ${y(mm)}`).join(" ");
  const updateFromPointer = (event: PointerEvent<SVGSVGElement>) => {
    const current = drag.current;
    if (!current || current.pointerId !== event.pointerId) return;
    const bounds = event.currentTarget.getBoundingClientRect();
    if (bounds.width <= 0 || bounds.height <= 0) return;
    const sx = (event.clientX - bounds.left) / bounds.width * 320;
    const sy = (event.clientY - bounds.top) / bounds.height * 180;
    const nextPoints = current.points.map((point) => [...point]);
    const nextTangents = [...current.tangents];
    if (current.mode === "tangent") {
      const center = nextPoints[current.index];
      const deltaScreenY = sy - y(center[1]);
      if (Math.abs(deltaScreenY) >= 2) {
        const slope = -(sx - x(center[0])) * current.radiusMax * 140 / (deltaScreenY * height * 268);
        nextTangents[current.index] = Math.max(-20, Math.min(20, slope));
      }
    } else {
      const [previous, following] = [nextPoints[current.index - 1], nextPoints[current.index + 1]];
      nextPoints[current.index][0] = Math.max(0.1, Math.min(current.radiusMax, (sx - 24) / 268 * current.radiusMax));
      nextPoints[current.index][1] = Math.max(previous[1] + 0.01, Math.min(following[1] - 0.01, (164 - sy) / 140 * height));
    }
    setDraft({ points: nextPoints, tangents: nextTangents });
    current.points = nextPoints;
    current.tangents = nextTangents;
  };
  const finishDrag = (event: PointerEvent<SVGSVGElement>) => {
    const current = drag.current;
    if (!current || current.pointerId !== event.pointerId) return;
    drag.current = null;
    event.currentTarget.releasePointerCapture(event.pointerId);
    setDraft(null);
    onCommit(current.points, current.tangents);
  };
  const adjustPoint = (index: number, key: string) => {
    if (index === 0 || index === source.length - 1 || disabled) return;
    const next = source.map((point) => [...point]);
    const amount = key === "ArrowLeft" ? -0.5 : key === "ArrowRight" ? 0.5 : key === "ArrowUp" ? 0.5 : -0.5;
    if (key === "ArrowLeft" || key === "ArrowRight") next[index][0] = Math.max(0.1, next[index][0] + amount);
    else next[index][1] = Math.max(next[index - 1][1] + 0.01, Math.min(next[index + 1][1] - 0.01, next[index][1] + amount));
    onCommit(next, tangentSource);
  };
  return <div className="space-y-1.5">
    <svg viewBox="0 0 320 180" role="group" aria-label="Bottle profile graph: radius horizontally and height vertically" className="h-44 w-full touch-none rounded border bg-muted/20" onPointerMove={updateFromPointer} onPointerUp={finishDrag} onPointerCancel={(event) => { if (drag.current?.pointerId === event.pointerId) { drag.current = null; setDraft(null); } }}>
      <path d="M 24 164 H 300 M 24 164 V 18" fill="none" stroke="currentColor" strokeOpacity="0.22" />
      <path d={profilePath} fill="none" stroke="currentColor" strokeWidth="2.5" className="text-primary" />
      {source.map(([radius, mm], index) => <g key={index}>
        {index === selectedIndex && (() => { const endpoint = handle(index); return <>
          <line x1={x(radius)} y1={y(mm)} x2={endpoint.x} y2={endpoint.y} stroke="currentColor" strokeOpacity="0.45" strokeWidth="1.5" pointerEvents="none" />
          <circle cx={endpoint.x} cy={endpoint.y} r="9" fill="transparent" className={!disabled ? "cursor-crosshair" : ""} onPointerDown={(event) => {
            if (disabled) return;
            event.preventDefault(); event.stopPropagation();
            event.currentTarget.ownerSVGElement?.setPointerCapture(event.pointerId);
            drag.current = { pointerId: event.pointerId, index, mode: "tangent", points: source.map((point) => [...point]), tangents: [...tangentSource], radiusMax };
          }} onKeyDown={(event) => {
            if (!["ArrowLeft", "ArrowRight", "ArrowUp", "ArrowDown"].includes(event.key) || disabled) return;
            event.preventDefault();
            const next = [...tangentSource];
            next[index] = Math.max(-20, Math.min(20, next[index] + ((event.key === "ArrowRight" || event.key === "ArrowUp") ? 0.1 : -0.1)));
            onCommit(source, next);
          }} role="slider" tabIndex={!disabled ? 0 : -1} aria-label={`Tangent at profile point ${index + 1}: ${tangentSource[index].toFixed(2)} millimeters per millimeter`} aria-valuenow={tangentSource[index]} aria-valuemin={-20} aria-valuemax={20} />
          <circle cx={endpoint.x} cy={endpoint.y} r="3" fill="var(--background)" stroke="var(--primary)" strokeWidth="1.5" pointerEvents="none" />
        </>; })()}
        <circle cx={x(radius)} cy={y(mm)} r="12" fill="transparent" className={`${index > 0 && index < source.length - 1 && !disabled ? "cursor-move" : ""} focus-visible:stroke-[var(--ring)] focus-visible:stroke-[3px]`} onPointerDown={(event) => {
          setSelectedIndex(index);
          if (disabled || index === 0 || index === source.length - 1) return;
          event.preventDefault();
          event.currentTarget.ownerSVGElement?.setPointerCapture(event.pointerId);
          drag.current = { pointerId: event.pointerId, index, mode: "point", points: source.map((point) => [...point]), tangents: [...tangentSource], radiusMax };
        }} onKeyDown={(event) => {
          setSelectedIndex(index);
          if (["ArrowLeft", "ArrowRight", "ArrowUp", "ArrowDown"].includes(event.key)) {
            event.preventDefault();
            adjustPoint(index, event.key);
          }
        }} role="button" tabIndex={index > 0 && index < source.length - 1 && !disabled ? 0 : -1} aria-label={`Profile control point ${index + 1}: radius ${radius.toFixed(1)} millimeters, height ${mm.toFixed(1)} millimeters${index === 0 || index === source.length - 1 ? ", anchored endpoint" : ", use arrow keys to edit"}`} aria-disabled={disabled || index === 0 || index === source.length - 1} />
        <circle cx={x(radius)} cy={y(mm)} r={index === 0 || index === source.length - 1 ? 4 : 6} fill={index === 0 || index === source.length - 1 ? "currentColor" : "var(--primary)"} stroke="var(--background)" strokeWidth="2" pointerEvents="none" />
      </g>)}
      <text x="286" y="178" textAnchor="end" className="fill-muted-foreground" fontSize="9">radius · mm</text>
      <text x="8" y="16" className="fill-muted-foreground" fontSize="9">height</text>
    </svg>
    <p className="text-[10px] leading-4 text-muted-foreground">Drag a middle point to shape the bottle, or drag any tangent handle to adjust the local curve. Arrow keys adjust the focused point; base and mouth stay anchored.</p>
  </div>;
}

const newLight = (kind: Light["kind"]): Light => kind === "directional"
  ? { kind, color: "#ffffff", intensity: 2, direction: [-1, -2, -1] }
  : kind === "point"
    ? { kind, color: "#ffffff", intensity: 8, position: [2, 2, 3] }
    : { kind, color: "#ffffff", intensity: 8, position: [2, 2, 3], direction: [-1, -1, -1], inner_deg: 20, outer_deg: 30 };

export function Scene3dPanel() {
  const studio = useStudio();
  const input = useRef<HTMLInputElement>(null);
  const foldCommitPending = useRef(false);
  const flatPreviewRequest = useRef(0);
  const [importing, setImporting] = useState(false);
  const [material, setMaterial] = useState(0);
  const [lightIndex, setLightIndex] = useState(0);
  const [status, setStatus] = useState<{ adapter?: string; fallback_reason?: string } | null>(null);
  const [formKind, setFormKind] = useState<ProductFormKind>("lathe-bottle");
  const [formPreset, setFormPreset] = useState("perfume-bottle");
  const [formBusy, setFormBusy] = useState(false);
  const [formError, setFormError] = useState<string | null>(null);
  const [formNotice, setFormNotice] = useState<string | null>(null);
  const [sleeveBusy, setSleeveBusy] = useState(false);
  const [sleeveError, setSleeveError] = useState<string | null>(null);
  const [sleeveNotice, setSleeveNotice] = useState<string | null>(null);
  const [packTemplate, setPackTemplate] = useState<"sleeve" | "straight-tuck" | "reverse-tuck" | "mailer" | "tube" | "pouch" | "bottle-label">("straight-tuck");
  const [sleeve, setSleeve] = useState({ length: 120, width: 70, height: 180, radius: 30, board: 0.5, bleed: 3, overlap: 0, glueFlap: 15 });
  const [foldDrag, setFoldDrag] = useState<number | null>(null);
  const [foldBusy, setFoldBusy] = useState(false);
  const [foldError, setFoldError] = useState<string | null>(null);
  const [creaseBusy, setCreaseBusy] = useState(false);
  const [creaseError, setCreaseError] = useState<string | null>(null);
  const [creaseRetry, setCreaseRetry] = useState<{ id: string; angle: number } | null>(null);
  const [creaseDirectionRetry, setCreaseDirectionRetry] = useState<{ id: string; direction: "mountain" | "valley" } | null>(null);
  const [settingsBusy, setSettingsBusy] = useState(false);
  const [settingsError, setSettingsError] = useState<string | null>(null);
  const [settingsRetry, setSettingsRetry] = useState<{ board_mm?: number; bleed_mm?: number } | null>(null);
  const [selectedCrease, setSelectedCrease] = useState<string | null>(null);
  const [panelArtBusy, setPanelArtBusy] = useState(false);
  const [dragOverGraphic, setDragOverGraphic] = useState<string | null>(null);
  const [artworkDropNotice, setArtworkDropNotice] = useState<string | null>(null);
  const [artworkSourceFilter, setArtworkSourceFilter] = useState("");
  const [panelArtError, setPanelArtError] = useState<string | null>(null);
  const [panelArtNotice, setPanelArtNotice] = useState<string | null>(null);
  const [panelTransformRetry, setPanelTransformRetry] = useState<{ panel: string; transform: PanelArtworkTransform } | null>(null);
  const [panelSafeZoneRetry, setPanelSafeZoneRetry] = useState<{ panel: string; inset: number } | null>(null);
  const [packPreflightBusy, setPackPreflightBusy] = useState(false);
  const [packPreflightError, setPackPreflightError] = useState<string | null>(null);
  const [packPreflight, setPackPreflight] = useState<Awaited<ReturnType<typeof api.scene3dPackPreflight>>["result"] | null>(null);
  const [flatPreviewBusy, setFlatPreviewBusy] = useState(false);
  const [flatPreviewError, setFlatPreviewError] = useState<string | null>(null);
  const [flatPreview, setFlatPreview] = useState<Awaited<ReturnType<typeof api.scene3dPackFlatPreview>> | null>(null);
  const selectedPanel = studio.selectedPackagePanel;
  const setSelectedPanel = studio.setSelectedPackagePanel;
  const [panelDragPreview, setPanelDragPreview] = useState<{ panel: string; transform: PanelArtworkTransform } | null>(null);
  const panelDrag = useRef<{ panel: string; transform: PanelArtworkTransform; origin: [number, number] } | null>(null);
  useEffect(() => {
    void api.scene3dStatus().then(setStatus).catch(() => setStatus({ fallback_reason: "Renderer status unavailable." }));
  }, []);
  const selected = useMemo(() => studio.doc ? findNode(pageOf(studio.doc, studio.pageId), studio.selectedId)?.node ?? null : null, [studio.doc, studio.pageId, studio.selectedId]);
  const node = selected?.kind === "scene3d" ? selected : null;
  const productForms = (studio.doc?.product_forms ?? {}) as Record<string, ProductForm>;
  const formId = typeof node?.product_form === "string" ? node.product_form : null;
  const productForm = formId ? productForms[formId] : undefined;
  const dielineId = typeof node?.dieline === "string" ? node.dieline : null;
  const dielines = (studio.doc?.dielines ?? {}) as Record<string, { template?: string; layout_page?: string; fold_state?: number; dimensions_mm?: { board?: number; bleed?: number }; wrap?: { radius_mm: number; direction: string; seam_angle_deg: number; overlap_mm?: number }; panels?: { id: string; rect_mm: [number, number, number, number]; artwork?: { kind?: string; page?: string; node?: string }; artwork_transform?: PanelArtworkTransform; safe_zone_mm?: number }[]; creases?: { id: string; angle_deg: number; direction: "valley" | "mountain" }[] }>;
  const dieline = dielineId ? dielines[dielineId] : undefined;
  const activePanel = dieline?.panels?.find((panel) => panel.id === selectedPanel);
  const activePanelTransform = activePanel?.artwork_transform ?? defaultPanelArtworkTransform;
  useEffect(() => {
    flatPreviewRequest.current += 1;
    setFlatPreview(null);
    setFlatPreviewError(null);
    setFlatPreviewBusy(false);
    const currentPanel = useStudio.getState().selectedPackagePanel;
    if (!dieline?.panels?.some((panel) => panel.id === currentPanel)) {
      setSelectedPanel(dieline?.panels?.[0]?.id ?? null);
    }
    setPackPreflight(null);
    setPackPreflightError(null);
  }, [dielineId, dieline]);
  useEffect(() => {
    setSelectedCrease(dieline?.creases?.[0]?.id ?? null);
    setCreaseError(null);
    setCreaseRetry(null);
    setCreaseDirectionRetry(null);
    setSettingsError(null);
    setSettingsRetry(null);
  }, [dielineId]);
  const runFlatPreview = async () => {
    const doc = useStudio.getState().doc;
    if (!doc || !dielineId || flatPreviewBusy) return;
    const requestId = ++flatPreviewRequest.current;
    setFlatPreviewBusy(true);
    setFlatPreviewError(null);
    try {
      const result = await api.scene3dPackFlatPreview({ document: doc, dieline: dielineId, min_ppi: 300 });
      if (requestId === flatPreviewRequest.current) setFlatPreview(result);
    } catch (error) {
      if (requestId === flatPreviewRequest.current) {
        setFlatPreview(null);
        setFlatPreviewError(error instanceof Error ? error.message : "The flat dieline preview could not be rendered.");
      }
    } finally {
      if (requestId === flatPreviewRequest.current) setFlatPreviewBusy(false);
    }
  };
  const runPackPreflight = async () => {
    const doc = useStudio.getState().doc;
    if (!doc || !dielineId || packPreflightBusy) return;
    setPackPreflightBusy(true);
    setPackPreflightError(null);
    try {
      const result = await api.scene3dPackPreflight({ document: doc, dieline: dielineId, min_ppi: 300 });
      setPackPreflight(result.result);
    } catch (error) {
      setPackPreflight(null);
      setPackPreflightError(error instanceof Error ? error.message : "Packaging preflight could not be completed.");
    } finally {
      setPackPreflightBusy(false);
    }
  };
  const foldState = Number(dieline?.fold_state ?? 1);
  const shownFoldState = foldDrag ?? foldState;
  const activeCrease = dieline?.creases?.find((crease) => crease.id === selectedCrease);
  const previewFold = (state: number) => {
    const apply = (doc: NonNullable<typeof studio.doc>) => {
      const all = (doc.dielines ?? {}) as Record<string, { fold_state?: number }>;
      if (!dielineId || !all[dielineId]) return;
      all[dielineId]!.fold_state = state;
    };
    if (studio.doc) {
      const preview = clone(studio.doc);
      apply(preview);
      studio.setPreview(preview);
    }
  };
  const commitFold = async (state: number) => {
    const doc = useStudio.getState().doc;
    if (!doc || !dielineId || foldCommitPending.current || creaseBusy) return;
    foldCommitPending.current = true;
    setFoldBusy(true);
    setFoldError(null);
    try {
      const result = await api.scene3dFold({ document: doc, dieline: dielineId, fold_state: state });
      studio.setPreview(null);
      studio.replaceDoc(result.document, "Fold package preview");
    } catch (error) {
      studio.setPreview(null);
      setFoldError(error instanceof Error ? error.message : "The package fold could not be saved.");
    } finally {
      foldCommitPending.current = false;
      setFoldBusy(false);
      setFoldDrag(null);
    }
  };
  const setCreaseAngle = async (crease: string, angle: number) => {
    const doc = useStudio.getState().doc;
    if (!doc || !dielineId || creaseBusy || foldBusy || foldDrag !== null) return;
    setCreaseBusy(true);
    setCreaseError(null);
    setCreaseRetry(null);
    setCreaseDirectionRetry(null);
    try {
      const result = await api.scene3dCreaseAngle({ document: doc, dieline: dielineId, crease, angle_deg: angle });
      studio.replaceDoc(result.document, "Set package crease angle");
      studio.notify(`Updated ${crease} fold angle.`);
    } catch (error) {
      setCreaseError(error instanceof Error ? error.message : "The crease angle could not be saved.");
      setCreaseRetry({ id: crease, angle });
    } finally {
      setCreaseBusy(false);
    }
  };
  const setCreaseDirection = async (crease: string, direction: "mountain" | "valley") => {
    const doc = useStudio.getState().doc;
    if (!doc || !dielineId || creaseBusy || foldBusy || foldDrag !== null) return;
    setCreaseBusy(true);
    setCreaseError(null);
    setCreaseRetry(null);
    setCreaseDirectionRetry(null);
    try {
      const result = await api.scene3dCreaseDirection({ document: doc, dieline: dielineId, crease, direction });
      studio.replaceDoc(result.document, "Set package fold direction");
      studio.notify(`Set ${crease} to fold ${direction}.`);
    } catch (error) {
      setCreaseError(error instanceof Error ? error.message : "The crease direction could not be saved.");
      setCreaseDirectionRetry({ id: crease, direction });
    } finally {
      setCreaseBusy(false);
    }
  };
  const setPackagingSettings = async (patch: { board_mm?: number; bleed_mm?: number; seam_angle_deg?: number; wrap_direction?: "clockwise" | "counterclockwise" }) => {
    const doc = useStudio.getState().doc;
    if (!doc || !dielineId || settingsBusy || flatPreviewBusy || packPreflightBusy || foldBusy || creaseBusy || panelArtBusy) return;
    setSettingsBusy(true);
    setSettingsError(null);
    setSettingsRetry(null);
    try {
      const result = await api.scene3dPackSettings({ document: doc, dieline: dielineId, ...patch });
      studio.replaceDoc(result.document, "Edit package print settings");
      setFlatPreview(null);
      setFlatPreviewError(null);
      setPackPreflight(null);
      setPackPreflightError(null);
      studio.notify("Updated package print and wrap settings.");
    } catch (error) {
      setSettingsError(error instanceof Error ? error.message : "Package settings could not be saved.");
      setSettingsRetry(patch);
    } finally {
      setSettingsBusy(false);
    }
  };
  const cam = node ? camera(node) : {};
  const target = vector(cam.target);
  const lights = Array.isArray(node?.lights) ? node.lights as Light[] : [];
  const light = lights[lightIndex];
  const change = (mutate: (draft: SceneNode) => void) => node && updateNode(node.id, mutate);
  const groundPlane = node?.ground_plane && typeof node.ground_plane === "object" && !Array.isArray(node.ground_plane)
    ? node.ground_plane as GroundPlane
    : { enabled: false };
  const patchGroundPlane = (patch: Partial<GroundPlane>) => change((draft) => {
    const current: GroundPlane = draft.ground_plane && typeof draft.ground_plane === "object" && !Array.isArray(draft.ground_plane)
      ? draft.ground_plane as GroundPlane
      : { enabled: true };
    draft.ground_plane = {
      enabled: current.enabled ?? true,
      color: current.color ?? "#d8d5d0",
      size_multiplier: current.size_multiplier ?? 2.5,
      offset: current.offset ?? 0,
      ...patch,
    };
  });
  const cameraValue = (key: string, value: unknown) => change((draft) => {
    const next = { ...camera(draft), [key]: value };
    for (const stale of ["eye", "up", "near", "far"]) delete next[stale];
    draft.camera = next;
  });
  const applyCameraPreset = (azimuth: number, elevation: number) => change((draft) => {
    const next: Record<string, unknown> = { ...camera(draft), azimuth, elevation };
    for (const stale of ["eye", "up", "near", "far"]) delete next[stale];
    draft.camera = next;
  });
  const override = (key: string, value: unknown) => change((draft) => {
    const all = { ...((draft.material_overrides as Record<string, Record<string, unknown>> | undefined) ?? {}) };
    all[material] = { ...(all[material] ?? {}), [key]: value };
    draft.material_overrides = all;
  });
  const currentOverride = (key: "metallic" | "roughness", fallback: number) => Number((node?.material_overrides as Record<string, Record<string, unknown>> | undefined)?.[material]?.[key] ?? fallback);
  const overrideRecord = (node?.material_overrides as Record<string, Record<string, unknown>> | undefined)?.[material] ?? {};
  const graphics = Array.isArray(node?.surface_graphics) ? node.surface_graphics as SurfaceGraphic[] : [];
  const artworkSources = studio.doc ? documentNodes(studio.doc).filter(({ node: item }) => item.id !== node?.id && item.kind !== "scene3d") : [];
  const sourceFilter = artworkSourceFilter.trim().toLowerCase();
  const dragSources: { key: string; label: string; source: { kind: "page"; page: string } | { kind: "node"; page: string; node: string } }[] = [
    ...(studio.doc?.pages ?? []).map((page) => ({ key: `page:${page.id}`, label: `Page · ${page.name || page.id}`, source: { kind: "page" as const, page: page.id } })),
    ...artworkSources.map(({ page, node: item }) => ({ key: `node:${page}:${item.id}`, label: `Node · ${item.name || item.id}`, source: { kind: "node" as const, page, node: item.id } })),
  ].filter(({ label, key }) => !sourceFilter || `${label} ${key}`.toLowerCase().includes(sourceFilter));
  const visibleDragSources = dragSources.slice(0, 80);
  const panelSourceValue = (source: { kind?: string; page?: string; node?: string } | undefined) => {
    if (source?.kind === "page" && source.page) return `page:${source.page}`;
    if (source?.kind === "node" && source.page && source.node) return `node:${source.page}:${source.node}`;
    return "";
  };
  const setPanelArtwork = async (panel: string, value: string) => {
    const doc = useStudio.getState().doc;
    if (!doc || !dielineId || panelArtBusy) return;
    let source: { kind: "page" | "node"; page: string; node?: string } | undefined;
    if (value.startsWith("page:")) source = { kind: "page", page: value.slice(5) };
    else if (value.startsWith("node:")) {
      const found = artworkSources.find(({ page, node: item }) => `node:${page}:${item.id}` === value);
      if (found) source = { kind: "node", page: found.page, node: found.node.id };
    }
    setPanelArtBusy(true);
    setPanelArtError(null);
    setPanelArtNotice(null);
    try {
      const result = await api.scene3dPanelArt({ document: doc, dieline: dielineId, panel, source });
      studio.replaceDoc(result.document, "Link sleeve panel artwork");
      setPanelArtNotice(`Updated panel ${panel}.`);
    } catch (error) {
      setPanelArtError(error instanceof Error ? error.message : "Panel artwork could not be linked.");
    } finally {
      setPanelArtBusy(false);
    }
  };
  const setPanelArtworkTransform = async (panel: string, transform: PanelArtworkTransform) => {
    const doc = useStudio.getState().doc;
    if (!doc || !dielineId || panelArtBusy) return;
    setPanelArtBusy(true);
    setPanelArtError(null);
    setPanelArtNotice(null);
    try {
      const result = await api.scene3dPanelArtTransform({ document: doc, dieline: dielineId, panel, ...transform });
      studio.replaceDoc(result.document, "Place packaging panel artwork");
      setPanelArtNotice(`Updated ${panel} artwork placement.`);
      setPanelTransformRetry(null);
      await new Promise<void>((resolve) => requestAnimationFrame(() => resolve()));
      await runFlatPreview();
    } catch (error) {
      setPanelArtError(error instanceof Error ? error.message : "Panel artwork placement could not be saved.");
      setPanelTransformRetry({ panel, transform });
    } finally {
      setPanelArtBusy(false);
    }
  };
  const setPanelSafeZone = async (panel: string, inset: number) => {
    const doc = useStudio.getState().doc;
    if (!doc || !dielineId || panelArtBusy) return;
    setPanelArtBusy(true);
    setPanelArtError(null);
    setPanelArtNotice(null);
    try {
      const result = await api.scene3dPanelSafeZone({ document: doc, dieline: dielineId, panel, inset_mm: inset });
      studio.replaceDoc(result.document, "Set packaging safe area");
      setPanelArtNotice(`Updated ${panel} safe-area guide.`);
      setPanelSafeZoneRetry(null);
      await new Promise<void>((resolve) => requestAnimationFrame(() => resolve()));
      await runFlatPreview();
      await runPackPreflight();
    } catch (error) {
      setPanelArtError(error instanceof Error ? error.message : "Panel safe-area guide could not be saved.");
      setPanelSafeZoneRetry({ panel, inset });
    } finally {
      setPanelArtBusy(false);
    }
  };
  const updateGraphic = (id: string, patch: Partial<SurfaceGraphic>) => change((draft) => {
    const items = [...((draft.surface_graphics as SurfaceGraphic[] | undefined) ?? [])];
    const index = items.findIndex((item) => item.id === id);
    if (index >= 0) items[index] = { ...items[index], ...patch };
    draft.surface_graphics = items;
  });
  const activateArtworkSource = (source: { kind: "page"; page: string } | { kind: "node"; page: string; node: string }) => {
    const first = graphics[0];
    if (first) {
      updateGraphic(first.id, { source });
      setArtworkDropNotice(`Linked ${source.kind === "page" ? `page ${source.page}` : `node ${source.node}`} to slot ${first.slot}.`);
      return;
    }
    let id = uid("graphic");
    while (graphics.some((graphic) => graphic.id === id)) id = uid("graphic");
    change((draft) => {
      draft.surface_graphics = [...((draft.surface_graphics as SurfaceGraphic[] | undefined) ?? []), {
        id, source, slot: "0", mode: "decal", projection: "uv", fit: "contain", wrap: "clamp", blend: "normal", opacity: 1, scale: [1, 1], offset: [0.5, 0.5],
      }];
    });
    setArtworkDropNotice(`Added ${source.kind === "page" ? `page ${source.page}` : `node ${source.node}`} to slot 0.`);
  };
  const editLight = (patch: Partial<Light>) => change((draft) => {
    const next = [...((draft.lights as Light[] | undefined) ?? [])];
    if (next[lightIndex]) next[lightIndex] = { ...next[lightIndex], ...patch };
    draft.lights = next;
  });

  const applyForm = async (action: "add" | "set", params: Record<string, unknown>, materialSlots?: string[], preset?: string) => {
    const doc = useStudio.getState().doc;
    if (!doc) return;
    if (doc.version !== 11) {
      setFormError("Upgrade this document to format v11 before editing product forms.");
      return;
    }
    setFormBusy(true);
    setFormError(null);
    setFormNotice(null);
    try {
      const base = { document: doc, action, form_id: formId ?? "" } as const;
      const result = action === "add"
        ? await api.scene3dForm({
          ...base,
          form_id: uid("form"),
          ...(preset
            ? { preset }
            : { kind: formKind, parameters: params, material_slots: materialSlots ?? formDefaults[formKind].slots }),
          node_id: uid("scene3d"),
          layer: studio.layerId ?? doc.pages?.[0]?.layers?.[0]?.id,
          page: studio.pageId ?? doc.pages?.[0]?.id,
          placement: [80, 80, 480, 560],
        })
        : await api.scene3dForm({ ...base, form_id: formId ?? "", parameters: params, ...(materialSlots ? { material_slots: materialSlots } : {}) });
      studio.replaceDoc(result.document, action === "add" ? "Create product form" : "Edit product form");
      if (result.result.node) studio.select(result.result.node);
      setFormNotice(`${action === "add" ? "Created" : "Updated"} ${result.result.form} · ${result.result.triangles.toLocaleString()} triangles`);
      studio.notify(`Product form ${action === "add" ? "created" : "updated"}.`);
    } catch (error) {
      setFormError(error instanceof Error ? error.message : "The product form could not be saved.");
    } finally {
      setFormBusy(false);
    }
  };

  const patchForm = (patch: Record<string, unknown>, materialSlots?: string[]) => {
    if (!productForm) return;
    const parameters = { ...productForm.parameters, ...patch };
    if (productForm.kind === "lathe-bottle" && typeof patch.height_mm === "number" && Array.isArray(parameters.profile_mm)) {
      const profile = parameters.profile_mm.map((point) => Array.isArray(point) ? [...point] : point);
      const last = profile[profile.length - 1];
      if (Array.isArray(last)) last[1] = patch.height_mm;
      parameters.profile_mm = profile;
    }
    void applyForm("set", parameters, materialSlots);
  };
  const formNumber = (key: string, label: string, min = 1, max = 100_000, step = 1) => {
    const value = Number(productForm?.parameters[key] ?? min);
    return <Row key={key} label={label}><NumberField className="min-h-10" value={value} min={min} max={max} step={step} disabled={formBusy} onCommit={(next) => patchForm({ [key]: step === 1 ? Math.round(next) : next })} ariaLabel={`${label} in millimeters`} /></Row>;
  };
  const updateProfilePoint = (index: number, axis: 0 | 1, value: number) => {
    const profile = productForm?.parameters.profile_mm;
    if (!Array.isArray(profile)) return;
    const next = profile.map((point) => Array.isArray(point) ? [...point] : point);
    if (!Array.isArray(next[index])) return;
    (next[index] as unknown[])[axis] = value;
    patchForm({ profile_mm: next });
  };
  const addProfilePoint = () => {
    const profile = productForm?.parameters.profile_mm;
    if (!Array.isArray(profile) || profile.length >= 128 || profile.length < 2) return;
    const next = profile.map((point) => Array.isArray(point) ? [...point] : point);
    const before = next[next.length - 2];
    const end = next[next.length - 1];
    if (!Array.isArray(before) || !Array.isArray(end)) return;
    const insertAt = next.length - 1;
    next.splice(insertAt, 0, [(Number(before[0]) + Number(end[0])) / 2, (Number(before[1]) + Number(end[1])) / 2]);
    const parameters: Record<string, unknown> = { ...(productForm?.parameters ?? {}), profile_mm: next };
    const existingTangents = parameters.profile_tangents;
    if (Array.isArray(existingTangents) && existingTangents.length === next.length - 1) {
      const slopes = existingTangents.map(Number);
      slopes.splice(insertAt, 0, (slopes[insertAt - 1] + slopes[insertAt]) / 2);
      parameters.profile_tangents = slopes;
    }
    void applyForm("set", parameters);
  };
  const removeProfilePoint = (index: number) => {
    const profile = productForm?.parameters.profile_mm;
    if (!Array.isArray(profile) || profile.length <= 3 || index === 0 || index === profile.length - 1) return;
    const parameters: Record<string, unknown> = { ...(productForm?.parameters ?? {}), profile_mm: profile.filter((_, pointIndex) => pointIndex !== index) };
    const existingTangents = parameters.profile_tangents;
    if (Array.isArray(existingTangents) && existingTangents.length === profile.length) {
      parameters.profile_tangents = existingTangents.filter((_, pointIndex) => pointIndex !== index);
    }
    void applyForm("set", parameters);
  };
  const toggleBottleCap = (enabled: boolean) => {
    if (!productForm || productForm.kind !== "lathe-bottle") return;
    const parameters = { ...productForm.parameters };
    const slots = [...(productForm.material_slots ?? [])];
    if (enabled) {
      const profile = parameters.profile_mm;
      const mouth = Array.isArray(profile) ? profile[profile.length - 1] : null;
      if (!Array.isArray(mouth)) return;
      parameters.cap_mm = {
        radius_mm: Number(mouth[0]) + 1,
        height_mm: Math.min(18, Number(parameters.height_mm ?? 180)),
      };
      if (!slots.includes("cap")) slots.push("cap");
    } else {
      delete parameters.cap_mm;
    }
    void applyForm("set", parameters, slots);
  };

  const createPackage = async () => {
    const doc = useStudio.getState().doc;
    if (!doc || !studio.layerId || doc.version !== 11) return;
    setSleeveBusy(true);
    setSleeveError(null);
    setSleeveNotice(null);
    try {
      const nodeId = uid("scene3d");
      const placement: [number, number, number, number] = [80, 80, 480, 560];
      const common = {
        document: doc,
        dieline_id: uid(packTemplate === "tube" ? "tube-label" : packTemplate),
        node_id: nodeId,
        layer: studio.layerId,
        page: studio.pageId,
        placement,
      };
      const result = packTemplate === "tube"
        ? await api.scene3dTubeLabel({ ...common, radius_mm: sleeve.radius, height_mm: sleeve.height, bleed_mm: sleeve.bleed, overlap_mm: sleeve.overlap })
        : packTemplate === "bottle-label"
          ? await api.scene3dBottleLabel({ ...common, radius_mm: sleeve.radius, height_mm: sleeve.height, bleed_mm: sleeve.bleed, overlap_mm: sleeve.overlap })
          : packTemplate === "pouch"
            ? await api.scene3dPouch({ ...common, front_width_mm: sleeve.length, gusset_mm: sleeve.width, height_mm: sleeve.height, bleed_mm: sleeve.bleed })
        : packTemplate === "sleeve"
          ? await api.scene3dSleeve({ ...common, length_mm: sleeve.length, width_mm: sleeve.width, height_mm: sleeve.height, board_mm: sleeve.board, bleed_mm: sleeve.bleed })
          : await api.scene3dCarton({ ...common, template: packTemplate, length_mm: sleeve.length, width_mm: sleeve.width, height_mm: sleeve.height, board_mm: sleeve.board, bleed_mm: sleeve.bleed, glue_flap_mm: sleeve.glueFlap });
      studio.replaceDoc(result.document, "Create packaging mockup");
      studio.select(nodeId);
      setSleeveNotice(packTemplate === "tube" || packTemplate === "bottle-label"
        ? `Created ${packTemplate === "tube" ? "tube" : "bottle"} label · radius ${sleeve.radius} mm × height ${sleeve.height} mm · ${sleeve.overlap} mm overlap`
        : packTemplate === "pouch"
          ? `Created stand-up pouch · front ${sleeve.length} mm · gusset ${sleeve.width} mm · height ${sleeve.height} mm`
          : `Created ${packTemplate.replaceAll("-", " ")} · ${sleeve.length} × ${sleeve.width} × ${sleeve.height} mm`);
      studio.notify("Packaging mockup created.");
    } catch (error) {
      setSleeveError(error instanceof Error ? error.message : "The packaging mockup could not be created.");
    } finally {
      setSleeveBusy(false);
    }
  };

  const importFile = async (file: File) => {
    if (!studio.doc || !studio.layerId) return;
    const format = file.name.split(".").pop()?.toLowerCase();
    if (format !== "glb" && format !== "gltf" && format !== "obj") return studio.notify("Choose a .glb, .gltf, or .obj model.", true);
    setImporting(true);
    try {
      let id = uid("scene3d");
      const ids = collectIds(studio.doc);
      while (ids.has(id)) id = uid("scene3d");
      const result = await api.scene3dImport({ document: studio.doc, data: await embeddedData(file), format, id, layer: studio.layerId, page: studio.pageId, x: 80, y: 80, width: 480, height: 480 });
      studio.replaceDoc(result.document, "Import 3D model");
      studio.select(id);
      studio.notify(`Imported ${file.name}.`);
    } catch (error) {
      studio.notify(error instanceof Error ? error.message : "Model import failed.", true);
    } finally {
      setImporting(false);
      if (input.current) input.current.value = "";
    }
  };

  const flatBounds = (dieline?.panels ?? []).reduce(
    (bounds, panel) => ({
      width: Math.max(bounds.width, panel.rect_mm[0] + panel.rect_mm[2]),
      height: Math.max(bounds.height, panel.rect_mm[1] + panel.rect_mm[3]),
    }),
    { width: 1, height: 1 },
  );
  const onPanelArtworkPointerMove = (event: PointerEvent<HTMLButtonElement>, panelId: string) => {
    const current = panelDrag.current;
    if (!current || current.panel !== panelId || !event.currentTarget.hasPointerCapture(event.pointerId)) return;
    const rect = event.currentTarget.getBoundingClientRect();
    if (!rect.width || !rect.height) return;
    const transform = current.transform;
    const x = (event.clientX - rect.left) / rect.width;
    const y = (event.clientY - rect.top) / rect.height;
    const offset: [number, number] = [
      Math.max(transform.scale[0] / 2, Math.min(1 - transform.scale[0] / 2, x)),
      Math.max(transform.scale[1] / 2, Math.min(1 - transform.scale[1] / 2, y)),
    ];
    const next = { ...transform, offset };
    panelDrag.current = { panel: panelId, transform: next, origin: current.origin };
    setPanelDragPreview({ panel: panelId, transform: next });
  };
  const finishPanelArtworkPointer = (panelId: string, commit: boolean) => {
    const current = panelDrag.current;
    panelDrag.current = null;
    setPanelDragPreview(null);
    if (commit && current?.panel === panelId && current.transform.offset.some((value, index) => Math.abs(value - current.origin[index]) > 0.002)) {
      void setPanelArtworkTransform(panelId, current.transform);
    }
  };

  return <div className="space-y-4">
    <section className="space-y-2 rounded border bg-card p-2.5" aria-label="Create a packaging mockup" aria-busy={sleeveBusy}>
      <div><h3 className={heading}>Packaging mockup</h3><p className="mt-1 text-[11px] text-muted-foreground">Generate linked flat artwork and a matching 3D preview.</p></div>
      <label className="block space-y-1 text-xs text-muted-foreground" htmlFor="scene3d-pack-template">Package template</label>
      <select id="scene3d-pack-template" className="h-10 w-full rounded border bg-input px-2 text-sm text-foreground focus-visible:ring-2 focus-visible:ring-ring" value={packTemplate} onChange={(event) => setPackTemplate(event.target.value as typeof packTemplate)}>
        <option value="sleeve">Open-ended sleeve · 4 panels</option><option value="straight-tuck">Straight-tuck carton · 13 panels</option><option value="reverse-tuck">Reverse-tuck carton · 13 panels</option><option value="mailer">Mailer box · 7 panels</option><option value="tube">Cylindrical tube label</option><option value="pouch">Stand-up pouch · 5 panels</option><option value="bottle-label">Bottle label · cylindrical band</option>
      </select>
      <fieldset className="space-y-1.5 rounded border p-2"><legend className="px-1 text-[11px] font-medium">{packTemplate === "tube" || packTemplate === "bottle-label" ? "Label dimensions · mm" : "Finished dimensions · mm"}</legend>
        {packTemplate === "tube" || packTemplate === "bottle-label" ? <>
          <Row label={packTemplate === "tube" ? "Tube radius" : "Bottle label-zone radius"}><NumberField className="min-h-10" value={sleeve.radius} min={1} max={100_000} disabled={sleeveBusy} onCommit={(value) => setSleeve((current) => ({ ...current, radius: value, overlap: Math.min(current.overlap, Math.PI * value / 2) }))} ariaLabel="Label zone radius in millimeters" /></Row>
          <Row label="Wrap overlap"><NumberField className="min-h-10" value={sleeve.overlap} min={0} max={Math.PI * sleeve.radius / 2} step={0.5} disabled={sleeveBusy} onCommit={(value) => setSleeve((current) => ({ ...current, overlap: value }))} ariaLabel="Wrap overlap in millimeters" /></Row>
          <p className="px-1 text-[10px] leading-4 text-muted-foreground">The flat tail wraps over the label start. Maximum overlap is 25% of circumference ({(Math.PI * sleeve.radius / 2).toFixed(1)} mm). Bottle labels use a constant-radius zone, not a tapered bottle profile.</p>
        </> : packTemplate === "pouch" ? <>
          <Row label="Front width"><NumberField className="min-h-10" value={sleeve.length} min={1} max={100_000} disabled={sleeveBusy} onCommit={(value) => setSleeve((current) => ({ ...current, length: value }))} ariaLabel="Pouch front width in millimeters" /></Row>
          <Row label="Gusset depth"><NumberField className="min-h-10" value={sleeve.width} min={1} max={100_000} disabled={sleeveBusy} onCommit={(value) => setSleeve((current) => ({ ...current, width: value }))} ariaLabel="Pouch gusset depth in millimeters" /></Row>
        </> : <>
          <Row label="Length"><NumberField className="min-h-10" value={sleeve.length} min={1} max={100_000} disabled={sleeveBusy} onCommit={(value) => setSleeve((current) => ({ ...current, length: value }))} ariaLabel="Package length in millimeters" /></Row>
          <Row label="Width"><NumberField className="min-h-10" value={sleeve.width} min={1} max={100_000} disabled={sleeveBusy} onCommit={(value) => setSleeve((current) => ({ ...current, width: value }))} ariaLabel="Package width in millimeters" /></Row>
        </>}
        <Row label="Height"><NumberField className="min-h-10" value={sleeve.height} min={1} max={100_000} disabled={sleeveBusy} onCommit={(value) => setSleeve((current) => ({ ...current, height: value }))} ariaLabel="Sleeve height in millimeters" /></Row>
        {packTemplate !== "tube" && packTemplate !== "bottle-label" && packTemplate !== "pouch" && <Row label="Board"><NumberField className="min-h-10" value={sleeve.board} min={0} max={100} step={0.1} disabled={sleeveBusy} onCommit={(value) => setSleeve((current) => ({ ...current, board: value }))} ariaLabel="Board thickness in millimeters" /></Row>}
        <Row label="Bleed"><NumberField className="min-h-10" value={sleeve.bleed} min={0} max={100} step={0.5} disabled={sleeveBusy} onCommit={(value) => setSleeve((current) => ({ ...current, bleed: value }))} ariaLabel="Sleeve bleed in millimeters" /></Row>
        {packTemplate !== "sleeve" && packTemplate !== "tube" && packTemplate !== "bottle-label" && packTemplate !== "pouch" && <Row label="Glue flap"><NumberField className="min-h-10" value={sleeve.glueFlap} min={0.1} max={1000} step={0.5} disabled={sleeveBusy} onCommit={(value) => setSleeve((current) => ({ ...current, glueFlap: value }))} ariaLabel="Carton glue flap width in millimeters" /></Row>}
      </fieldset>
      <Button className="min-h-10 w-full" disabled={sleeveBusy || formBusy || !studio.doc || !studio.layerId || studio.doc.version !== 11} onClick={() => void createPackage()}>
        {sleeveBusy ? <Loader2 className="animate-spin" size={14} /> : <Plus size={14} />}{sleeveBusy ? "Building mockup…" : `Create ${packTemplate === "tube" ? "tube label" : packTemplate === "bottle-label" ? "bottle label" : packTemplate === "pouch" ? "pouch" : packTemplate === "sleeve" ? "sleeve" : "carton"} on active layer`}
      </Button>
      {studio.doc && studio.doc.version < 11 && <div className="space-y-1 rounded border border-warning/30 bg-warning/5 p-2 text-xs text-muted-foreground"><p>Packaging mockups use format v11.</p><Button variant="outline" className="min-h-10 w-full" disabled={studio.busy || sleeveBusy} onClick={() => void studio.upgradeDoc(11)}>Upgrade to v11</Button></div>}
      {sleeveError && <p role="alert" className="rounded border border-destructive/30 bg-destructive/5 p-2 text-xs text-destructive">{sleeveError} Adjust the dimensions and try again.</p>}
      {sleeveNotice && <p role="status" className="text-xs text-muted-foreground">{sleeveNotice} · {packTemplate === "tube" ? "the flat label maps to the measured circumference and overlap; tube ends are not included." : packTemplate === "bottle-label" ? "constant-radius visual wrap only; tapered bottle profiles are not included." : packTemplate === "pouch" ? "visual box-bottom net only; seals, zipper, and film behavior are not modeled." : "visual mockup only; board compensation and manufacturing tolerances are not applied."}</p>}
      {!studio.layerId && <p className="text-xs text-muted-foreground">Select an editable layer before creating a mockup.</p>}
    </section>
    <section className="space-y-2 rounded border bg-card p-2.5" aria-label="Product form creation" aria-busy={formBusy}>
      <h3 className={heading}>Product forms</h3>
      <label className="block space-y-1 text-xs text-muted-foreground" htmlFor="scene3d-product-form-preset">Start with a product</label>
      <select id="scene3d-product-form-preset" className="h-10 w-full rounded border bg-input px-2 text-sm text-foreground focus-visible:ring-2 focus-visible:ring-ring" value={formPreset} disabled={formBusy} onChange={(event) => setFormPreset(event.target.value)}>
        <option value="perfume-bottle">Perfume bottle</option><option value="cosmetic-jar">Cosmetic jar</option><option value="cosmetic-tube">Cosmetic tube</option><option value="shipping-carton">Shipping carton mockup</option><option value="product-card">Product card</option><option value="device-frame">Simple device frame</option><option value="stand-up-pouch">Stand-up pouch</option><option value="custom">Custom dimensions…</option>
      </select>
      {formPreset === "custom" && <>
        <label className="block space-y-1 text-xs text-muted-foreground" htmlFor="scene3d-product-form-kind">Custom form type</label>
        <select id="scene3d-product-form-kind" className="h-10 w-full rounded border bg-input px-2 text-sm text-foreground focus-visible:ring-2 focus-visible:ring-ring" value={formKind} disabled={formBusy} onChange={(event) => setFormKind(event.target.value as ProductFormKind)}>
          <option value="lathe-bottle">Bottle or jar · profile</option><option value="box">Box or carton</option><option value="pouch">Gusseted pouch</option><option value="cylinder">Cylinder</option><option value="tube">Open tube</option>
        </select>
      </>}
      <p className="text-[11px] leading-4 text-muted-foreground">Starter geometry is editable in millimeters. The carton is a visual box, not a folding dieline; the pouch is a simplified gusseted concept, not manufacturing CAD.</p>
      <Button className="min-h-10 w-full" disabled={formBusy || !studio.doc || !studio.layerId || (studio.doc.version !== 11)} onClick={() => void (formPreset === "custom" ? applyForm("add", formDefaults[formKind].parameters) : applyForm("add", {}, undefined, formPreset))}>
        {formBusy ? <Loader2 className="animate-spin" size={14} /> : <Plus size={14} />}{formBusy ? "Building…" : "Create on active layer"}
      </Button>
      {studio.doc && studio.doc.version < 11 && <div className="space-y-1 rounded border border-warning/30 bg-warning/5 p-2 text-xs text-muted-foreground"><p>Editable product forms use format v11.</p><Button variant="outline" className="min-h-10 w-full" disabled={studio.busy || formBusy} onClick={() => void studio.upgradeDoc(11)}>Upgrade to v11</Button></div>}
      {formError && <p role="alert" className="rounded border border-destructive/30 bg-destructive/5 p-2 text-xs text-destructive">{formError} No change was saved. Correct the value and try again.</p>}
      {formNotice && <p role="status" className="text-xs text-muted-foreground">{formNotice}</p>}
      {!studio.layerId && <p className="text-xs text-muted-foreground">Select an editable layer before creating a form.</p>}
    </section>
    {node && productForm && <section className="space-y-2 rounded border bg-card p-2.5" aria-label="Editable product form dimensions" aria-busy={formBusy}>
      <div><h3 className={heading}>Form dimensions · mm</h3><p className="mt-1 truncate text-[11px] text-muted-foreground">{formId} · {productForm.kind}</p></div>
      {productForm.kind === "box" && <>{formNumber("length_mm", "Length", 1)}{formNumber("width_mm", "Depth", 1)}{formNumber("height_mm", "Height", 1)}{formNumber("bevel_mm", "Bevel", 0, 100_000, 0.5)}</>}
      {productForm.kind === "pouch" && <>{formNumber("width_mm", "Width", 1)}{formNumber("height_mm", "Height", 1)}{formNumber("gusset_mm", "Gusset depth", 1)}</>}
      {(productForm.kind === "cylinder" || productForm.kind === "tube") && <>{formNumber("height_mm", "Height", 1)}{formNumber("outer_diameter_mm", "Outer Ø", 1)}{productForm.kind === "tube" && formNumber("inner_diameter_mm", "Inner Ø", 0.1)}{formNumber("radial_segments", "Segments", 8, 256)}</>}
      {productForm.kind === "cylinder" && <Check checked={productForm.parameters.capped !== false} onChange={(value) => patchForm({ capped: value })}>Closed ends</Check>}
      {productForm.kind === "lathe-bottle" && <>
        {formNumber("height_mm", "Height", 1)}{formNumber("radial_segments", "Segments", 8, 256)}
        <fieldset className="space-y-1.5 rounded border p-2"><legend className="px-1 text-[11px] font-medium">Bottle profile · radius / height</legend>
          <BottleProfileEditor profile={productForm.parameters.profile_mm as unknown[]} tangents={productForm.parameters.profile_tangents} height={Number(productForm.parameters.height_mm ?? 180)} disabled={formBusy} onCommit={(profile, tangents) => patchForm({ profile_mm: profile, profile_tangents: tangents })} />
          <div className="grid grid-cols-[auto_1fr_1fr_auto] gap-1.5 px-1 text-[10px] text-muted-foreground"><span>Point</span><span>Radius · mm</span><span>Height · mm</span><span className="sr-only">Remove point</span></div>
          {Array.isArray(productForm.parameters.profile_mm) && productForm.parameters.profile_mm.map((point, index, profile) => Array.isArray(point) && <div key={index} className="grid grid-cols-[auto_1fr_1fr_auto] items-center gap-1.5"><span className="min-w-7 text-center text-[10px] text-muted-foreground">P{index + 1}</span><NumberField className="min-h-10 min-w-0" value={Number(point[0])} min={0.1} max={100_000} step={0.5} disabled={formBusy} onCommit={(value) => updateProfilePoint(index, 0, value)} ariaLabel={`Profile point ${index + 1} radius in millimeters`} /><NumberField className="min-h-10 min-w-0" value={Number(point[1])} min={index > 0 ? Number((profile[index - 1] as unknown[])[1]) + 0.01 : 0} max={index < profile.length - 1 ? Number((profile[index + 1] as unknown[])[1]) - 0.01 : Number(productForm.parameters.height_mm ?? 180)} step={0.5} disabled={formBusy || index === 0 || index === profile.length - 1} onCommit={(value) => updateProfilePoint(index, 1, value)} ariaLabel={`Profile point ${index + 1} height in millimeters`} /><Button type="button" variant="outline" className="min-h-10 min-w-10 px-2" aria-label={`Remove profile point ${index + 1}`} title={`Remove profile point ${index + 1}`} disabled={formBusy || index === 0 || index === profile.length - 1 || profile.length <= 3} onClick={() => removeProfilePoint(index)}><Trash2 size={14} aria-hidden="true" /></Button></div>)}
          <Button type="button" variant="outline" className="min-h-10 w-full" disabled={formBusy || !Array.isArray(productForm.parameters.profile_mm) || productForm.parameters.profile_mm.length >= 128} onClick={addProfilePoint}><Plus size={14} aria-hidden="true" />Add profile point</Button>
          <p className="text-[10px] leading-4 text-muted-foreground">The base starts at 0 mm and the final point follows total height. Middle heights must stay in ascending order.</p>
        </fieldset>
        <Check checked={!!productForm.parameters.cap_mm} disabled={formBusy} onChange={toggleBottleCap}>Separate cap material region</Check>
        {productForm.parameters.cap_mm && typeof productForm.parameters.cap_mm === "object" && <fieldset className="space-y-1.5 rounded border p-2" aria-busy={formBusy}>
          <legend className="px-1 text-[11px] font-medium">Cap · mm</legend>
          <p className="text-[10px] leading-4 text-muted-foreground">The cap crown and skirt share a selectable cap slot. Offset lifts or lowers both pieces from the bottle mouth.</p>
          <Row label="Radius"><NumberField className="min-h-10" value={Number((productForm.parameters.cap_mm as Record<string, unknown>).radius_mm ?? 9)} min={0.1} max={100_000} step={0.5} disabled={formBusy} onCommit={(value) => patchForm({ cap_mm: { ...(productForm.parameters.cap_mm as Record<string, unknown>), radius_mm: value } })} ariaLabel="Bottle cap radius in millimeters" /></Row>
          <Row label="Height"><NumberField className="min-h-10" value={Number((productForm.parameters.cap_mm as Record<string, unknown>).height_mm ?? 18)} min={0.1} max={Number(productForm.parameters.height_mm ?? 180)} step={0.5} disabled={formBusy} onCommit={(value) => patchForm({ cap_mm: { ...(productForm.parameters.cap_mm as Record<string, unknown>), height_mm: value } })} ariaLabel="Bottle cap height in millimeters" /></Row>
          <Row label="Offset"><NumberField className="min-h-10" value={Number((productForm.parameters.cap_mm as Record<string, unknown>).offset_mm ?? 0)} min={Number((productForm.parameters.cap_mm as Record<string, unknown>).height_mm ?? 18) - Number(productForm.parameters.height_mm ?? 180)} max={100_000 - Number(productForm.parameters.height_mm ?? 180)} step={0.5} disabled={formBusy} onCommit={(value) => patchForm({ cap_mm: { ...(productForm.parameters.cap_mm as Record<string, unknown>), offset_mm: value } })} ariaLabel="Bottle cap vertical offset from the mouth in millimeters" /></Row>
        </fieldset>}
      </>}
      <div className="rounded border bg-muted/50 p-2 text-[11px] text-muted-foreground">{(productForm.material_slots ?? []).length} material regions · changing these dimensions regenerates the 3D form deterministically.</div>
    </section>}
    <section className="space-y-2">
      <h3 className={heading}>Static 3D</h3>
      <Button className="min-h-10" variant="outline" disabled={importing || !studio.doc || studio.doc.version < 10} onClick={() => input.current?.click()}>
        {importing ? <Loader2 className="animate-spin" size={14} /> : <Box size={14} />}{importing ? "Importing…" : "Import model…"}
      </Button>
      <input ref={input} hidden type="file" accept=".glb,.gltf,.obj,model/gltf-binary,model/gltf+json" onChange={(event) => { const file = event.target.files?.[0]; if (file) void importFile(file); }} />
      {studio.doc && studio.doc.version < 10 && <p className="text-xs text-muted-foreground">3D needs format v10. Use the format upgrade in the top bar.</p>}
      <div role="status" className="rounded border bg-muted px-2 py-1.5 text-[11px]"><span className="font-medium">Renderer: reference CPU</span>{status?.adapter && <span className="block text-muted-foreground">Adapter: {status.adapter}</span>}{status?.fallback_reason && <span className="block text-muted-foreground">{status.fallback_reason}</span>}</div>
    </section>
    {!node ? <Empty>Import or select a 3D object to edit its view, lights, and materials.</Empty> : <>
      {dieline && dieline.template !== "tube" && <section className="space-y-2 rounded border bg-card p-2.5" aria-label="Package fold preview" aria-busy={foldBusy || creaseBusy}>
        <h3 className={heading}>Package fold</h3>
        <label className="block text-xs" htmlFor="scene3d-fold-state">
          <span className="flex justify-between"><span>Flat to folded</span><span className="tabular-nums text-muted-foreground">{Math.round(shownFoldState * 100)}%</span></span>
          <input id="scene3d-fold-state" type="range" min={0} max={1} step={0.01} value={shownFoldState} disabled={foldBusy || creaseBusy} className="mt-1 min-h-10 w-full accent-[var(--primary)] focus-visible:ring-2 focus-visible:ring-ring" aria-valuetext={`${Math.round(shownFoldState * 100)} percent folded`} onChange={(event) => {
            const value = Number(event.target.value);
            setFoldDrag(value);
            previewFold(value);
          }} onPointerUp={(event) => {
            const value = Number(event.currentTarget.value);
            void commitFold(value);
          }} onKeyUp={(event) => {
            const value = Number(event.currentTarget.value);
            void commitFold(value);
          }} onBlur={(event) => {
            if (foldDrag === null) return;
            const value = Number(event.currentTarget.value);
            void commitFold(value);
          }} />
        </label>
        <p className="text-[11px] text-muted-foreground">Drag to preview; release to save one undoable fold-state change.</p>
        {(dieline.panels?.length ?? 0) > 0 && <fieldset className="space-y-1.5 rounded border p-2">
          <legend className="px-1 text-[11px] font-medium">Folded face · linked to flat panel</legend>
          <p className="text-[11px] text-muted-foreground">Click a visible face in the folded preview to select it, or choose its stable panel ID here. The same ID highlights the flat region and opens that panel’s artwork controls.</p>
          <div className="flex flex-wrap gap-2" role="group" aria-label="Select folded package face">
            {(dieline.panels ?? []).map((panel) => <Button key={`folded-${panel.id}`} type="button" variant={selectedPanel === panel.id ? "default" : "outline"} aria-pressed={selectedPanel === panel.id} aria-label={`Select folded face ${panel.id}`} className="min-h-10" onClick={() => setSelectedPanel(panel.id)}>{panel.id}</Button>)}
          </div>
        </fieldset>}
        {(dieline.creases?.length ?? 0) > 0 && <fieldset className="space-y-1.5 rounded border p-2" aria-busy={creaseBusy}>
          <legend className="px-1 text-[11px] font-medium">Per-crease target angle</legend>
          <Row label="Crease"><select aria-label="Select package crease" className="min-h-10 min-w-0 flex-1 rounded border bg-input px-2 text-xs text-foreground focus-visible:ring-2 focus-visible:ring-ring" value={selectedCrease ?? ""} disabled={creaseBusy || foldBusy || foldDrag !== null} onChange={(event) => setSelectedCrease(event.target.value)}>
            {(dieline.creases ?? []).map((crease) => <option key={crease.id} value={crease.id}>{crease.id} · {crease.direction}</option>)}
          </select></Row>
          {activeCrease && <Row label="Target angle"><NumberField className="min-h-10" value={activeCrease.angle_deg} min={-180} max={180} step={5} disabled={creaseBusy || foldBusy || foldDrag !== null} onCommit={(angle) => void setCreaseAngle(activeCrease.id, angle)} ariaLabel={`${activeCrease.id} target fold angle in degrees`} /></Row>}
          {activeCrease && <Row label="Fold direction"><select aria-label={`${activeCrease.id} fold direction`} className="min-h-10 min-w-0 flex-1 rounded border bg-input px-2 text-xs text-foreground focus-visible:ring-2 focus-visible:ring-ring" value={activeCrease.direction} disabled={creaseBusy || foldBusy || foldDrag !== null} onChange={(event) => void setCreaseDirection(activeCrease.id, event.target.value as "mountain" | "valley")}><option value="valley">Valley fold</option><option value="mountain">Mountain fold</option></select></Row>}
          <p className="text-[10px] leading-4 text-muted-foreground">Angle sets the fully folded amount; direction flips the hinge. The shared slider previews all crease settings together.</p>
          {creaseBusy && <p role="status" className="text-xs text-muted-foreground">Saving crease settings…</p>}
          {creaseError && <div role="alert" className="space-y-1 text-xs text-destructive"><p>{creaseError} Your document was not changed.</p>{creaseRetry && <Button type="button" variant="outline" className="min-h-10" disabled={creaseBusy} onClick={() => void setCreaseAngle(creaseRetry.id, creaseRetry.angle)}>Retry angle update</Button>}{creaseDirectionRetry && <Button type="button" variant="outline" className="min-h-10" disabled={creaseBusy} onClick={() => void setCreaseDirection(creaseDirectionRetry.id, creaseDirectionRetry.direction)}>Retry fold direction</Button>}</div>}
        </fieldset>}
        {foldError && <p role="alert" className="rounded border border-destructive/30 bg-destructive/5 p-2 text-xs text-destructive">{foldError} The saved document was not changed.</p>}
      </section>}
      {dieline && <section className="space-y-2 rounded border bg-card p-2.5" aria-label="Package print and material settings" aria-busy={settingsBusy}>
        <h3 className={heading}>Print &amp; material</h3>
        <Row label="Board mm"><NumberField className="min-h-10" value={dieline.dimensions_mm?.board ?? 0} min={0} max={100} step={0.1} disabled={settingsBusy || flatPreviewBusy || packPreflightBusy || foldBusy || creaseBusy || panelArtBusy} onCommit={(board_mm) => void setPackagingSettings({ board_mm })} ariaLabel="Package board thickness in millimeters" /></Row>
        <Row label="Bleed mm"><NumberField className="min-h-10" value={dieline.dimensions_mm?.bleed ?? 0} min={0} max={100} step={0.1} disabled={settingsBusy || flatPreviewBusy || packPreflightBusy || foldBusy || creaseBusy || panelArtBusy} onCommit={(bleed_mm) => void setPackagingSettings({ bleed_mm })} ariaLabel="Package print bleed in millimeters" /></Row>
        {dieline.wrap && <fieldset className="space-y-1.5 rounded border p-2">
          <legend className="px-1 text-[11px] font-medium">Cylindrical label wrap</legend>
          <Row label="Seam °"><NumberField className="min-h-10" value={dieline.wrap.seam_angle_deg} min={-360} max={360} step={5} disabled={settingsBusy || flatPreviewBusy || packPreflightBusy || foldBusy || creaseBusy || panelArtBusy} onCommit={(seam_angle_deg) => void setPackagingSettings({ seam_angle_deg })} ariaLabel="Label wrap seam angle in degrees" /></Row>
          <Row label="Direction"><select aria-label="Cylindrical label wrap direction" className="min-h-10 min-w-0 flex-1 rounded border bg-input px-2 text-xs text-foreground focus-visible:ring-2 focus-visible:ring-ring" value={dieline.wrap.direction as "clockwise" | "counterclockwise"} disabled={settingsBusy || flatPreviewBusy || packPreflightBusy || foldBusy || creaseBusy || panelArtBusy} onChange={(event) => void setPackagingSettings({ wrap_direction: event.target.value as "clockwise" | "counterclockwise" })}><option value="clockwise">Clockwise</option><option value="counterclockwise">Counterclockwise</option></select></Row>
          <p className="text-[10px] leading-4 text-muted-foreground">The seam marker and folded label update together. Keep critical artwork away from the seam and inspect the wrap preflight warning.</p>
        </fieldset>}
        <p className="text-[10px] leading-4 text-muted-foreground">Bleed expands flat artwork/export bounds; board thickness is used by the limited overlap preflight, not as certified fold compensation.</p>
        {settingsBusy && <p role="status" className="text-xs text-muted-foreground">Saving print settings…</p>}
        {settingsError && <div role="alert" className="space-y-1 text-xs text-destructive"><p>{settingsError} Your document was not changed.</p>{settingsRetry && <Button type="button" variant="outline" className="min-h-10" disabled={settingsBusy} onClick={() => void setPackagingSettings(settingsRetry)}>Retry settings update</Button>}</div>}
      </section>}
      {dieline?.layout_page && <section className="space-y-2 rounded border bg-card p-2.5" aria-label="Editable flat package layout">
        <h3 className={heading}>Editable flat layout</h3>
        <p className="text-[11px] text-muted-foreground">Open the linked `.pen` page to edit panel guides and production paths with ordinary canvas tools. Stable panel and crease IDs keep the folded preview in sync.</p>
        <Button type="button" className="min-h-10 w-full" variant="outline" onClick={() => {
          const state = useStudio.getState();
          state.setWorkspace("design");
          state.setPage(dieline.layout_page!);
        }}>Edit flat layout page</Button>
      </section>}
      {dieline && studio.workspaceId !== "3d" && <section className="space-y-2 rounded border bg-card p-2.5" aria-label="Flat dieline preview" aria-busy={flatPreviewBusy}>
        <div className="flex items-center justify-between gap-2"><h3 className={heading}>Flat dieline</h3><Button className="min-h-10" variant="outline" disabled={flatPreviewBusy} onClick={() => void runFlatPreview()}>{flatPreviewBusy && <Loader2 className="animate-spin" size={14} />}{flatPreviewBusy ? "Rendering…" : flatPreview ? "Refresh" : flatPreviewError ? "Try again" : "Preview flat"}</Button></div>
        {!flatPreview && !flatPreviewBusy && !flatPreviewError && <p className="text-[11px] text-muted-foreground">Render linked panel art and production paths from the shared Rust exporter.</p>}
        {flatPreviewBusy && <p role="status" className="text-xs text-muted-foreground">Rendering the flat dieline and resolving linked artwork…</p>}
        {flatPreviewError && <div role="alert" className="space-y-2 rounded border border-destructive/30 bg-destructive/5 p-2 text-xs text-destructive"><p>The flat preview could not be rendered.</p><p>{flatPreviewError}</p><Button className="min-h-10" variant="outline" disabled={flatPreviewBusy} onClick={() => void runFlatPreview()}>Retry preview</Button></div>}
        {flatPreview && <>
          <div className="relative w-full overflow-hidden rounded border bg-background" style={{ aspectRatio: `${flatBounds.width} / ${flatBounds.height}` }}>
            <FlatDielineImage svg={flatPreview.svg} alt={`${dielineId} flat dieline with linked artwork and technical paths`} />
            {(dieline.panels ?? []).map((panel) => {
              if (!panel.artwork) return selectedPanel === panel.id ? <div key={panel.id} aria-hidden="true" className="pointer-events-none absolute border-2 border-primary bg-primary/10" style={{ left: `${panel.rect_mm[0] / flatBounds.width * 100}%`, top: `${panel.rect_mm[1] / flatBounds.height * 100}%`, width: `${panel.rect_mm[2] / flatBounds.width * 100}%`, height: `${panel.rect_mm[3] / flatBounds.height * 100}%` }} /> : null;
              const transform = panelDragPreview?.panel === panel.id
                ? panelDragPreview.transform
                : panel.artwork_transform ?? defaultPanelArtworkTransform;
              const left = panel.rect_mm[0] / flatBounds.width * 100;
              const top = panel.rect_mm[1] / flatBounds.height * 100;
              const width = panel.rect_mm[2] / flatBounds.width * 100;
              const height = panel.rect_mm[3] / flatBounds.height * 100;
              const isSelected = selectedPanel === panel.id;
              return <button
                key={panel.id}
                type="button"
                aria-label={`Drag artwork on ${panel.id} panel to reposition; use placement fields for keyboard adjustment`}
                aria-pressed={isSelected}
                title={`Drag to reposition artwork on ${panel.id}`}
                disabled={panelArtBusy}
                className={`absolute touch-none border-2 focus-visible:z-10 focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2 ${isSelected ? "border-primary bg-primary/10" : "border-transparent hover:border-primary/70 hover:bg-primary/5"}`}
                style={{ left: `${left}%`, top: `${top}%`, width: `${width}%`, height: `${height}%` }}
                onClick={() => setSelectedPanel(panel.id)}
                onPointerDown={(event) => {
                  if (event.button !== 0) return;
                  event.preventDefault();
                  setSelectedPanel(panel.id);
                  const sourceTransform = panel.artwork_transform ?? defaultPanelArtworkTransform;
                  panelDrag.current = { panel: panel.id, transform: { ...sourceTransform, scale: [...sourceTransform.scale], offset: [...sourceTransform.offset] }, origin: [...sourceTransform.offset] };
                  event.currentTarget.setPointerCapture(event.pointerId);
                }}
                onPointerMove={(event) => onPanelArtworkPointerMove(event, panel.id)}
                onPointerUp={(event) => {
                  if (event.currentTarget.hasPointerCapture(event.pointerId)) event.currentTarget.releasePointerCapture(event.pointerId);
                  finishPanelArtworkPointer(panel.id, true);
                }}
                onPointerCancel={(event) => {
                  if (event.currentTarget.hasPointerCapture(event.pointerId)) event.currentTarget.releasePointerCapture(event.pointerId);
                  finishPanelArtworkPointer(panel.id, false);
                }}
              >
                <span aria-hidden="true" className="pointer-events-none absolute border border-dashed border-primary" style={{ left: `${(transform.offset[0] - transform.scale[0] / 2) * 100}%`, top: `${(transform.offset[1] - transform.scale[1] / 2) * 100}%`, width: `${transform.scale[0] * 100}%`, height: `${transform.scale[1] * 100}%` }} />
              </button>;
            })}
          </div>
          <p className="text-[11px] text-muted-foreground">Drag linked artwork within its panel to reposition it. Keyboard users can select a panel and adjust its center below; the same panel ID maps to the folded face.</p>
          <div className="flex flex-wrap gap-1" aria-label="Select flat dieline panel">
            {(dieline.panels ?? []).map((panel) => <Button key={panel.id} type="button" variant={selectedPanel === panel.id ? "default" : "outline"} aria-pressed={selectedPanel === panel.id} aria-label={`Highlight panel ${panel.id}`} className="min-h-10" onClick={() => setSelectedPanel(panel.id)}>{panel.id}</Button>)}
          </div>
          {flatPreview.report.artwork_panels.length === 0 && <p className="rounded border bg-muted/50 p-2 text-[11px] text-muted-foreground">This net has no linked panel artwork yet. Choose a page or node below, then refresh the preview.</p>}
          {flatPreview.report.warnings.length > 0 && <ul className="list-disc space-y-1 pl-4 text-[11px] text-muted-foreground">{flatPreview.report.warnings.map((warning) => <li key={warning}>{warning}</li>)}</ul>}
        </>}
      </section>}
      {dieline && <section className="space-y-2 rounded border bg-card p-2.5" aria-label="Packaging preflight" aria-busy={packPreflightBusy}>
        <div className="flex items-center justify-between gap-2"><h3 className={heading}>Artwork preflight</h3><Button className="min-h-10" variant="outline" disabled={packPreflightBusy} onClick={() => void runPackPreflight()}>{packPreflightBusy && <Loader2 className="animate-spin" size={14} />}{packPreflightBusy ? "Checking…" : packPreflight ? "Run again" : "Run preflight"}</Button></div>
        {!packPreflight && !packPreflightBusy && !packPreflightError && <p className="text-[11px] text-muted-foreground">Checks linked artwork, raster resolution at 300 ppi, hinge continuity, and zero-thickness intersections in the current folded preview. This is a design check, not manufacturing approval.</p>}
        {packPreflightBusy && <p role="status" className="text-xs text-muted-foreground">Checking the current dieline and linked artwork…</p>}
        {packPreflightError && <div role="alert" className="space-y-1 rounded border border-destructive/30 bg-destructive/5 p-2 text-xs text-destructive"><p>Preflight could not be completed.</p><p>{packPreflightError}</p><p className="text-muted-foreground">Your document was not changed. Check linked artwork and retry.</p></div>}
        {packPreflight && <div className="space-y-2 text-[11px]">
          <p role="status" className="font-medium">{packPreflight.preflight_status === "mockup-ready" ? "No preflight warnings found" : "Review needed"} · {packPreflight.panel_art_bound}/{packPreflight.panel_count} panels linked</p>
          {packPreflight.fold_hinge_checks.length > 0 && <p className="text-muted-foreground">Fold continuity · {packPreflight.fold_hinge_checks.filter((check) => check.joined).length}/{packPreflight.fold_hinge_checks.length} seams joined within tolerance</p>}
          {packPreflight.fold_hinge_checks.filter((check) => !check.joined).map((check) => <p key={`${check.kind}:${check.panels.join(":")}`} className="text-warning">{check.kind === "wrap-seam" ? "Wrap seam" : `Fold seam · ${check.panels.join(" ↔ ")}`}: {check.gap_mm.toFixed(4)} mm gap (tolerance {check.tolerance_mm.toFixed(4)} mm)</p>)}
          {packPreflight.fold_collision_report && <p className={packPreflight.fold_collision_report.collision_count ? "text-warning" : "text-muted-foreground"}>Folded-face intersections · {packPreflight.fold_collision_report.collision_count} potential overlap{packPreflight.fold_collision_report.collision_count === 1 ? "" : "s"} across {packPreflight.fold_collision_report.pairs_tested} non-adjacent panel pairs at {Math.round(packPreflight.fold_collision_report.fold_state * 100)}% fold</p>}
          {packPreflight.fold_collision_report?.collisions.map((collision) => <p key={`collision-${collision.panels.join(":")}`} className="pl-2 text-warning">Inspect {collision.panels.join(" ↔ ")} · zero-thickness faces intersect</p>)}
          {packPreflight.rasterized_ppi.map((item) => <p key={item.panel} className="text-muted-foreground">{item.panel}: {item.effective_ppi.toFixed(0)} ppi{item.effective_ppi < packPreflight.minimum_ppi ? " · below minimum" : ""}</p>)}
          {packPreflight.safe_zone_checks.map((item) => <p key={`safe-${item.panel}`} className={item.within_safe_zone ? "text-muted-foreground" : "text-warning"}>{item.panel}: {item.inset_mm === null ? "safe area not configured" : item.within_safe_zone ? `inside ${item.inset_mm} mm safe area` : `artwork exceeds ${item.inset_mm} mm safe area`}</p>)}
          {packPreflight.warnings.length > 0 && <ul className="list-disc space-y-1 pl-4 text-muted-foreground">{packPreflight.warnings.map((warning) => <li key={warning}>{warning}</li>)}</ul>}
          <p className="rounded border bg-muted/50 p-2 text-muted-foreground">Not manufacturing-certified. The fold scan checks zero-thickness faces only; board/film thickness, physical gaps, structural checks, and PDF/X separations are not certified.</p>
        </div>}
      </section>}
      {dieline && <section className="space-y-2 rounded border bg-card p-2.5" aria-label="Package panel artwork" aria-busy={panelArtBusy}>
        <h3 className={heading}>Panel artwork</h3>
        {(dieline.panels ?? []).map((panel) => <Row key={panel.id} label={panel.id}>
          <select aria-label={`${panel.id} panel artwork source`} className="min-h-10 min-w-0 flex-1 rounded border bg-input px-2 text-xs text-foreground focus-visible:ring-2 focus-visible:ring-ring" disabled={panelArtBusy} value={panelSourceValue(panel.artwork)} onChange={(event) => void setPanelArtwork(panel.id, event.target.value)}>
            <option value="">No artwork</option>
            {(studio.doc?.pages ?? []).map((page) => <option key={`page:${page.id}`} value={`page:${page.id}`}>Page · {page.name || page.id}</option>)}
            {artworkSources.map(({ page, node: item }) => <option key={`node:${page}:${item.id}`} value={`node:${page}:${item.id}`}>Node · {item.name || item.id}</option>)}
          </select>
        </Row>)}
        {activePanel?.artwork ? <fieldset className="space-y-1.5 rounded border p-2" aria-busy={panelArtBusy}>
          <legend className="px-1 text-[11px] font-medium">Placement · {activePanel.id}</legend>
          <p className="text-[10px] leading-4 text-muted-foreground">Scale and center use panel fractions. These edits update both flat art and the folded face; rotation is clipped to the panel.</p>
          <Row label="Scale X"><NumberField className="min-h-10" value={activePanelTransform.scale[0]} min={0.000001} max={Math.max(0.000001, Math.min(1, 2 * activePanelTransform.offset[0], 2 * (1 - activePanelTransform.offset[0])))} step={0.05} disabled={panelArtBusy} onCommit={(value) => void setPanelArtworkTransform(activePanel.id, { ...activePanelTransform, scale: [value, activePanelTransform.scale[1]] })} ariaLabel={`${activePanel.id} artwork width as panel fraction`} /></Row>
          <Row label="Scale Y"><NumberField className="min-h-10" value={activePanelTransform.scale[1]} min={0.000001} max={Math.max(0.000001, Math.min(1, 2 * activePanelTransform.offset[1], 2 * (1 - activePanelTransform.offset[1])))} step={0.05} disabled={panelArtBusy} onCommit={(value) => void setPanelArtworkTransform(activePanel.id, { ...activePanelTransform, scale: [activePanelTransform.scale[0], value] })} ariaLabel={`${activePanel.id} artwork height as panel fraction`} /></Row>
          <Row label="Center X"><NumberField className="min-h-10" value={activePanelTransform.offset[0]} min={activePanelTransform.scale[0] / 2} max={1 - activePanelTransform.scale[0] / 2} step={0.05} disabled={panelArtBusy} onCommit={(value) => void setPanelArtworkTransform(activePanel.id, { ...activePanelTransform, offset: [value, activePanelTransform.offset[1]] })} ariaLabel={`${activePanel.id} artwork horizontal center as panel fraction`} /></Row>
          <Row label="Center Y"><NumberField className="min-h-10" value={activePanelTransform.offset[1]} min={activePanelTransform.scale[1] / 2} max={1 - activePanelTransform.scale[1] / 2} step={0.05} disabled={panelArtBusy} onCommit={(value) => void setPanelArtworkTransform(activePanel.id, { ...activePanelTransform, offset: [activePanelTransform.offset[0], value] })} ariaLabel={`${activePanel.id} artwork vertical center as panel fraction`} /></Row>
          <Row label="Rotation"><NumberField className="min-h-10" value={activePanelTransform.rotate} min={-360} max={360} step={5} disabled={panelArtBusy} onCommit={(value) => void setPanelArtworkTransform(activePanel.id, { ...activePanelTransform, rotate: value })} ariaLabel={`${activePanel.id} artwork rotation in degrees`} /></Row>
        </fieldset> : <p className="rounded border bg-muted/50 p-2 text-[11px] text-muted-foreground">Select a page or node for a panel to edit its flat and folded artwork placement.</p>}
        {activePanel && <fieldset className="space-y-1.5 rounded border p-2" aria-busy={panelArtBusy}>
          <legend className="px-1 text-[11px] font-medium">Print safe area · {activePanel.id}</legend>
          <p className="text-[10px] leading-4 text-muted-foreground">Inset from the panel edge in millimeters. This guide and bounds check are not bleed or manufacturing approval.</p>
          <Row label="Inset"><NumberField className="min-h-10" value={activePanel.safe_zone_mm ?? 0} min={0} max={Math.max(0, Math.min(activePanel.rect_mm[2], activePanel.rect_mm[3]) / 2 - 0.000001)} step={0.5} disabled={panelArtBusy} onCommit={(value) => void setPanelSafeZone(activePanel.id, value)} ariaLabel={`${activePanel.id} safe-area inset in millimeters`} /></Row>
        </fieldset>}
        <p className="text-[11px] text-muted-foreground">Links stay editable; the same Rust render path paints the folded faces.</p>
        {panelArtBusy && <p role="status" className="text-xs text-muted-foreground">Saving panel artwork…</p>}
        {panelArtNotice && <p role="status" className="text-xs text-muted-foreground">{panelArtNotice}</p>}
        {panelArtError && <div role="alert" className="space-y-2 rounded border border-destructive/30 bg-destructive/5 p-2 text-xs text-destructive"><p>{panelArtError} The saved artwork and document are unchanged.</p>{panelTransformRetry && <Button type="button" variant="outline" className="min-h-10" disabled={panelArtBusy} onClick={() => void setPanelArtworkTransform(panelTransformRetry.panel, panelTransformRetry.transform)}>Retry placement</Button>}{panelSafeZoneRetry && <Button type="button" variant="outline" className="min-h-10" disabled={panelArtBusy} onClick={() => void setPanelSafeZone(panelSafeZoneRetry.panel, panelSafeZoneRetry.inset)}>Retry safe-area guide</Button>}</div>}
      </section>}
      <section className="space-y-2">
        <h3 className={heading}>Camera · {node.id}</h3>
        <div role="group" aria-label="Camera presets" className="grid grid-cols-2 gap-1.5">
          {([ ["Front", 0, 0], ["Back", 180, 0], ["Left", -90, 0], ["Right", 90, 0], ["Top", 0, 80], ["Three-quarter", 30, 15] ] as const).map(([label, azimuth, elevation]) => <Button key={label} type="button" variant="outline" className="min-h-12 focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2" aria-label={`Camera preset ${label}`} onClick={() => applyCameraPreset(azimuth, elevation)}>{label}</Button>)}
        </div>
        <Row label="Orbit X"><NumberField value={Number(cam.azimuth ?? 30)} min={-360} max={360} onCommit={(v) => cameraValue("azimuth", v)} ariaLabel="Camera azimuth" /></Row>
        <Row label="Orbit Y"><NumberField value={Number(cam.elevation ?? 15)} min={-89} max={89} onCommit={(v) => cameraValue("elevation", v)} ariaLabel="Camera elevation" /></Row>
        <Row label="Zoom"><NumberField value={Number(cam.fov ?? 40)} min={5} max={120} onCommit={(v) => cameraValue("fov", v)} ariaLabel="Camera field of view" /></Row>
        <div className="grid grid-cols-3 gap-1" aria-label="Pan camera target">{["X", "Y", "Z"].map((axis, index) => <Button key={axis} variant="outline" className="min-h-10" onClick={() => cameraValue("target", target.map((v, i) => i === index ? v + 0.1 : v))}>Pan {axis}+</Button>)}</div>
        <Button variant="outline" className="min-h-10 w-full" onClick={() => change((draft) => { delete draft.camera; delete draft.physical_camera; })}>Frame model</Button>
      </section>
      <section className="space-y-2">
        <h3 className={heading}>Lighting</h3>
        <Row label="Exposure"><NumberField value={Number(node.exposure_ev ?? 0)} min={-10} max={10} step={0.25} onCommit={(v) => change((draft) => { draft.exposure_ev = v; })} ariaLabel="Exposure in EV" /></Row>
        {light && <>
          <Row label="Light"><select aria-label="Selected light" className="h-8 w-full rounded border bg-input px-2 text-xs" value={lightIndex} onChange={(event) => setLightIndex(Number(event.target.value))}>{lights.map((entry, index) => <option key={index} value={index}>{index + 1}. {entry.kind}</option>)}</select></Row>
          <Row label="Intensity"><NumberField value={Number(light.intensity)} min={0} max={100000} step={0.25} onCommit={(v) => editLight({ intensity: v })} ariaLabel="Light intensity" /></Row>
          <Row label="Color"><input aria-label="Light color" type="color" className="h-10 w-full rounded border bg-input p-1" value={light.color} onChange={(event) => editLight({ color: event.target.value })} /></Row>
          <Button className="min-h-10 w-full" variant="outline" onClick={() => change((draft) => { const next = [...((draft.lights as Light[] | undefined) ?? [])]; next.splice(lightIndex, 1); draft.lights = next; setLightIndex(Math.max(0, lightIndex - 1)); })}>Remove light</Button>
        </>}
        <div className="grid grid-cols-3 gap-1">{(["directional", "point", "spot"] as const).map((kind) => <Button key={kind} className="min-h-10 capitalize" variant="outline" onClick={() => change((draft) => { const next = [...((draft.lights as Light[] | undefined) ?? []), newLight(kind)]; draft.lights = next; setLightIndex(next.length - 1); })}>{kind}</Button>)}</div>
        <div className="flex gap-1"><Button className="min-h-10 flex-1" variant="outline" onClick={() => change((draft) => { delete draft.lights; })}>Default rig</Button><Button className="min-h-10 flex-1" variant="outline" onClick={() => change((draft) => { draft.lights = []; })}>No lights</Button></div>
      </section>
      <section className="space-y-2 rounded border bg-card p-2.5" aria-label="Ground plane settings">
        <h3 className={heading}>Ground plane</h3>
        <Check checked={groundPlane.enabled} onChange={(enabled) => patchGroundPlane({ enabled })}>Show explicit ground plane</Check>
        {groundPlane.enabled && <>
          <Row label="Color"><input aria-label="Ground plane color" type="color" className="h-10 w-full rounded border bg-input p-1 focus-visible:ring-2 focus-visible:ring-ring" value={(groundPlane.color ?? "#d8d5d0").slice(0, 7)} onChange={(event) => patchGroundPlane({ color: event.target.value })} /></Row>
          <Row label="Render mode"><select aria-label="Ground plane render mode" className="h-10 w-full rounded border bg-input px-2" value={groundPlane.mode ?? "opaque"} onChange={(event) => patchGroundPlane({ mode: event.target.value as GroundPlane["mode"] })}><option value="opaque">Opaque floor</option><option value="shadow-catcher">Transparent shadow catcher</option></select></Row>
          {groundPlane.mode === "shadow-catcher" && <Row label="Shadow opacity"><NumberField value={groundPlane.opacity ?? 0.65} min={0} max={1} step={0.05} onCommit={(value) => patchGroundPlane({ opacity: value })} ariaLabel="Shadow catcher opacity" /></Row>}
          <Row label="Footprint"><NumberField value={groundPlane.size_multiplier ?? 2.5} min={1} max={4} step={0.1} onCommit={(value) => patchGroundPlane({ size_multiplier: value })} ariaLabel="Ground plane size multiplier" /></Row>
          <Row label="Gap below object"><NumberField value={groundPlane.offset ?? 0} min={0} max={0.25} step={0.01} onCommit={(value) => patchGroundPlane({ offset: value })} ariaLabel="Ground plane offset as a fraction of object diagonal" /></Row>
          <p className="text-[10px] leading-4 text-muted-foreground">Shadow-catcher mode exports shadow pixels over transparency; disable shadows in the lighting settings to omit them.</p>
        </>}
      </section>
      <section className="space-y-2">
        <h3 className={heading}>Material override</h3>
        <Row label="Material"><NumberField value={material} min={0} step={1} onCommit={(v) => setMaterial(Math.floor(v))} ariaLabel="Material index" /></Row>
        <Row label="Metallic"><NumberField value={currentOverride("metallic", 0)} min={0} max={1} step={0.05} onCommit={(v) => override("metallic", v)} ariaLabel="Material metallic value" /></Row>
        <Row label="Roughness"><NumberField value={currentOverride("roughness", 1)} min={0} max={1} step={0.05} onCommit={(v) => override("roughness", v)} ariaLabel="Material roughness value" /></Row>
        <Row label="Base"><input aria-label="Material base color" type="color" className="h-10 w-full rounded border bg-input p-1" value={String(overrideRecord.base_color ?? "#ffffff").slice(0, 7)} onChange={(event) => override("base_color", event.target.value)} /></Row>
        <Row label="Emissive"><input aria-label="Material emissive color" type="color" className="h-10 w-full rounded border bg-input p-1" value={String(overrideRecord.emissive ?? "#000000").slice(0, 7)} onChange={(event) => override("emissive", event.target.value)} /></Row>
        <Check checked={Boolean(overrideRecord.double_sided)} onChange={(value) => override("double_sided", value)}>Double-sided</Check>
        <Button variant="outline" className="min-h-10 w-full" onClick={() => change((draft) => { delete draft.material_overrides; })}>Reset overrides</Button>
      </section>
      <section className="space-y-2" aria-label="Linked surface artwork">
        <div className="flex items-center justify-between"><h3 className={heading}>Linked artwork</h3><Button variant="outline" size="icon" className="size-8" aria-label="Add linked artwork" title="Add linked artwork" disabled={!artworkSources.length || (studio.doc?.version ?? 0) < 11} onClick={() => {
          const source = artworkSources[0];
          if (!source) return;
          let id = uid("graphic");
          while (graphics.some((graphic) => graphic.id === id)) id = uid("graphic");
          change((draft) => { draft.surface_graphics = [...((draft.surface_graphics as SurfaceGraphic[] | undefined) ?? []), { id, source: { kind: "node", page: source.page, node: source.node.id }, slot: "0", mode: "decal", projection: "uv", fit: "contain", wrap: "clamp", blend: "normal", opacity: 1, scale: [1, 1], offset: [0.5, 0.5] }]; });
        }}><Plus size={14} /></Button></div>
        {studio.doc && studio.doc.version < 11 && <div className="space-y-1 rounded border border-warning/30 bg-warning/5 p-2 text-[11px] text-muted-foreground"><p>Linked product artwork requires document format v11. Upgrade is undoable; save after checking your document.</p><Button variant="outline" className="min-h-9 w-full" disabled={studio.busy} onClick={() => void studio.upgradeDoc(11)}>Upgrade to v11</Button></div>}
        <p className="text-[11px] text-muted-foreground">Artwork stays linked to its page or node; previews and exports use the Rust renderer. Activate a source to link it to the first slot, or drag it onto a specific slot.</p>
        {!!studio.doc && <div className="space-y-1 rounded border bg-muted/30 p-1">
          <label className="sr-only" htmlFor="artwork-source-filter">Filter artwork sources</label>
          <input id="artwork-source-filter" type="search" autoComplete="off" className="min-h-12 w-full rounded border bg-input px-2 text-xs focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2" placeholder="Filter pages and nodes…" value={artworkSourceFilter} onChange={(event) => setArtworkSourceFilter(event.target.value)} />
          <div role="group" aria-label="Draggable artwork sources" className="flex max-h-24 flex-wrap gap-1 overflow-y-auto">
            {visibleDragSources.map(({ key, label, source }) => <button key={key} type="button" draggable disabled={studio.doc!.version < 11} aria-label={`Drag ${label} as artwork`} title="Drag onto a specific slot, or activate to link with the first slot" className="min-h-12 max-w-full truncate rounded border bg-card px-2 text-[11px] hover:bg-accent focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2 disabled:cursor-not-allowed disabled:opacity-40" onClick={() => activateArtworkSource(source)} onDragStart={(event) => { event.dataTransfer.effectAllowed = "copy"; event.dataTransfer.setData(ARTWORK_SOURCE_DRAG_TYPE, JSON.stringify(source)); }}>{label}</button>)}
            {visibleDragSources.length === 0 && <p role="status" className="px-2 py-1 text-[11px] text-muted-foreground">No page or drawable node matches. Clear the filter or choose from a slot source picker.</p>}
          </div>
          {dragSources.length > visibleDragSources.length && <p role="status" className="px-1 text-[10px] text-muted-foreground">Showing 80 of {dragSources.length} matches. Filter to narrow the list.</p>}
        </div>}
        {artworkDropNotice && <p role="status" className="text-xs text-muted-foreground">{artworkDropNotice}</p>}
        {!graphics.length ? <Empty>{artworkSources.length ? "No artwork is mapped to this model yet. Add a link to start a label or decal." : "Add a page or drawable node to this document before linking artwork."}</Empty> : graphics.map((graphic) => {
          const sourceKey = graphic.source.kind === "page" ? `page:${graphic.source.page}` : `node:${graphic.source.page}:${graphic.source.node}`;
          const selectedSource = artworkSources.find(({ page, node: item }) => `node:${page}:${item.id}` === sourceKey);
          const pageSource = studio.doc?.pages?.find((page) => `page:${page.id}` === sourceKey);
          const setSource = (value: string) => {
            if (value.startsWith("page:")) updateGraphic(graphic.id, { source: { kind: "page", page: value.slice(5) } });
            else { const found = artworkSources.find(({ page, node: item }) => `node:${page}:${item.id}` === value); if (found) updateGraphic(graphic.id, { source: { kind: "node", page: found.page, node: found.node.id } }); }
          };
          return <div
            key={graphic.id}
            data-artwork-drop-slot={graphic.slot}
            onDragOver={(event) => {
              if (!event.dataTransfer.types.includes(ARTWORK_SOURCE_DRAG_TYPE)) return;
              event.preventDefault();
              event.dataTransfer.dropEffect = "copy";
              setDragOverGraphic(graphic.id);
            }}
            onDragLeave={(event) => {
              const nextTarget = event.relatedTarget;
              if (!(nextTarget instanceof Node) || !event.currentTarget.contains(nextTarget)) setDragOverGraphic(null);
            }}
            onDrop={(event) => {
              event.preventDefault();
              setDragOverGraphic(null);
              const source = parseArtworkSourceDrag(event.dataTransfer.getData(ARTWORK_SOURCE_DRAG_TYPE));
              if (!source) {
                setArtworkDropNotice("That drag did not contain a valid artwork source; use the source picker instead.");
                return;
              }
              const pageExists = studio.doc?.pages.some((page) => page.id === source.page);
              const nodeExists = source.kind === "page" || artworkSources.some(({ page, node: item }) => page === source.page && item.id === source.node);
              if (!pageExists || !nodeExists) {
                setArtworkDropNotice("The dragged page or node is no longer in this document. Choose an available source from the picker.");
                return;
              }
              updateGraphic(graphic.id, { source });
              setArtworkDropNotice(`Linked ${source.kind === "page" ? `page ${source.page}` : `node ${source.node}`} to slot ${graphic.slot}.`);
            }}
            className={`space-y-1.5 rounded border bg-card p-2 ${dragOverGraphic === graphic.id ? "border-primary bg-primary/5 ring-2 ring-ring" : ""}`}
          >
            {dragOverGraphic === graphic.id && <p role="status" className="rounded bg-primary/10 px-2 py-1 text-[11px] text-primary">Release to link artwork to slot {graphic.slot}.</p>}
            <div className="flex items-center justify-between"><span className="truncate text-[11px] font-medium">{graphic.id}</span><Button variant="ghost" size="icon" className="size-8" aria-label={`Remove ${graphic.id}`} title="Remove link" onClick={() => change((draft) => { const next = ((draft.surface_graphics as SurfaceGraphic[] | undefined) ?? []).filter((item) => item.id !== graphic.id); if (next.length) draft.surface_graphics = next; else delete draft.surface_graphics; })}><Trash2 size={13} /></Button></div>
            <label className="block space-y-1 text-[11px] text-muted-foreground">Source<select aria-label={`${graphic.id} source`} className="h-8 w-full rounded border bg-input px-2 text-xs text-foreground" value={selectedSource ? sourceKey : pageSource ? sourceKey : ""} onChange={(event) => setSource(event.target.value)}><option value="" disabled>Choose page or node…</option>{(studio.doc?.pages ?? []).map((page) => <option key={`page:${page.id}`} value={`page:${page.id}`}>Page · {page.name || page.id}</option>)}{artworkSources.map(({ page, node: item }) => <option key={`node:${page}:${item.id}`} value={`node:${page}:${item.id}`}>Node · {item.name || item.id}</option>)}</select></label>
            {!selectedSource && !pageSource && <p role="status" className="text-[11px] text-warning">Linked source is missing. Choose another source to restore the mapping.</p>}
            <Row label="Material slot"><input aria-label={`${graphic.id} material slot`} className="h-8 min-w-0 flex-1 rounded border bg-input px-2 text-xs" value={graphic.slot} onChange={(event) => updateGraphic(graphic.id, { slot: event.target.value })} /></Row>
            <Row label="UV island"><input aria-label={`${graphic.id} UV island ID`} autoComplete="off" spellCheck={false} className="h-8 min-w-0 flex-1 rounded border bg-input px-2 text-xs focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring" value={graphic.island ?? ""} placeholder="All slot faces" onChange={(event) => updateGraphic(graphic.id, { island: event.target.value.trim() || undefined })} /></Row>
            <Row label="Mode"><select aria-label={`${graphic.id} placement mode`} className="h-8 min-w-0 flex-1 rounded border bg-input px-2 text-xs" value={graphic.mode} onChange={(event) => { const mode = event.target.value as SurfaceGraphic["mode"]; updateGraphic(graphic.id, { mode, scale: mode === "fill" ? [Math.max(1, graphic.scale?.[0] ?? 1), Math.max(1, graphic.scale?.[1] ?? 1)] : [Math.min(1, graphic.scale?.[0] ?? 1), Math.min(1, graphic.scale?.[1] ?? 1)], offset: mode === "fill" ? [0, 0] : [0.5, 0.5], wrap: mode === "decal" ? "clamp" : graphic.wrap }); }}><option value="decal">Decal</option><option value="fill">Region fill</option></select></Row>
            <Row label="Projection"><select aria-label={`${graphic.id} projection`} className="h-10 min-w-0 flex-1 rounded border bg-input px-2 text-xs focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring" value={graphic.projection} onChange={(event) => { const projection = event.target.value as SurfaceGraphic["projection"]; updateGraphic(graphic.id, { projection, axis: ["planar", "cylindrical", "spherical"].includes(projection) ? graphic.axis ?? "z" : undefined }); }}><option value="uv">Existing UVs</option><option value="planar">Planar</option><option value="box">Box</option><option value="cylindrical">Cylindrical</option><option value="spherical">Spherical</option><option value="triplanar">Triplanar</option></select></Row>
            {graphic.projection !== "uv" && graphic.projection !== "triplanar" && <Row label="Axis"><select aria-label={`${graphic.id} projection axis`} className="h-10 min-w-0 flex-1 rounded border bg-input px-2 text-xs focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring" value={graphic.axis ?? "z"} onChange={(event) => updateGraphic(graphic.id, { axis: event.target.value as SurfaceGraphic["axis"] })}><option value="x">X</option><option value="y">Y</option><option value="z">Z</option></select></Row>}
            <Row label="Fit"><select aria-label={`${graphic.id} artwork fit`} className="h-8 min-w-0 flex-1 rounded border bg-input px-2 text-xs" value={graphic.fit} onChange={(event) => updateGraphic(graphic.id, { fit: event.target.value as SurfaceGraphic["fit"] })}><option value="contain">Contain</option><option value="cover">Cover</option><option value="stretch">Stretch</option></select></Row>
            {graphic.mode === "fill" && <Row label="Tile"><select aria-label={`${graphic.id} fill repeat mode`} className="h-8 min-w-0 flex-1 rounded border bg-input px-2 text-xs" value={graphic.wrap} onChange={(event) => updateGraphic(graphic.id, { wrap: event.target.value as SurfaceGraphic["wrap"] })}><option value="clamp">Single</option><option value="repeat">Repeat</option><option value="mirror">Mirror</option></select></Row>}
            <Row label="Opacity"><NumberField value={graphic.opacity ?? 1} min={0} max={1} step={0.05} onCommit={(value) => updateGraphic(graphic.id, { opacity: value })} ariaLabel={`${graphic.id} opacity`} /></Row>
            <fieldset className="space-y-1 rounded border border-border/60 p-2"><legend className="px-1 text-[10px] text-muted-foreground">Overlay material response</legend>
              <Row label="Metallic"><NumberField value={graphic.material?.metallic ?? 0} min={0} max={1} step={0.05} onCommit={(value) => updateGraphic(graphic.id, { material: { ...graphic.material, metallic: value } })} ariaLabel={`${graphic.id} metallic response`} /></Row>
              <Row label="Roughness"><NumberField value={graphic.material?.roughness ?? 1} min={0} max={1} step={0.05} onCommit={(value) => updateGraphic(graphic.id, { material: { ...graphic.material, roughness: value } })} ariaLabel={`${graphic.id} roughness response`} /></Row>
              <Row label="Mat. opacity"><NumberField value={graphic.material?.opacity ?? 1} min={0} max={1} step={0.05} onCommit={(value) => updateGraphic(graphic.id, { material: { ...graphic.material, opacity: value } })} ariaLabel={`${graphic.id} material opacity`} /></Row>
            </fieldset>
            <Row label="Blend"><select aria-label={`${graphic.id} blend mode`} className="h-8 min-w-0 flex-1 rounded border bg-input px-2 text-xs" value={graphic.blend ?? "normal"} onChange={(event) => updateGraphic(graphic.id, { blend: event.target.value })}>{["normal", "multiply", "screen", "overlay", "darken", "lighten"].map((mode) => <option key={mode} value={mode}>{mode}</option>)}</select></Row>
            <Row label="Scale X"><NumberField value={graphic.scale?.[0] ?? 1} min={graphic.mode === "fill" ? 1 : 0.01} max={graphic.mode === "fill" ? 16 : 1} step={0.05} onCommit={(value) => updateGraphic(graphic.id, { scale: [value, graphic.scale?.[1] ?? 1], offset: graphic.mode === "decal" ? [Math.max(value / 2, Math.min(1 - value / 2, graphic.offset?.[0] ?? 0.5)), graphic.offset?.[1] ?? 0.5] : graphic.offset })} ariaLabel={`${graphic.id} horizontal scale`} /></Row>
            <Row label="Scale Y"><NumberField value={graphic.scale?.[1] ?? 1} min={graphic.mode === "fill" ? 1 : 0.01} max={graphic.mode === "fill" ? 16 : 1} step={0.05} onCommit={(value) => updateGraphic(graphic.id, { scale: [graphic.scale?.[0] ?? 1, value], offset: graphic.mode === "decal" ? [graphic.offset?.[0] ?? 0.5, Math.max(value / 2, Math.min(1 - value / 2, graphic.offset?.[1] ?? 0.5))] : graphic.offset })} ariaLabel={`${graphic.id} vertical scale`} /></Row>
            <Row label="Offset U"><NumberField value={graphic.offset?.[0] ?? (graphic.mode === "decal" ? 0.5 : 0)} min={0} max={1} step={0.01} onCommit={(value) => updateGraphic(graphic.id, { offset: graphic.mode === "decal" ? [Math.max((graphic.scale?.[0] ?? 1) / 2, Math.min(1 - (graphic.scale?.[0] ?? 1) / 2, value)), graphic.offset?.[1] ?? 0.5] : [value, graphic.offset?.[1] ?? 0] })} ariaLabel={`${graphic.id} horizontal offset`} /></Row>
            <Row label="Offset V"><NumberField value={graphic.offset?.[1] ?? (graphic.mode === "decal" ? 0.5 : 0)} min={0} max={1} step={0.01} onCommit={(value) => updateGraphic(graphic.id, { offset: graphic.mode === "decal" ? [graphic.offset?.[0] ?? 0.5, Math.max((graphic.scale?.[1] ?? 1) / 2, Math.min(1 - (graphic.scale?.[1] ?? 1) / 2, value))] : [graphic.offset?.[0] ?? 0, value] })} ariaLabel={`${graphic.id} vertical offset`} /></Row>
            <Row label="Rotation"><NumberField value={graphic.rotate ?? 0} min={-360} max={360} step={1} onCommit={(value) => updateGraphic(graphic.id, { rotate: value })} ariaLabel={`${graphic.id} rotation degrees`} /></Row>
            <fieldset className="space-y-1"><legend className="text-[11px] text-muted-foreground">Source crop · normalized</legend><div className="grid grid-cols-2 gap-x-2 gap-y-1">{(["X", "Y", "Width", "Height"] as const).map((label, index) => { const crop = graphic.crop ?? [0, 0, 1, 1]; const max = index === 0 ? 1 - crop[2] : index === 1 ? 1 - crop[3] : 1 - crop[index - 2]; return <label key={label} className="flex items-center gap-1 text-[10px] text-muted-foreground">{label}<NumberField value={crop[index]} min={index < 2 ? 0 : 0.01} max={max} step={0.01} onCommit={(value) => { const next = [...(graphic.crop ?? [0, 0, 1, 1])]; next[index] = value; updateGraphic(graphic.id, { crop: next }); }} ariaLabel={`${graphic.id} crop ${label.toLowerCase()}`} /></label>; })}</div></fieldset>
          </div>;
        })}
        {artworkSources.length === 0 && graphics.length > 0 && <p role="status" className="text-[11px] text-warning">The linked source is missing. Add a source node or page, then reselect it.</p>}
      </section>
    </>}
  </div>;
}
