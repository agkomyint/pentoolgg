import { useEffect, useMemo, useRef, useState, type DragEvent as ReactDragEvent, type FormEvent, type MouseEvent, type PointerEvent as ReactPointerEvent } from "react";
import { Download, Move, RefreshCw } from "lucide-react";
import { api, type Doc, type Scene3dUvIsland, type SceneNode } from "@/lib/api";
import { children, findNode, layerNodes, pageOf } from "@/lib/model";
import { useStudio } from "@/store";
import { Button } from "@/components/ui/button";
import { Empty, NumberField, Row } from "@/components/ui/kit";
import { Canvas } from "@/components/Canvas";

type Preview = Awaited<ReturnType<typeof api.scene3dUvPreview>>;
type ModelPreview = Awaited<ReturnType<typeof api.scene3dUvModelPreview>>;
type UvTransform = { scale: number[]; offset: number[]; rotate: number };
type StoredIsland = { id: string; uv_set: number; triangles: number[]; transform: UvTransform };
type Scene3dUvIslandRecord = Scene3dUvIsland & StoredIsland;
type Overlay = { projection?: string; axis?: string; islands?: StoredIsland[] };
type SurfaceGraphic = { id: string; slot: string; mode: "decal" | "fill"; projection: string; source?: { kind: "page" | "node"; page: string; node?: string }; fit?: string; wrap?: string; crop?: [number, number, number, number]; opacity?: number; scale?: [number, number]; offset?: [number, number]; rotate?: number };
type GraphicTransform = { scale: [number, number]; offset: [number, number]; rotate: number };
type GraphicTransformGesture = {
  mode: "resize" | "rotate";
  pointerId: number;
  captureTarget: HTMLButtonElement;
  start: [number, number];
  center: [number, number];
  startAngle: number;
  transform: GraphicTransform;
  draft: GraphicTransform;
  moved: boolean;
};
type IslandTransformGesture = {
  mode: "move" | "scale" | "rotate";
  pointerId: number;
  captureTarget: HTMLButtonElement;
  start: [number, number];
  center: [number, number];
  startAngle: number;
  startBounds: [number, number, number, number];
  transform: UvTransform;
  draft: UvTransform;
  moved: boolean;
};
type ProjectedDoc = Doc & { models?: Record<string, { uv?: Record<string, Overlay>; [key: string]: unknown }>; product_forms?: Record<string, { uv?: Record<string, Overlay>; [key: string]: unknown }> };

function modelId(node: SceneNode | null): string | null {
  return typeof node?.model === "string" ? node.model : null;
}

export function UvWorkspaceStage({ active = true }: { active?: boolean } = {}) {
  const studio = useStudio();
  const [primitive, setPrimitive] = useState(0);
  const [preview, setPreview] = useState<Preview | null>(null);
  const [modelPreview, setModelPreview] = useState<ModelPreview | null>(null);
  const [modelPreviewLoading, setModelPreviewLoading] = useState(false);
  const [modelPreviewError, setModelPreviewError] = useState("");
  const [showPageContext, setShowPageContext] = useState(false);
  const [uvTarget, setUvTarget] = useState<{ pageId: string; nodeId: string } | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState("");
  const [islandId, setIslandId] = useState("");
  const [newId, setNewId] = useState("label-island");
  const [triangleText, setTriangleText] = useState("");
  const [formError, setFormError] = useState("");
  const [projection, setProjection] = useState("box");
  const [textureResolution, setTextureResolution] = useState(2048);
  const [minimumDensity, setMinimumDensity] = useState("");
  const [packPadding, setPackPadding] = useState("0.01");
  const [packing, setPacking] = useState(false);
  const [packError, setPackError] = useState("");
  const [packMessage, setPackMessage] = useState("");
  const [refreshNonce, setRefreshNonce] = useState(0);
  const [graphicId, setGraphicId] = useState("");
  const [graphicSourceKind, setGraphicSourceKind] = useState<"page" | "node">("page");
  const [graphicSourcePage, setGraphicSourcePage] = useState("");
  const [graphicSourceNode, setGraphicSourceNode] = useState("");
  const [graphicDraftId, setGraphicDraftId] = useState("surface-art");
  const [graphicMode, setGraphicMode] = useState<"decal" | "fill">("decal");
  const [placingOnFace, setPlacingOnFace] = useState(false);
  const [graphicFit, setGraphicFit] = useState("contain");
  const [graphicWrap, setGraphicWrap] = useState("clamp");
  const [graphicCrop, setGraphicCrop] = useState<[number, number, number, number]>([0, 0, 1, 1]);
  const [dragGraphicOffset, setDragGraphicOffset] = useState<[number, number] | null>(null);
  const [graphicTransformDraft, setGraphicTransformDraft] = useState<GraphicTransform | null>(null);
  const [islandTransformDraft, setIslandTransformDraft] = useState<UvTransform | null>(null);
  const [dragOverUv, setDragOverUv] = useState(false);
  const [facePicking, setFacePicking] = useState(false);
  const canvasRef = useRef<HTMLDivElement>(null);
  const graphicDragMoved = useRef(false);
  const graphicDragStart = useRef<[number, number] | null>(null);
  const modelGraphicDrag = useRef<{ pointerId: number; start: [number, number]; clientStart: [number, number]; hit: Promise<Awaited<ReturnType<typeof api.scene3dUvFacePick>>>; moved: boolean } | null>(null);
  const graphicTransformGesture = useRef<GraphicTransformGesture | null>(null);
  const islandTransformGesture = useRef<IslandTransformGesture | null>(null);
  const suppressNextModelClick = useRef(false);
  const doc = studio.doc as ProjectedDoc | null;
  const selected = useMemo(() => doc ? findNode(pageOf(doc, studio.pageId), studio.selectedId)?.node ?? null : null, [doc, studio.pageId, studio.selectedId]);
  useEffect(() => {
    if (!showPageContext && selected?.kind === "scene3d" && studio.pageId) {
      setUvTarget({ pageId: studio.pageId, nodeId: selected.id });
    }
  }, [selected?.id, selected?.kind, showPageContext, studio.pageId]);
  const targetPageId = uvTarget?.pageId ?? studio.pageId;
  const node = useMemo(() => {
    const candidate = doc && uvTarget ? findNode(pageOf(doc, uvTarget.pageId), uvTarget.nodeId)?.node : selected;
    return candidate?.kind === "scene3d" ? candidate : null;
  }, [doc, selected, uvTarget]);
  const nodeLocked = !!(doc && node && findNode(pageOf(doc, targetPageId), node.id)?.layer.locked);
  const uvGraphics = (Array.isArray(node?.surface_graphics) ? node.surface_graphics as SurfaceGraphic[] : [])
    .filter((graphic) => graphic.projection === "uv");
  const uvGraphicIds = uvGraphics.map((graphic) => graphic.id).join("\0");
  const selectedGraphic = uvGraphics.find((graphic) => graphic.id === graphicId) ?? null;
  const selectedGraphicScale = graphicTransformDraft?.scale ?? selectedGraphic?.scale ?? [1, 1];
  const selectedGraphicOffset = dragGraphicOffset ?? graphicTransformDraft?.offset ?? selectedGraphic?.offset ?? (selectedGraphic?.mode === "fill" ? [0, 0] : [0.5, 0.5]);
  const selectedGraphicRotation = graphicTransformDraft?.rotate ?? selectedGraphic?.rotate ?? 0;
  const selectedGraphicHandlePositions = selectedGraphic ? (() => {
    const radians = selectedGraphicRotation * Math.PI / 180;
    const rotatePoint = (x: number, y: number): [number, number] => [
      selectedGraphicOffset[0] + x * Math.cos(radians) - y * Math.sin(radians),
      selectedGraphicOffset[1] + x * Math.sin(radians) + y * Math.cos(radians),
    ];
    return {
      resize: rotatePoint(selectedGraphicScale[0] / 2, selectedGraphicScale[1] / 2),
      rotate: rotatePoint(0, -selectedGraphicScale[1] / 2 - 0.04),
    };
  })() : null;
  const graphicSourceCandidates = useMemo(() => {
    const page = doc?.pages?.find((candidate) => candidate.id === graphicSourcePage);
    const out: SceneNode[] = [];
    const visit = (items: SceneNode[]) => items.forEach((item) => { out.push(item); visit(children(item)); });
    page?.layers?.filter((layer) => !layer.locked && layer.visible !== false).forEach((layer) => visit(layerNodes(layer)));
    return out.filter((candidate) => candidate.id !== node?.id && candidate.visible !== false);
  }, [doc, graphicSourcePage, node?.id]);
  const digest = modelId(node) ?? (typeof node?.product_form === "string" ? node.product_form : null);
  const sourceKind = modelId(node) ? "model" : "product_form";
  const record = digest ? (sourceKind === "model" ? doc?.models?.[digest] : doc?.product_forms?.[digest]) : undefined;
  const uvMap = record?.uv ?? {};
  const selectedOverlay = uvMap[String(primitive)] ?? uvMap["*"] ?? {};
  const entries = preview?.report.primitives ?? [];
  const current = entries.find((entry) => entry.primitive === primitive) ?? null;
  const storedIslands = selectedOverlay.islands ?? [];
  const islands: Scene3dUvIslandRecord[] = (current?.islands ?? []).map((entry) => ({
    ...entry,
    triangles: storedIslands.find((item) => item.id === entry.id)?.triangles ?? [],
    transform: storedIslands.find((item) => item.id === entry.id)?.transform ?? { scale: [1, 1], offset: [0, 0], rotate: 0 },
  }));
  const selectedIsland = islands.find((island) => island.id === islandId) ?? null;
  const displayedIslandBounds = selectedIsland ? (() => {
    const { min, max } = selectedIsland.uv_bounds;
    const transform = islandTransformDraft ?? selectedIsland.transform;
    const start = selectedIsland.transform;
    const deltaU = transform.offset[0] - start.offset[0];
    const deltaV = transform.offset[1] - start.offset[1];
    const ratioU = Math.abs(start.scale[0]) > 1e-6 ? Math.abs(transform.scale[0] / start.scale[0]) : 1;
    const ratioV = Math.abs(start.scale[1]) > 1e-6 ? Math.abs(transform.scale[1] / start.scale[1]) : 1;
    const width = (max[0] - min[0]) * ratioU;
    const height = (max[1] - min[1]) * ratioV;
    return { x: (min[0] + max[0]) / 2 + deltaU - width / 2, y: (min[1] + max[1]) / 2 + deltaV - height / 2, width, height, rotate: transform.rotate - start.rotate };
  })() : null;
  const islandHandlePositions = displayedIslandBounds ? (() => {
    const bounds = displayedIslandBounds;
    if (bounds.width >= 0.14 && bounds.height >= 0.14) {
      return {
        scale: [bounds.x + bounds.width, bounds.y + bounds.height] as [number, number],
        rotate: [bounds.x + bounds.width / 2, Math.max(0.08, bounds.y - 0.06)] as [number, number],
      };
    }
    const right = bounds.x + bounds.width / 2 < 0.5;
    const below = bounds.y + bounds.height / 2 < 0.5;
    const clamp = (value: number) => Math.max(0.08, Math.min(0.92, value));
    const x = clamp(right ? bounds.x + bounds.width + 0.1 : bounds.x - 0.1);
    const scaleY = clamp(below ? bounds.y + bounds.height + 0.1 : bounds.y - 0.1);
    return {
      scale: [x, scaleY] as [number, number],
      rotate: [x, clamp(scaleY + (below ? 0.11 : -0.11))] as [number, number],
    };
  })() : null;
  const parsedMinimumDensity = minimumDensity.trim() ? Number(minimumDensity) : undefined;
  const minimumDensityValue = parsedMinimumDensity != null && Number.isFinite(parsedMinimumDensity) && parsedMinimumDensity > 0
    ? parsedMinimumDensity
    : undefined;
  const discontinuityCount = current?.uv_discontinuities.reduce((total, set) => total + set.count, 0) ?? 0;
  const seamAnalysisTruncated = current?.uv_discontinuities.some((set) => set.analysis_truncated || set.details_truncated || set.render_segments_truncated) ?? false;

  useEffect(() => {
    setPrimitive(0);
    setIslandId("");
    setGraphicId("");
    setDragGraphicOffset(null);
    setGraphicTransformDraft(null);
    graphicTransformGesture.current = null;
    setPreview(null);
    setModelPreview(null);
    setModelPreviewError("");
    setPlacingOnFace(false);
  }, [node?.id]);

  useEffect(() => {
    const fallbackPage = studio.pageId ?? doc?.pages?.[0]?.id ?? "";
    if (!doc?.pages?.some((page) => page.id === graphicSourcePage)) setGraphicSourcePage(fallbackPage);
  }, [doc, graphicSourcePage, studio.pageId]);

  useEffect(() => {
    if (!graphicSourceCandidates.some((candidate) => candidate.id === graphicSourceNode)) {
      setGraphicSourceNode(graphicSourceCandidates[0]?.id ?? "");
    }
  }, [graphicSourceCandidates, graphicSourceNode]);

  useEffect(() => {
    setGraphicId((currentId) => uvGraphics.some((graphic) => graphic.id === currentId)
      ? currentId
      : uvGraphics[0]?.id ?? "");
    setDragGraphicOffset(null);
    setGraphicTransformDraft(null);
    graphicTransformGesture.current = null;
  }, [node?.id, primitive, uvGraphicIds]);

  useEffect(() => {
    const cancelGesture = (event: KeyboardEvent) => {
      if (!active) return;
      if (event.key !== "Escape") return;
      const islandGesture = islandTransformGesture.current;
      if (islandGesture) {
        event.preventDefault();
        if (islandGesture.captureTarget.hasPointerCapture(islandGesture.pointerId)) {
          islandGesture.captureTarget.releasePointerCapture(islandGesture.pointerId);
        }
        islandTransformGesture.current = null;
        setIslandTransformDraft(null);
        setPackMessage("Island transform cancelled.");
        return;
      }
      const gesture = graphicTransformGesture.current;
      if (!gesture) return;
      event.preventDefault();
      if (gesture.captureTarget.hasPointerCapture(gesture.pointerId)) {
        gesture.captureTarget.releasePointerCapture(gesture.pointerId);
      }
      graphicTransformGesture.current = null;
      setGraphicTransformDraft(null);
      setPackMessage("Transform cancelled.");
    };
    window.addEventListener("keydown", cancelGesture);
    return () => window.removeEventListener("keydown", cancelGesture);
  }, [active]);

  useEffect(() => setProjection(selectedOverlay.projection ?? "box"), [primitive, node?.id, selectedOverlay.projection]);

  useEffect(() => {
    if (!active) return;
    if (!doc || !node || !digest) {
      setPreview(null);
      setError("");
      return;
    }
    let current = true;
    const timer = window.setTimeout(() => {
      setLoading(true);
      setError("");
      setPreview(null);
      void api.scene3dUvPreview(doc, node.id, primitive, 1024, textureResolution, minimumDensityValue)
        .then((result) => { if (current) setPreview(result); })
        .catch((reason) => { if (current) setError(reason instanceof Error ? reason.message : "Could not inspect this model's UVs."); })
        .finally(() => { if (current) setLoading(false); });
    }, 180);
    return () => { current = false; window.clearTimeout(timer); };
  }, [doc, node, digest, primitive, refreshNonce, textureResolution, minimumDensity, active]);

  useEffect(() => {
    if (!active) return;
    if (!doc || !node || !targetPageId || !digest) {
      setModelPreview(null);
      setModelPreviewError("");
      return;
    }
    let current = true;
    const timer = window.setTimeout(() => {
      setModelPreviewLoading(true);
      setModelPreview(null);
      setModelPreviewError("");
      void api.scene3dUvModelPreview({
        document: doc,
        page: targetPageId,
        node: node.id,
        primitive,
        island: selectedIsland?.id,
      })
        .then((result) => { if (current) setModelPreview(result); })
        .catch((reason) => {
          if (current) {
            setModelPreview(null);
            setModelPreviewError(reason instanceof Error ? reason.message : "Could not render the selected 3D model.");
          }
        })
        .finally(() => { if (current) setModelPreviewLoading(false); });
    }, 120);
    return () => { current = false; window.clearTimeout(timer); };
  }, [doc, node, digest, targetPageId, primitive, selectedIsland?.id, refreshNonce, active]);

  useEffect(() => setIslandId(""), [primitive, node?.id]);

  useEffect(() => {
    setIslandTransformDraft(null);
    islandTransformGesture.current = null;
  }, [islandId, primitive, node?.id]);

  const editOverlay = (mutate: (overlay: Overlay) => void, label: string) => {
    if (!digest) return;
    studio.edit(label, (draft) => {
      const document = draft as ProjectedDoc;
      const source = sourceKind === "model"
        ? ((document.models ??= {})[digest] ??= {})
        : ((document.product_forms ??= {})[digest] ??= {});
      const overlays = (source.uv ??= {});
      const key = String(primitive);
      const overlay = structuredClone(overlays[key] ?? {});
      mutate(overlay);
      overlays[key] = overlay;
    });
  };

  const editIsland = (mutate: (record: StoredIsland) => void, label: string) => {
    if (!selectedIsland) return;
    editOverlay((overlay) => {
      const records = overlay.islands ?? [];
      const selectedRecord = records.find((item) => item.id === selectedIsland.id);
      if (selectedRecord) mutate(selectedRecord);
    }, label);
  };

  const beginIslandTransform = (mode: IslandTransformGesture["mode"], event: ReactPointerEvent<HTMLButtonElement>) => {
    if (!selectedIsland || !displayedIslandBounds || nodeLocked || !canvasRef.current) return;
    event.preventDefault();
    event.stopPropagation();
    event.currentTarget.setPointerCapture(event.pointerId);
    const rect = canvasRef.current.getBoundingClientRect();
    const { x, y, width, height } = displayedIslandBounds;
    const center: [number, number] = [rect.left + (x + width / 2) * rect.width, rect.top + (y + height / 2) * rect.height];
    const transform = structuredClone(islandTransformDraft ?? selectedIsland.transform);
    islandTransformGesture.current = {
      mode,
      pointerId: event.pointerId,
      captureTarget: event.currentTarget,
      start: [event.clientX, event.clientY],
      center,
      startAngle: Math.atan2(event.clientY - center[1], event.clientX - center[0]),
      startBounds: [x, y, width, height],
      transform,
      draft: structuredClone(transform),
      moved: false,
    };
  };

  const moveIslandTransform = (event: ReactPointerEvent<HTMLButtonElement>) => {
    const gesture = islandTransformGesture.current;
    if (!gesture || gesture.pointerId !== event.pointerId || !canvasRef.current) return;
    const rect = canvasRef.current.getBoundingClientRect();
    const dx = (event.clientX - gesture.start[0]) / Math.max(1, rect.width);
    const dy = (event.clientY - gesture.start[1]) / Math.max(1, rect.height);
    if (dx * dx + dy * dy > 0.000004) gesture.moved = true;
    const draft = structuredClone(gesture.transform);
    if (gesture.mode === "move") {
      draft.offset[0] = Math.max(-4, Math.min(4, draft.offset[0] + dx));
      draft.offset[1] = Math.max(-4, Math.min(4, draft.offset[1] + dy));
    } else if (gesture.mode === "scale") {
      const [,, width, height] = gesture.startBounds;
      const ratioU = Math.max(0.01, Math.min(16, 1 + dx * 2 / Math.max(width, 0.001)));
      const ratioV = Math.max(0.01, Math.min(16, 1 + dy * 2 / Math.max(height, 0.001)));
      draft.scale[0] = Math.sign(gesture.transform.scale[0] || 1) * Math.min(16, Math.max(0.01, Math.abs(gesture.transform.scale[0]) * ratioU));
      draft.scale[1] = Math.sign(gesture.transform.scale[1] || 1) * Math.min(16, Math.max(0.01, Math.abs(gesture.transform.scale[1]) * ratioV));
    } else {
      const angle = Math.atan2(event.clientY - gesture.center[1], event.clientX - gesture.center[0]);
      const delta = angle - gesture.startAngle;
      draft.rotate = Math.max(-360, Math.min(360, gesture.transform.rotate + delta * 180 / Math.PI));
    }
    gesture.draft = draft;
    setIslandTransformDraft(draft);
  };

  const endIslandTransform = (event: ReactPointerEvent<HTMLButtonElement>, commit: boolean) => {
    const gesture = islandTransformGesture.current;
    if (!gesture || gesture.pointerId !== event.pointerId) return;
    if (event.currentTarget.hasPointerCapture(event.pointerId)) event.currentTarget.releasePointerCapture(event.pointerId);
    islandTransformGesture.current = null;
    setIslandTransformDraft(null);
    if (commit && gesture.moved) {
      const transform = gesture.draft;
      editIsland((record) => { record.transform = transform; }, gesture.mode === "move" ? "Move UV island" : gesture.mode === "scale" ? "Scale UV island" : "Rotate UV island");
    }
  };

  const keyboardIslandTransform = (mode: IslandTransformGesture["mode"], event: React.KeyboardEvent<HTMLButtonElement>) => {
    if (!selectedIsland || nodeLocked) return;
    const step = event.shiftKey ? 0.05 : 0.01;
    if (mode === "move") {
      const offset = [...selectedIsland.transform.offset];
      if (event.key === "ArrowLeft") offset[0] -= step;
      else if (event.key === "ArrowRight") offset[0] += step;
      else if (event.key === "ArrowUp") offset[1] -= step;
      else if (event.key === "ArrowDown") offset[1] += step;
      else return;
      event.preventDefault();
      editIsland((record) => { record.transform.offset = offset.map((value) => Math.max(-4, Math.min(4, value))); }, "Move UV island");
    } else if (mode === "scale") {
      const scale = [...selectedIsland.transform.scale];
      if (event.key === "ArrowLeft") scale[0] *= 1 - step;
      else if (event.key === "ArrowRight") scale[0] *= 1 + step;
      else if (event.key === "ArrowUp") scale[1] *= 1 - step;
      else if (event.key === "ArrowDown") scale[1] *= 1 + step;
      else return;
      event.preventDefault();
      editIsland((record) => { record.transform.scale = scale.map((value) => Math.sign(value || 1) * Math.max(0.01, Math.min(16, Math.abs(value)))); }, "Scale UV island");
    } else {
      if (event.key !== "ArrowLeft" && event.key !== "ArrowRight") return;
      event.preventDefault();
      const delta = (event.key === "ArrowRight" ? 1 : -1) * (event.shiftKey ? 15 : 1);
      editIsland((record) => { record.transform.rotate = Math.max(-360, Math.min(360, record.transform.rotate + delta)); }, "Rotate UV island");
    }
  };

  const updateGraphic = (id: string, patch: Partial<SurfaceGraphic>) => {
    if (!node || !targetPageId || nodeLocked) return;
    studio.edit("Edit linked UV artwork", (document) => {
      const found = findNode(pageOf(document, targetPageId), node.id);
      if (!found || found.layer.locked) return;
      const draft = found.node;
      const graphics = [...((draft.surface_graphics as SurfaceGraphic[] | undefined) ?? [])];
      const index = graphics.findIndex((graphic) => graphic.id === id);
      if (index < 0) return;
      graphics[index] = { ...graphics[index], ...patch };
      draft.surface_graphics = graphics;
    });
    setDragGraphicOffset(null);
    setGraphicTransformDraft(null);
  };

  const adjustSelectedDecalScale = (delta: number) => {
    if (!selectedGraphic || selectedGraphic.mode !== "decal") return;
    const scale: [number, number] = [
      Math.max(0.01, Math.min(1, selectedGraphicScale[0] + delta)),
      Math.max(0.01, Math.min(1, selectedGraphicScale[1] + delta)),
    ];
    updateGraphic(selectedGraphic.id, {
      scale,
      offset: [
        Math.max(scale[0] / 2, Math.min(1 - scale[0] / 2, selectedGraphicOffset[0])),
        Math.max(scale[1] / 2, Math.min(1 - scale[1] / 2, selectedGraphicOffset[1])),
      ],
    });
  };

  const makeLinkedGraphic = (id: string, slot: string, mode: "decal" | "fill", offset: [number, number], sourceOverride?: SurfaceGraphic["source"]): SurfaceGraphic | null => {
    if (!node || nodeLocked || (!graphicSourcePage && !sourceOverride)) return null;
    if (!id) { setPackMessage("Enter a stable ID for this surface graphic."); return null; }
    if ((node?.surface_graphics as SurfaceGraphic[] | undefined)?.some((graphic) => graphic.id === id)) { setPackMessage(`Graphic ID “${id}” is already used on this model.`); return null; }
    if (graphicCrop[0] + graphicCrop[2] > 1 || graphicCrop[1] + graphicCrop[3] > 1) { setPackMessage("Source crop must stay inside 0–1 on both axes."); return null; }
    if (graphicSourceKind === "node" && !graphicSourceCandidates.some((candidate) => candidate.id === graphicSourceNode)) {
      setPackMessage("Choose a visible source node on the selected page."); return null;
    }
    const source: NonNullable<SurfaceGraphic["source"]> = sourceOverride ?? (graphicSourceKind === "page"
      ? { kind: "page", page: graphicSourcePage }
      : { kind: "node", page: graphicSourcePage, node: graphicSourceNode });
    const graphic: SurfaceGraphic = {
      id,
      source,
      slot,
      mode,
      projection: "uv",
      fit: graphicFit,
      wrap: graphicWrap,
      ...(graphicCrop[0] !== 0 || graphicCrop[1] !== 0 || graphicCrop[2] !== 1 || graphicCrop[3] !== 1 ? { crop: graphicCrop } : {}),
      opacity: 1,
      scale: mode === "fill" ? [2, 2] : [0.25, 0.25],
      offset: mode === "fill" ? [0, 0] : offset,
      rotate: 0,
    };
    return graphic;
  };

  const commitLinkedGraphic = (graphic: SurfaceGraphic) => {
    if (!node || !targetPageId || nodeLocked) return;
    studio.edit("Link artwork to UV surface", (document) => {
      const found = findNode(pageOf(document, targetPageId), node.id);
      if (!found || found.layer.locked) return;
      const draft = found.node;
      draft.surface_graphics = [...((draft.surface_graphics as SurfaceGraphic[] | undefined) ?? []), graphic];
    });
    setGraphicId(graphic.id);
    const sourceDescription = graphic.source?.kind === "page" ? `page ${graphic.source.page}` : `node ${graphic.source?.node}`;
    setPackMessage(`Added linked ${graphic.mode} “${graphic.id}” from ${sourceDescription} on slot ${graphic.slot}.`);
  };

  const addLinkedGraphic = (event: FormEvent) => {
    event.preventDefault();
    if (!preview?.material_slot) return;
    const id = graphicDraftId.trim();
    const graphic = makeLinkedGraphic(id, preview.material_slot, graphicMode, [0.5, 0.5]);
    if (graphic) commitLinkedGraphic(graphic);
  };

  const pickModelAt = async (x: number, y: number) => {
    if (!doc || !node || !targetPageId || facePicking) return;
    setFacePicking(true);
    setPackMessage("Picking a model face…");
    try {
      const hit = await api.scene3dUvFacePick({ document: doc, page: targetPageId, node: node.id, primitive, x, y });
      if (placingOnFace) {
        const id = graphicDraftId.trim();
        if (!id) return setPackMessage("Enter a stable ID for this surface graphic before picking a face.");
        if (hit.triangle === null) return setPackMessage("No face on the selected primitive at that point.");
        if (!hit.material_slot) return setPackMessage(`Face ${hit.triangle} has no named material slot to receive artwork.`);
        if (!hit.uv || hit.uv.some((value) => !Number.isFinite(value) || value < 0 || value > 1)) return setPackMessage(`Face ${hit.triangle} has no normalized UV at the hit point. Generate or repair its UVs first.`);
        const center: [number, number] = [Math.max(0.125, Math.min(0.875, hit.uv[0])), Math.max(0.125, Math.min(0.875, hit.uv[1]))];
        const graphic = makeLinkedGraphic(id, hit.material_slot, "decal", center);
        if (!graphic) return;
        commitLinkedGraphic(graphic);
        setPlacingOnFace(false);
        return;
      }
      if (hit.island) {
        setIslandId(hit.island);
        setGraphicId("");
        setPackMessage(`Selected UV island “${hit.island}” from face ${hit.triangle}.`);
      } else if (hit.triangle !== null) {
        setPackMessage(`Face ${hit.triangle}${hit.material_slot ? ` · ${hit.material_slot}` : ""} selected; it has no editable UV island assigned yet.`);
      } else {
        setPackMessage("No model face at that point.");
      }
    } catch (reason) {
      setPackMessage(reason instanceof Error ? reason.message : "Could not pick a model face.");
    } finally {
      setFacePicking(false);
    }
  };

  const pickModelFace = (event: MouseEvent<HTMLImageElement>) => {
    if (suppressNextModelClick.current) {
      suppressNextModelClick.current = false;
      return;
    }
    const bounds = event.currentTarget.getBoundingClientRect();
    if (!bounds.width || !bounds.height) return;
    void pickModelAt(
      Math.max(0, Math.min(1, (event.clientX - bounds.left) / bounds.width)),
      Math.max(0, Math.min(1, (event.clientY - bounds.top) / bounds.height)),
    );
  };

  const modelFaceCoordinates = (image: HTMLImageElement, clientX: number, clientY: number): [number, number] => {
    const bounds = image.getBoundingClientRect();
    return [
      Math.max(0, Math.min(1, (clientX - bounds.left) / Math.max(1, bounds.width))),
      Math.max(0, Math.min(1, (clientY - bounds.top) / Math.max(1, bounds.height))),
    ];
  };

  const beginModelGraphicDrag = (event: ReactPointerEvent<HTMLImageElement>) => {
    if (!selectedGraphic || selectedGraphic.mode !== "decal" || placingOnFace || nodeLocked || !doc || !node || !targetPageId) return;
    event.preventDefault();
    event.currentTarget.setPointerCapture(event.pointerId);
    const start = modelFaceCoordinates(event.currentTarget, event.clientX, event.clientY);
    const hit = api.scene3dUvFacePick({ document: doc, page: targetPageId, node: node.id, primitive, x: start[0], y: start[1] });
    modelGraphicDrag.current = { pointerId: event.pointerId, start, clientStart: [event.clientX, event.clientY], hit, moved: false };
  };

  const moveModelGraphicDrag = (event: ReactPointerEvent<HTMLImageElement>) => {
    const drag = modelGraphicDrag.current;
    if (!drag || drag.pointerId !== event.pointerId) return;
    if ((event.clientX - drag.clientStart[0]) ** 2 + (event.clientY - drag.clientStart[1]) ** 2 >= 16) drag.moved = true;
  };

  const endModelGraphicDrag = async (event: ReactPointerEvent<HTMLImageElement>) => {
    const drag = modelGraphicDrag.current;
    if (!drag || drag.pointerId !== event.pointerId) return;
    modelGraphicDrag.current = null;
    if (event.currentTarget.hasPointerCapture(event.pointerId)) event.currentTarget.releasePointerCapture(event.pointerId);
    if (!drag.moved || !doc || !node || !targetPageId || !selectedGraphic) return;
    suppressNextModelClick.current = true;
    window.setTimeout(() => { suppressNextModelClick.current = false; }, 0);
    setFacePicking(true);
    try {
      const endPoint = modelFaceCoordinates(event.currentTarget, event.clientX, event.clientY);
      const [startHit, end] = await Promise.all([
        drag.hit,
        api.scene3dUvFacePick({ document: doc, page: targetPageId, node: node.id, primitive, x: endPoint[0], y: endPoint[1] }),
      ]);
      if (!startHit.uv || !end.uv || startHit.material_slot !== selectedGraphic.slot || end.material_slot !== selectedGraphic.slot) {
        setPackMessage("The decal can only move across visible faces using its current material slot.");
        return;
      }
      const scale = selectedGraphic.scale ?? [1, 1];
      const offset = selectedGraphic.offset ?? [0.5, 0.5];
      updateGraphic(selectedGraphic.id, { offset: [
        Math.max(scale[0] / 2, Math.min(1 - scale[0] / 2, offset[0] + end.uv[0] - startHit.uv[0])),
        Math.max(scale[1] / 2, Math.min(1 - scale[1] / 2, offset[1] + end.uv[1] - startHit.uv[1])),
      ] });
      setPackMessage(`Moved “${selectedGraphic.id}” on slot ${selectedGraphic.slot}.`);
    } catch (reason) {
      setPackMessage(reason instanceof Error ? reason.message : "Could not move the surface decal.");
    } finally {
      setFacePicking(false);
    }
  };

  const placeGraphic = (graphic: SurfaceGraphic, offset: [number, number]) => {
    const scale = graphic.scale ?? [1, 1];
    updateGraphic(graphic.id, {
      offset: [
        Math.max(scale[0] / 2, Math.min(1 - scale[0] / 2, offset[0])),
        Math.max(scale[1] / 2, Math.min(1 - scale[1] / 2, offset[1])),
      ],
    });
  };

  const beginGraphicTransformGesture = (mode: GraphicTransformGesture["mode"], event: ReactPointerEvent<HTMLButtonElement>) => {
    if (!selectedGraphic || selectedGraphic.mode !== "decal" || nodeLocked || !canvasRef.current) return;
    event.preventDefault();
    event.stopPropagation();
    event.currentTarget.setPointerCapture(event.pointerId);
    const rect = canvasRef.current.getBoundingClientRect();
    const transform: GraphicTransform = {
      scale: [...selectedGraphicScale],
      offset: [...selectedGraphicOffset],
      rotate: selectedGraphicRotation,
    };
    const center: [number, number] = [
      rect.left + transform.offset[0] * rect.width,
      rect.top + transform.offset[1] * rect.height,
    ];
    const startAngle = Math.atan2(event.clientY - center[1], event.clientX - center[0]);
    graphicTransformGesture.current = {
      mode,
      pointerId: event.pointerId,
      captureTarget: event.currentTarget,
      start: [event.clientX, event.clientY],
      center,
      startAngle,
      transform,
      draft: transform,
      moved: false,
    };
    setGraphicTransformDraft(transform);
  };

  const moveGraphicTransformGesture = (event: ReactPointerEvent<HTMLButtonElement>) => {
    const gesture = graphicTransformGesture.current;
    const rect = canvasRef.current?.getBoundingClientRect();
    if (!gesture || gesture.pointerId !== event.pointerId || !rect) return;
    event.preventDefault();
    event.stopPropagation();
    if ((event.clientX - gesture.start[0]) ** 2 + (event.clientY - gesture.start[1]) ** 2 >= 9) gesture.moved = true;
    let draft = gesture.draft;
    if (gesture.mode === "resize") {
      const radians = gesture.transform.rotate * Math.PI / 180;
      const dx = event.clientX - gesture.start[0];
      const dy = event.clientY - gesture.start[1];
      const localU = (dx * Math.cos(radians) + dy * Math.sin(radians)) / Math.max(1, rect.width);
      const localV = (-dx * Math.sin(radians) + dy * Math.cos(radians)) / Math.max(1, rect.height);
      const scale: [number, number] = [
        Math.max(0.01, Math.min(1, gesture.transform.scale[0] + localU * 2)),
        Math.max(0.01, Math.min(1, gesture.transform.scale[1] + localV * 2)),
      ];
      draft = {
        ...gesture.transform,
        scale,
        offset: [
          Math.max(scale[0] / 2, Math.min(1 - scale[0] / 2, gesture.transform.offset[0])),
          Math.max(scale[1] / 2, Math.min(1 - scale[1] / 2, gesture.transform.offset[1])),
        ],
      };
    } else {
      const angle = Math.atan2(event.clientY - gesture.center[1], event.clientX - gesture.center[0]);
      let delta = angle - gesture.startAngle;
      if (delta > Math.PI) delta -= Math.PI * 2;
      else if (delta < -Math.PI) delta += Math.PI * 2;
      draft = {
        ...gesture.transform,
        rotate: Math.max(-360, Math.min(360, gesture.transform.rotate + delta * 180 / Math.PI)),
      };
    }
    gesture.draft = draft;
    setGraphicTransformDraft(draft);
  };

  const endGraphicTransformGesture = (event: ReactPointerEvent<HTMLButtonElement>, commit: boolean) => {
    const gesture = graphicTransformGesture.current;
    if (!gesture || gesture.pointerId !== event.pointerId) return;
    event.preventDefault();
    event.stopPropagation();
    if (event.currentTarget.hasPointerCapture(event.pointerId)) event.currentTarget.releasePointerCapture(event.pointerId);
    graphicTransformGesture.current = null;
    if (commit && gesture.moved && selectedGraphic) {
      updateGraphic(selectedGraphic.id, gesture.draft);
      setPackMessage(`Updated scale and rotation for “${selectedGraphic.id}”.`);
    } else {
      setGraphicTransformDraft(null);
    }
  };

  const keyboardGraphicTransform = (mode: GraphicTransformGesture["mode"], event: React.KeyboardEvent<HTMLButtonElement>) => {
    if (!selectedGraphic || selectedGraphic.mode !== "decal") return;
    if (event.key === "Escape") {
      const gesture = graphicTransformGesture.current;
      if (gesture?.captureTarget.hasPointerCapture(gesture.pointerId)) {
        gesture.captureTarget.releasePointerCapture(gesture.pointerId);
      }
      graphicTransformGesture.current = null;
      setGraphicTransformDraft(null);
      return;
    }
    const step = event.shiftKey ? 0.05 : 0.01;
    const transform: GraphicTransform = {
      scale: [...selectedGraphicScale],
      offset: [...selectedGraphicOffset],
      rotate: selectedGraphicRotation,
    };
    if (mode === "resize") {
      if (event.key === "ArrowLeft") transform.scale[0] -= step;
      else if (event.key === "ArrowRight") transform.scale[0] += step;
      else if (event.key === "ArrowUp") transform.scale[1] -= step;
      else if (event.key === "ArrowDown") transform.scale[1] += step;
      else return;
      transform.scale = [
        Math.max(0.01, Math.min(1, transform.scale[0])),
        Math.max(0.01, Math.min(1, transform.scale[1])),
      ];
      transform.offset = [
        Math.max(transform.scale[0] / 2, Math.min(1 - transform.scale[0] / 2, transform.offset[0])),
        Math.max(transform.scale[1] / 2, Math.min(1 - transform.scale[1] / 2, transform.offset[1])),
      ];
    } else if (event.key === "ArrowLeft" || event.key === "ArrowRight") {
      transform.rotate = Math.max(-360, Math.min(360, transform.rotate + (event.key === "ArrowRight" ? 1 : -1) * (event.shiftKey ? 15 : 5)));
    } else if (event.key === "Home") {
      transform.rotate = 0;
    } else {
      return;
    }
    event.preventDefault();
    updateGraphic(selectedGraphic.id, transform);
    setPackMessage(mode === "resize" ? `Resized “${selectedGraphic.id}”.` : `Rotated “${selectedGraphic.id}”.`);
  };

  const addIsland = (event: React.FormEvent) => {
    event.preventDefault();
    const triangles = triangleText.split(",").map((item) => item.trim()).filter(Boolean).map(Number);
    if (!newId.trim()) return setFormError("Give this island a stable ID.");
    if (!triangles.length || triangles.some((value) => !Number.isInteger(value) || value < 0 || value >= (current?.triangles ?? 0))) return setFormError(`Enter triangle indices from 0 to ${Math.max(0, (current?.triangles ?? 1) - 1)}.`);
    if (new Set(triangles).size !== triangles.length) return setFormError("Each triangle can appear only once in the selection.");
    if (islands.some((item) => item.id === newId.trim())) return setFormError("That island ID is already used on this primitive.");
    const occupied = new Set(islands.flatMap((item) => item.triangles));
    if (triangles.some((value) => occupied.has(value))) return setFormError("A selected triangle already belongs to an island in this UV set.");
    const id = newId.trim();
    editOverlay((overlay) => {
      overlay.islands = [...(overlay.islands ?? []), { id, uv_set: 0, triangles, transform: { scale: [1, 1], offset: [0, 0], rotate: 0 } }];
    }, "Add UV island");
    setIslandId(id);
    setGraphicId("");
    setTriangleText("");
    setFormError("");
  };

  const applyProjection = () => editOverlay((overlay) => { overlay.projection = projection; }, "Project UVs");
  const removeIsland = () => {
    if (!selectedIsland) return;
    const id = selectedIsland.id;
    editOverlay((overlay) => { overlay.islands = (overlay.islands ?? []).filter((item) => item.id !== id); if (!overlay.islands.length) delete overlay.islands; }, "Remove UV island");
    setIslandId("");
  };

  const packIslands = async () => {
    if (!doc || !node || !targetPageId || !islands.length || packing) return;
    const padding = Number(packPadding);
    if (!Number.isFinite(padding) || padding < 0 || padding >= 0.25) {
      setPackError("Padding must be at least 0 and less than 0.25 UV units.");
      setPackMessage("");
      return;
    }
    setPacking(true);
    setPackError("");
    setPackMessage("");
    try {
      const result = await api.scene3dUvPack({ document: doc, page: targetPageId, node: node.id, primitive, padding });
      studio.replaceDoc(result.document, "Pack UV islands");
      setPackMessage(`Packed ${result.result.islands} island${result.result.islands === 1 ? "" : "s"}.`);
      setRefreshNonce((value) => value + 1);
    } catch (reason) {
      setPackError(reason instanceof Error ? reason.message : "Could not pack UV islands.");
    } finally {
      setPacking(false);
    }
  };

  const exportSvg = () => {
    if (!preview?.svg) return;
    const url = URL.createObjectURL(new Blob([preview.svg], { type: "image/svg+xml" }));
    const link = document.createElement("a");
    link.href = url;
    link.download = `${node?.id ?? "model"}-primitive-${primitive}-uv.svg`;
    link.click();
    window.setTimeout(() => URL.revokeObjectURL(url), 1000);
  };

  const exportPng = () => {
    if (!preview?.png_base64) return;
    const link = document.createElement("a");
    link.href = `data:image/png;base64,${preview.png_base64}`;
    link.download = `${node?.id ?? "model"}-primitive-${primitive}-uv.png`;
    link.click();
  };

  const onUvDragOver = (event: ReactDragEvent<HTMLDivElement>) => {
    if (!event.dataTransfer.types.includes("application/x-pentool-uv-source")) return;
    event.preventDefault();
    event.dataTransfer.dropEffect = "copy";
    setDragOverUv(true);
  };

  const onUvDrop = (event: ReactDragEvent<HTMLDivElement>) => {
    event.preventDefault();
    setDragOverUv(false);
    if (!doc || !node) {
      setPackMessage("Select a 3D model before dropping artwork on the UV layout.");
      return;
    }
    if (nodeLocked) {
      setPackMessage("Unlock the 3D model layer before placing artwork on its surface.");
      return;
    }
    if (!preview?.material_slot) {
      setPackMessage("Choose a model primitive with a named material slot before placing artwork.");
      return;
    }
    if (!canvasRef.current) {
      setPackMessage("The UV layout is not ready yet. Wait for it to load, then try again.");
      return;
    }
    try {
      const source = JSON.parse(event.dataTransfer.getData("application/x-pentool-uv-source")) as
        | { kind: "page"; page: string }
        | { kind: "node"; page: string; node: string };
      const sourcePage = doc.pages?.find((page) => page.id === source.page);
      if (!sourcePage || source.page === targetPageId && source.kind === "node" && source.node === node.id) {
        setPackMessage("Choose artwork on a page other than the target model.");
        return;
      }
      if (source.kind !== "node" || !source.node) {
        setPackMessage("Drag a selected artwork object rather than a whole-page preview.");
        return;
      }
      const sourceNode = findNode(sourcePage, source.node);
      if (!sourceNode) {
        setPackMessage("The dragged artwork is no longer available. Select it again on the page canvas.");
        return;
      }
      if (sourceNode.layer.locked || sourceNode.node.visible === false) {
        setPackMessage("Unlock and show the source artwork before linking it to this surface.");
        return;
      }
      const rect = canvasRef.current.getBoundingClientRect();
      const offset: [number, number] = [
        Math.max(0.125, Math.min(0.875, (event.clientX - rect.left) / Math.max(1, rect.width))),
        Math.max(0.125, Math.min(0.875, (event.clientY - rect.top) / Math.max(1, rect.height))),
      ];
      const baseId = `uv-${source.node}-art`.replace(/[^A-Za-z0-9._-]+/g, "-").slice(0, 80);
      const existing = new Set((node.surface_graphics as SurfaceGraphic[] | undefined)?.map((graphic) => graphic.id) ?? []);
      let id = baseId;
      for (let suffix = 2; existing.has(id); suffix += 1) id = `${baseId}-${suffix}`;
      const graphic = makeLinkedGraphic(id, preview.material_slot, "decal", offset, source);
      if (!graphic) return;
      setGraphicSourceKind("node");
      setGraphicSourcePage(source.page);
      setGraphicSourceNode(source.node);
      setGraphicMode("decal");
      setGraphicDraftId(id);
      commitLinkedGraphic(graphic);
    } catch {
      setPackMessage("This drag did not contain a Pentool page object. Select artwork and drag its handle from the page canvas.");
    }
  };

  return <div className="flex min-h-0 min-w-0 flex-1 flex-col lg:flex-row">
    <section aria-label="3D model viewport" className="flex min-h-[320px] min-w-0 flex-1 flex-col border-b lg:min-h-0 lg:border-b-0 lg:border-r">
      <header className="flex min-h-12 shrink-0 flex-wrap items-center gap-2 border-b bg-card px-3 py-1 text-xs">
        <span className="font-medium">{showPageContext ? "Page context" : "3D model"}</span>
        <span className="mr-auto truncate text-muted-foreground">{node?.name || node?.id || "Select a 3D model in the page"}</span>
        {showPageContext && node && <span className="w-full text-[11px] text-muted-foreground">Select artwork on this page, then drag its handle onto the UV layout.</span>}
        {!showPageContext && <Button variant={placingOnFace ? "default" : "outline"} className="min-h-10" aria-pressed={placingOnFace} aria-busy={facePicking} disabled={!node || nodeLocked || facePicking || !graphicSourcePage || (graphicSourceKind === "node" && !graphicSourceNode)} onClick={() => { setPlacingOnFace((active) => !active); setPackMessage(placingOnFace ? "Face placement cancelled." : "Choose a visible face in the 3D preview to place linked artwork."); }}>{placingOnFace ? "Cancel face placement" : "Place art on face"}</Button>}
        {!showPageContext && selectedGraphic?.mode === "decal" && <div role="group" aria-label={`3D viewport transform controls for ${selectedGraphic.id}`} className="flex flex-wrap items-center gap-1">
          <span className="px-1 text-[11px] text-muted-foreground">Decal</span>
          <Button variant="outline" className="min-h-10" disabled={nodeLocked || facePicking} aria-label={`Shrink ${selectedGraphic.id}`} onClick={() => adjustSelectedDecalScale(-0.05)}>−</Button>
          <Button variant="outline" className="min-h-10" disabled={nodeLocked || facePicking} aria-label={`Enlarge ${selectedGraphic.id}`} onClick={() => adjustSelectedDecalScale(0.05)}>+</Button>
          <Button variant="outline" className="min-h-10" disabled={nodeLocked || facePicking} aria-label={`Rotate ${selectedGraphic.id} counterclockwise`} onClick={() => updateGraphic(selectedGraphic.id, { rotate: Math.max(-360, selectedGraphicRotation - 15) })}>↶ 15°</Button>
          <Button variant="outline" className="min-h-10" disabled={nodeLocked || facePicking} aria-label={`Rotate ${selectedGraphic.id} clockwise`} onClick={() => updateGraphic(selectedGraphic.id, { rotate: Math.min(360, selectedGraphicRotation + 15) })}>↷ 15°</Button>
        </div>}
        <Button variant="outline" className="min-h-10" aria-pressed={showPageContext} onClick={() => {
          if (!showPageContext && node && studio.pageId) setUvTarget({ pageId: studio.pageId, nodeId: node.id });
          setShowPageContext((value) => !value);
        }}>{showPageContext ? "Show model" : "Show page"}</Button>
      </header>
      {showPageContext ? <Canvas dragToUv active={active && showPageContext} />
        : !node ? <div className="flex flex-1 items-center justify-center p-6"><div className="max-w-sm space-y-4 text-center">
              <h2 className="text-lg font-semibold">Create a packaging mockup</h2>
              <p className="text-sm leading-relaxed text-muted-foreground">Choose a package template and dimensions in the right panel, or import a 3D model. Your flat artwork and folded preview will appear here.</p>
              {doc && doc.version < 11 && <div className="space-y-3 rounded-md border bg-card p-4"><p className="text-xs text-muted-foreground">This project uses an older document format. Upgrade it to enable packaging tools. The file changes only when you save.</p><Button disabled={studio.busy} onClick={() => { if (window.confirm("Upgrade this project to format v11 for packaging? Save keeps the upgrade; Undo reverts it.")) void studio.upgradeDoc(); }}>Enable packaging tools</Button></div>}
            </div></div>
          : modelPreviewLoading && !modelPreview ? <div role="status" className="m-4 flex-1 animate-pulse rounded border bg-muted" aria-label="Rendering 3D model preview" />
            : modelPreviewError ? <div className="m-4 flex flex-col items-start justify-center gap-2 rounded border border-destructive/40 bg-destructive/5 p-3 text-xs"><p className="text-destructive">Couldn’t render this model preview.</p><p className="break-words text-muted-foreground">{modelPreviewError}</p><Button variant="outline" className="min-h-10" onClick={() => setRefreshNonce((value) => value + 1)}>Try again</Button></div>
              : modelPreview ? <div className="grid min-h-0 flex-1 place-items-center overflow-auto bg-muted/30 p-3"><img onPointerDown={beginModelGraphicDrag} onPointerMove={moveModelGraphicDrag} onPointerUp={(event) => { void endModelGraphicDrag(event); }} onPointerCancel={() => { modelGraphicDrag.current = null; }} onClick={pickModelFace} onKeyDown={(event) => { if (event.key === "Enter" || event.key === " ") { event.preventDefault(); void pickModelAt(0.5, 0.5); } }} role="button" tabIndex={0} aria-label={selectedGraphic?.mode === "decal" && !placingOnFace ? `Drag linked decal ${selectedGraphic.id} across faces on slot ${selectedGraphic.slot}; click to select a UV island` : placingOnFace ? "Choose a visible model face to place linked artwork" : "Select a visible model face and its UV island"} aria-busy={facePicking} title={selectedGraphic?.mode === "decal" && !placingOnFace ? "Drag the selected decal to reposition it on visible faces with the same material slot" : placingOnFace ? "Click a face to place linked art; keyboard activation picks the preview center" : "Click a face to select its UV island"} className={`max-h-full max-w-full touch-none object-contain focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2 ${selectedGraphic?.mode === "decal" && !placingOnFace ? "cursor-move" : "cursor-crosshair"} ${placingOnFace ? "rounded ring-2 ring-primary" : ""}`} src={`data:image/png;base64,${modelPreview.png_base64}`} alt={selectedIsland ? `3D model preview with UV island ${selectedIsland.id} highlighted` : `3D model preview for ${node.name || node.id}`} /></div>
                : <div className="flex flex-1 items-center justify-center p-6"><Empty>The 3D preview will appear here.</Empty></div>}
    </section>
    <section aria-label="UV layout canvas" className="flex min-h-[380px] min-w-0 flex-1 flex-col lg:min-h-0 lg:max-w-[52%]">
      <header className="flex min-h-10 shrink-0 flex-wrap items-center gap-2 border-b bg-card px-3 py-1.5 text-xs">
        <span className="mr-auto font-medium">UV layout</span>
        {showPageContext && <span role="status" className="text-muted-foreground">Drop selected page artwork here to create a linked decal.</span>}
        <label className="flex items-center gap-1 text-muted-foreground">Primitive<select aria-label="UV primitive" className="h-10 rounded border bg-input px-2 text-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring" value={primitive} onChange={(event) => setPrimitive(Number(event.target.value))}>{entries.map((entry) => <option key={entry.primitive} value={entry.primitive}>{entry.primitive} · {entry.name || `${entry.triangles} triangles`}</option>)}</select></label>
        {uvGraphics.length > 0 && <label className="flex items-center gap-1 text-muted-foreground">UV graphic<select aria-label="UV graphic to edit" className="h-10 max-w-40 rounded border bg-input px-2 text-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring" value={selectedGraphic?.id ?? ""} onChange={(event) => { setGraphicId(event.target.value); setDragGraphicOffset(null); setGraphicTransformDraft(null); }}>{uvGraphics.map((graphic) => <option key={graphic.id} value={graphic.id}>{graphic.id} · {graphic.mode}</option>)}</select></label>}
        <label className="flex items-center gap-1 text-muted-foreground">Texture<select aria-label="Texture resolution" className="h-10 rounded border bg-input px-2 text-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring" value={textureResolution} onChange={(event) => setTextureResolution(Number(event.target.value))}><option value={512}>512²</option><option value={1024}>1K</option><option value={2048}>2K</option><option value={4096}>4K</option><option value={8192}>8K</option><option value={16384}>16K</option></select></label>
        <label className="flex items-center gap-1 text-muted-foreground">Warn below<input aria-label="Minimum texel density in pixels per model unit" type="number" min="0.01" step="1" inputMode="decimal" placeholder="off" className="h-9 w-20 rounded border bg-input px-2 text-foreground placeholder:text-muted-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring" value={minimumDensity} onChange={(event) => setMinimumDensity(event.target.value)} /></label>
        <Button variant="outline" size="icon" className="size-10" aria-label="Refresh UV layout" title="Refresh" disabled={loading} onClick={() => setRefreshNonce((value) => value + 1)}><RefreshCw size={14} /></Button>
        <Button variant="outline" className="min-h-10" onClick={exportSvg} disabled={!preview?.svg}><Download size={13} />SVG</Button>
        <Button variant="outline" className="min-h-10" onClick={exportPng} disabled={!preview?.png_base64}><Download size={13} />PNG</Button>
      </header>
      {!node ? <div className="flex flex-1 items-center justify-center p-6"><Empty>The UV canvas appears when a 3D model is selected.</Empty></div>
        : loading && !preview ? <div role="status" className="m-4 flex-1 animate-pulse rounded border bg-muted" aria-label="Loading UV layout" />
        : error ? <div className="m-4 flex flex-col items-start gap-2 rounded border border-destructive/40 bg-destructive/5 p-3 text-xs"><p className="text-destructive">Couldn’t build the UV layout.</p><p className="break-words text-muted-foreground">{error}</p><Button variant="outline" className="min-h-9" onClick={() => setRefreshNonce((value) => value + 1)}>Try again</Button></div>
        : preview && current ? <div className="flex min-h-0 flex-1 flex-col overflow-auto p-3">
          <div className="mx-auto aspect-square w-full max-w-[640px] border bg-card">
            <div
              ref={canvasRef}
              className={`relative size-full ${dragOverUv ? "ring-4 ring-inset ring-primary" : ""}`}
              onDragOver={onUvDragOver}
              onDragLeave={(event) => { if (!event.currentTarget.contains(event.relatedTarget as Node | null)) setDragOverUv(false); }}
              onDrop={onUvDrop}
            ><img draggable={false} className="size-full select-none object-contain" src={`data:image/svg+xml;charset=utf-8,${encodeURIComponent(preview.svg)}`} alt={`UV wireframe for primitive ${primitive}; possible UV discontinuities are highlighted in red`} />
              {selectedIsland && displayedIslandBounds && <>
                <div aria-hidden="true" className="pointer-events-none absolute border-2 border-primary" style={{ left: `${displayedIslandBounds.x * 100}%`, top: `${displayedIslandBounds.y * 100}%`, width: `${displayedIslandBounds.width * 100}%`, height: `${displayedIslandBounds.height * 100}%`, transform: `rotate(${displayedIslandBounds.rotate}deg)`, transformOrigin: "center" }} />
                {!selectedGraphic && !nodeLocked && islandHandlePositions && <>
                  <button
                    type="button"
                    aria-label={`Move UV island ${selectedIsland.id}; drag or use arrow keys`}
                    title="Drag to move · arrow keys for fine adjustment"
                    className="absolute z-10 grid min-h-[40px] min-w-[40px] touch-none cursor-move place-items-center border border-dashed border-primary/70 bg-primary/10 text-primary focus-visible:z-20 focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2"
                    style={{ left: `${displayedIslandBounds.x * 100}%`, top: `${displayedIslandBounds.y * 100}%`, width: `${displayedIslandBounds.width * 100}%`, height: `${displayedIslandBounds.height * 100}%`, transform: `rotate(${displayedIslandBounds.rotate}deg)`, transformOrigin: "center" }}
                    onPointerDown={(event) => beginIslandTransform("move", event)}
                    onPointerMove={moveIslandTransform}
                    onPointerUp={(event) => endIslandTransform(event, true)}
                    onPointerCancel={(event) => endIslandTransform(event, false)}
                    onKeyDown={(event) => keyboardIslandTransform("move", event)}
                  ><span aria-hidden="true" className="pointer-events-none grid size-10 place-items-center rounded-full border-2 border-primary bg-card shadow-sm"><Move size={14} /></span></button>
                  <button
                    type="button"
                    aria-label={`Scale UV island ${selectedIsland.id}; drag or use arrow keys`}
                    title="Drag to scale · arrow keys adjust U/V"
                    className="absolute z-20 grid size-[40px] touch-none -translate-x-1/2 -translate-y-1/2 place-items-center rounded-full border-2 border-primary bg-card text-primary shadow-sm focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2"
                    style={{ left: `${islandHandlePositions.scale[0] * 100}%`, top: `${islandHandlePositions.scale[1] * 100}%` }}
                    onPointerDown={(event) => beginIslandTransform("scale", event)}
                    onPointerMove={moveIslandTransform}
                    onPointerUp={(event) => endIslandTransform(event, true)}
                    onPointerCancel={(event) => endIslandTransform(event, false)}
                    onKeyDown={(event) => keyboardIslandTransform("scale", event)}
                  ><span aria-hidden="true">↘</span></button>
                  <button
                    type="button"
                    aria-label={`Rotate UV island ${selectedIsland.id}; drag or use arrow keys`}
                    title="Drag to rotate · left/right arrow keys for fine adjustment"
                    className="absolute z-20 grid size-[40px] touch-none -translate-x-1/2 -translate-y-1/2 place-items-center rounded-full border-2 border-primary bg-card text-primary shadow-sm focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2"
                    style={{ left: `${islandHandlePositions.rotate[0] * 100}%`, top: `${islandHandlePositions.rotate[1] * 100}%` }}
                    onPointerDown={(event) => beginIslandTransform("rotate", event)}
                    onPointerMove={moveIslandTransform}
                    onPointerUp={(event) => endIslandTransform(event, true)}
                    onPointerCancel={(event) => endIslandTransform(event, false)}
                    onKeyDown={(event) => keyboardIslandTransform("rotate", event)}
                  ><span aria-hidden="true">↻</span></button>
                </>}
              </>}
              {selectedGraphic?.mode === "decal" && <button
                type="button"
                disabled={nodeLocked}
                aria-label={`Move linked UV decal ${selectedGraphic.id}; drag to reposition or use arrow keys`}
                aria-pressed="true"
                title={`${selectedGraphic.id} · ${selectedGraphic.slot}`}
                className="absolute min-h-10 min-w-10 touch-none cursor-grab border-2 border-primary bg-primary/20 text-primary-foreground active:cursor-grabbing focus-visible:z-10 focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2"
                style={{ left: `${(selectedGraphicOffset[0] - selectedGraphicScale[0] / 2) * 100}%`, top: `${(selectedGraphicOffset[1] - selectedGraphicScale[1] / 2) * 100}%`, width: `${selectedGraphicScale[0] * 100}%`, height: `${selectedGraphicScale[1] * 100}%`, transform: `rotate(${selectedGraphicRotation}deg)` }}
                onPointerDown={(event) => {
                  event.currentTarget.setPointerCapture(event.pointerId);
                  graphicDragMoved.current = false;
                  graphicDragStart.current = [event.clientX, event.clientY];
                  setGraphicId(selectedGraphic.id);
                  setDragGraphicOffset(selectedGraphicOffset);
                }}
                onPointerMove={(event) => {
                  if (!event.currentTarget.hasPointerCapture(event.pointerId) || !canvasRef.current) return;
                  const start = graphicDragStart.current;
                  if (!start || (event.clientX - start[0]) ** 2 + (event.clientY - start[1]) ** 2 < 16) return;
                  graphicDragMoved.current = true;
                  const rect = canvasRef.current.getBoundingClientRect();
                  const x = (event.clientX - rect.left) / Math.max(1, rect.width);
                  const y = (event.clientY - rect.top) / Math.max(1, rect.height);
                  setDragGraphicOffset([
                    Math.max(selectedGraphicScale[0] / 2, Math.min(1 - selectedGraphicScale[0] / 2, x)),
                    Math.max(selectedGraphicScale[1] / 2, Math.min(1 - selectedGraphicScale[1] / 2, y)),
                  ]);
                }}
                onPointerUp={(event) => {
                  if (!event.currentTarget.hasPointerCapture(event.pointerId)) return;
                  event.currentTarget.releasePointerCapture(event.pointerId);
                  graphicDragStart.current = null;
                  if (!graphicDragMoved.current || !canvasRef.current) {
                    setDragGraphicOffset(null);
                    setGraphicTransformDraft(null);
                    return;
                  }
                  const rect = canvasRef.current.getBoundingClientRect();
                  placeGraphic(selectedGraphic, [
                    (event.clientX - rect.left) / Math.max(1, rect.width),
                    (event.clientY - rect.top) / Math.max(1, rect.height),
                  ]);
                }}
                onPointerCancel={() => { graphicDragStart.current = null; setDragGraphicOffset(null); setGraphicTransformDraft(null); }}
                onKeyDown={(event) => {
                  const step = event.shiftKey ? 0.05 : 0.01;
                  const next: [number, number] = [...selectedGraphicOffset];
                  if (event.key === "ArrowLeft") next[0] -= step;
                  else if (event.key === "ArrowRight") next[0] += step;
                  else if (event.key === "ArrowUp") next[1] -= step;
                  else if (event.key === "ArrowDown") next[1] += step;
                  else return;
                  event.preventDefault();
                  placeGraphic(selectedGraphic, next);
                }}
              />}
              {selectedGraphic?.mode === "decal" && selectedGraphicHandlePositions && !nodeLocked && <>
                <button
                  type="button"
                  aria-label={`Resize linked UV decal ${selectedGraphic.id}; drag to scale`}
                  title="Drag to resize"
                  className="absolute z-10 flex size-[40px] touch-none items-center justify-center rounded-full border-2 border-primary bg-card text-primary shadow-sm cursor-nwse-resize focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2"
                  style={{ left: `${selectedGraphicHandlePositions.resize[0] * 100}%`, top: `${selectedGraphicHandlePositions.resize[1] * 100}%`, transform: "translate(-50%, -50%)" }}
                  onPointerDown={(event) => beginGraphicTransformGesture("resize", event)}
                  onPointerMove={moveGraphicTransformGesture}
                  onPointerUp={(event) => endGraphicTransformGesture(event, true)}
                  onPointerCancel={(event) => endGraphicTransformGesture(event, false)}
                  onKeyDown={(event) => keyboardGraphicTransform("resize", event)}
                ><span aria-hidden="true">↘</span>
                </button>
                <button
                  type="button"
                  aria-label={`Rotate linked UV decal ${selectedGraphic.id}; drag to rotate`}
                  title="Drag to rotate"
                  className="absolute z-10 flex size-[40px] touch-none items-center justify-center rounded-full border-2 border-primary bg-card text-primary shadow-sm cursor-grab focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2"
                  style={{ left: `${selectedGraphicHandlePositions.rotate[0] * 100}%`, top: `${selectedGraphicHandlePositions.rotate[1] * 100}%`, transform: "translate(-50%, -50%)" }}
                  onPointerDown={(event) => beginGraphicTransformGesture("rotate", event)}
                  onPointerMove={moveGraphicTransformGesture}
                  onPointerUp={(event) => endGraphicTransformGesture(event, true)}
                  onPointerCancel={(event) => endGraphicTransformGesture(event, false)}
                  onKeyDown={(event) => keyboardGraphicTransform("rotate", event)}
                ><span aria-hidden="true">↻</span>
                </button>
              </>}
            </div>
          </div>
          <div className="mx-auto mt-2 w-full max-w-[640px] space-y-1 text-[11px] text-muted-foreground">
            <p>{current.triangles} triangles · {current.vertices} vertices · {current.uv_sets} UV set{current.uv_sets === 1 ? "" : "s"}</p>
            {current.texel_density.map((set) => <p key={set.uv_set}>Set {set.uv_set} · {set.texture_resolution_px[0]}² · {set.texel_density_px_per_model_unit == null ? "Density unavailable" : `${set.texel_density_px_per_model_unit.toLocaleString(undefined, { maximumFractionDigits: 1 })} px / model unit`}{set.below_minimum_texel_density && <span className="text-warning"> · Below target</span>}</p>)}
            {discontinuityCount > 0 && <p role="status" className="text-warning">{discontinuityCount} possible UV discontinuities; affected edges are highlighted in red. Coincident separate shells may also be reported.</p>}
            {seamAnalysisTruncated && <p role="status" className="text-warning">Seam analysis and highlighting are bounded; some triangles, edge details, or highlight segments may be omitted.</p>}
            {current.island_overlap_candidates?.length ? <p role="status" className="text-warning">UV triangle overlap: {current.island_overlap_candidates.map((pair) => pair.join(" ↔ ")).join(", ")}.</p> : null}
            {current.island_overlap_analysis_truncated && <p role="status" className="text-warning">Exact overlap analysis reached its bounded triangle-pair budget; additional island pairs may not be reported.</p>}
            {!current.uv_sets && <p>No UV set yet. Choose a projection below to generate one.</p>}
          </div>
          <div className="mx-auto mt-4 w-full max-w-[640px] space-y-4 border-t pt-3">
            <form className="space-y-2 rounded border bg-card p-3" onSubmit={addLinkedGraphic} aria-busy={loading}>
              <fieldset className="space-y-2" disabled={nodeLocked || loading}>
                <legend className="px-1 text-xs font-medium">Link artwork to this surface</legend>
                <p className="text-xs text-muted-foreground">Source art stays linked to its page or node; choose decal placement or a repeating fill for the selected material slot.</p>
                <div className="grid grid-cols-2 gap-2">
                  <label className="space-y-1 text-[11px] text-muted-foreground">Graphic ID<input aria-label="New surface graphic ID" spellCheck={false} autoComplete="off" type="text" maxLength={128} className="h-10 w-full rounded border bg-input px-2 text-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring" value={graphicDraftId} onChange={(event) => setGraphicDraftId(event.target.value)} /></label>
                  <label className="space-y-1 text-[11px] text-muted-foreground">Mode<select aria-label="Surface graphic mode" className="h-10 w-full rounded border bg-input px-2 text-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring" value={graphicMode} onChange={(event) => setGraphicMode(event.target.value as "decal" | "fill")}><option value="decal">Decal</option><option value="fill">Repeating fill</option></select></label>
                  <label className="space-y-1 text-[11px] text-muted-foreground">Source type<select aria-label="Surface graphic source type" className="h-10 w-full rounded border bg-input px-2 text-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring" value={graphicSourceKind} onChange={(event) => setGraphicSourceKind(event.target.value as "page" | "node")}><option value="page">Whole page</option><option value="node">Single node</option></select></label>
                  <label className="space-y-1 text-[11px] text-muted-foreground">Source page<select aria-label="Surface graphic source page" className="h-10 w-full rounded border bg-input px-2 text-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring" value={graphicSourcePage} onChange={(event) => { setGraphicSourcePage(event.target.value); setGraphicSourceNode(""); }}>{doc?.pages?.map((page) => <option key={page.id} value={page.id}>{page.name || page.id}</option>)}</select></label>
                  {graphicSourceKind === "node" && <label className="col-span-2 space-y-1 text-[11px] text-muted-foreground">Source node<select aria-label="Surface graphic source node" className="h-10 w-full rounded border bg-input px-2 text-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring" value={graphicSourceNode} onChange={(event) => setGraphicSourceNode(event.target.value)}>{graphicSourceCandidates.map((candidate) => <option key={candidate.id} value={candidate.id}>{candidate.name || candidate.id} · {candidate.kind || "object"}</option>)}</select></label>}
                  <label className="space-y-1 text-[11px] text-muted-foreground">Fit<select aria-label="Surface graphic fit" className="h-10 w-full rounded border bg-input px-2 text-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring" value={graphicFit} onChange={(event) => setGraphicFit(event.target.value)}><option value="contain">Contain</option><option value="cover">Cover</option><option value="stretch">Stretch</option></select></label>
                  <label className="space-y-1 text-[11px] text-muted-foreground">Tile edge<select aria-label="Surface graphic tile edge" className="h-10 w-full rounded border bg-input px-2 text-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring" value={graphicWrap} onChange={(event) => setGraphicWrap(event.target.value)}><option value="clamp">Clamp</option><option value="repeat">Repeat</option><option value="mirror">Mirror</option></select></label>
                </div>
                {!preview?.material_slot && <p role="status" className="text-xs text-warning">This primitive has no named material slot for a linked graphic.</p>}
                {graphicSourceKind === "node" && !graphicSourceCandidates.length && <p role="status" className="text-xs text-muted-foreground">No visible artwork nodes on this page. Choose another source page or link a whole page.</p>}
                <fieldset className="grid grid-cols-2 gap-2 rounded border p-2" aria-label="Source crop rectangle">
                  <legend className="px-1 text-[11px] text-muted-foreground">Source crop · x, y, width, height</legend>
                  {(["X", "Y", "Width", "Height"] as const).map((label, index) => <label key={label} className="space-y-1 text-[11px] text-muted-foreground">{label}<NumberField className="min-h-10" value={graphicCrop[index]} min={index < 2 ? 0 : 0.01} max={index === 0 ? 1 - graphicCrop[2] : index === 1 ? 1 - graphicCrop[3] : 1 - graphicCrop[index === 2 ? 0 : 1]} step={0.01} onCommit={(value) => setGraphicCrop((old) => old.map((part, partIndex) => partIndex === index ? value : part) as [number, number, number, number])} ariaLabel={`New graphic crop ${label.toLowerCase()}`} /></label>)}
                  <p className="col-span-2 text-[11px] text-muted-foreground">Crop rectangle must stay inside the source page or node bounds.</p>
                </fieldset>
                <Button type="submit" variant="outline" className="min-h-10" disabled={!preview?.material_slot || !graphicSourcePage || (graphicSourceKind === "node" && !graphicSourceNode)}>Add linked {graphicMode}</Button>
              </fieldset>
            </form>
            {selectedGraphic?.mode === "decal" && <fieldset className="space-y-2 rounded border bg-card p-2" aria-label={`UV decal placement ${selectedGraphic.id}`}>
              <legend className="px-1 text-[11px] font-medium">UV decal placement · {selectedGraphic.id}</legend>
              <p className="text-xs text-muted-foreground">Drag the outlined region on the UV layout. Changes update the linked graphic and the 3D preview.</p>
              {nodeLocked && <p role="status" className="text-xs text-muted-foreground">Unlock this layer to move its linked artwork.</p>}
              <div className="grid grid-cols-2 gap-x-3 gap-y-2">
                <Row label="Center U"><NumberField className="min-h-10" disabled={nodeLocked} value={selectedGraphicOffset[0]} min={selectedGraphicScale[0] / 2} max={1 - selectedGraphicScale[0] / 2} step={0.01} onCommit={(value) => updateGraphic(selectedGraphic.id, { offset: [value, selectedGraphicOffset[1]] })} ariaLabel={`${selectedGraphic.id} UV center U`} /></Row>
                <Row label="Center V"><NumberField className="min-h-10" disabled={nodeLocked} value={selectedGraphicOffset[1]} min={selectedGraphicScale[1] / 2} max={1 - selectedGraphicScale[1] / 2} step={0.01} onCommit={(value) => updateGraphic(selectedGraphic.id, { offset: [selectedGraphicOffset[0], value] })} ariaLabel={`${selectedGraphic.id} UV center V`} /></Row>
                <Row label="Scale U"><NumberField className="min-h-10" disabled={nodeLocked} value={selectedGraphicScale[0]} min={0.01} max={1} step={0.01} onCommit={(value) => updateGraphic(selectedGraphic.id, { scale: [value, selectedGraphicScale[1]], offset: [Math.max(value / 2, Math.min(1 - value / 2, selectedGraphicOffset[0])), selectedGraphicOffset[1]] })} ariaLabel={`${selectedGraphic.id} UV width`} /></Row>
                <Row label="Scale V"><NumberField className="min-h-10" disabled={nodeLocked} value={selectedGraphicScale[1]} min={0.01} max={1} step={0.01} onCommit={(value) => updateGraphic(selectedGraphic.id, { scale: [selectedGraphicScale[0], value], offset: [selectedGraphicOffset[0], Math.max(value / 2, Math.min(1 - value / 2, selectedGraphicOffset[1]))] })} ariaLabel={`${selectedGraphic.id} UV height`} /></Row>
              </div>
              <Row label="Rotation"><NumberField className="min-h-10" disabled={nodeLocked} value={selectedGraphic.rotate ?? 0} min={-360} max={360} step={1} onCommit={(value) => updateGraphic(selectedGraphic.id, { rotate: value })} ariaLabel={`${selectedGraphic.id} UV decal rotation in degrees`} /></Row>
            </fieldset>}
            {selectedGraphic?.mode === "fill" && <fieldset className="space-y-2 rounded border bg-card p-2" aria-label={`UV tile settings ${selectedGraphic.id}`}>
              <legend className="px-1 text-[11px] font-medium">UV fill tiles · {selectedGraphic.id}</legend>
              <p className="text-xs text-muted-foreground">Repeat count and phase update the linked fill across the selected material surface.</p>
              {nodeLocked && <p role="status" className="text-xs text-muted-foreground">Unlock this layer to edit its linked artwork.</p>}
              <div className="grid grid-cols-2 gap-x-3 gap-y-2">
                <Row label="Tiles U"><NumberField className="min-h-10" disabled={nodeLocked} value={selectedGraphicScale[0]} min={1} max={16} step={1} onCommit={(value) => updateGraphic(selectedGraphic.id, { scale: [value, selectedGraphicScale[1]] })} ariaLabel={`${selectedGraphic.id} horizontal tile count`} /></Row>
                <Row label="Tiles V"><NumberField className="min-h-10" disabled={nodeLocked} value={selectedGraphicScale[1]} min={1} max={16} step={1} onCommit={(value) => updateGraphic(selectedGraphic.id, { scale: [selectedGraphicScale[0], value] })} ariaLabel={`${selectedGraphic.id} vertical tile count`} /></Row>
                <Row label="Phase U"><NumberField className="min-h-10" disabled={nodeLocked} value={selectedGraphicOffset[0]} min={0} max={1} step={0.01} onCommit={(value) => updateGraphic(selectedGraphic.id, { offset: [value, selectedGraphicOffset[1]] })} ariaLabel={`${selectedGraphic.id} horizontal tile phase`} /></Row>
                <Row label="Phase V"><NumberField className="min-h-10" disabled={nodeLocked} value={selectedGraphicOffset[1]} min={0} max={1} step={0.01} onCommit={(value) => updateGraphic(selectedGraphic.id, { offset: [selectedGraphicOffset[0], value] })} ariaLabel={`${selectedGraphic.id} vertical tile phase`} /></Row>
              </div>
              <label className="block space-y-1 text-[11px] text-muted-foreground">Tile edge<select aria-label={`${selectedGraphic.id} tile edge mode`} disabled={nodeLocked} className="h-10 w-full rounded border bg-input px-2 text-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring" value={selectedGraphic.wrap ?? "clamp"} onChange={(event) => updateGraphic(selectedGraphic.id, { wrap: event.target.value })}><option value="clamp">Clamp</option><option value="repeat">Repeat</option><option value="mirror">Mirror</option></select></label>
            </fieldset>}
            {selectedGraphic && <fieldset className="space-y-2 rounded border bg-card p-2" aria-label={`Source crop and fit ${selectedGraphic.id}`}>
              <legend className="px-1 text-[11px] font-medium">Source crop and fit</legend>
              <div className="grid grid-cols-2 gap-2">
                {(["X", "Y", "Width", "Height"] as const).map((label, index) => {
                  const crop = selectedGraphic.crop ?? [0, 0, 1, 1];
                  return <label key={label} className="space-y-1 text-[11px] text-muted-foreground">{label}<NumberField className="min-h-10" disabled={nodeLocked} value={crop[index]} min={index < 2 ? 0 : 0.01} max={index === 0 ? 1 - crop[2] : index === 1 ? 1 - crop[3] : 1 - crop[index === 2 ? 0 : 1]} step={0.01} onCommit={(value) => updateGraphic(selectedGraphic.id, { crop: crop.map((part, partIndex) => partIndex === index ? value : part) as [number, number, number, number] })} ariaLabel={`${selectedGraphic.id} crop ${label.toLowerCase()}`} /></label>;
                })}
              </div>
              <label className="block space-y-1 text-[11px] text-muted-foreground">Fit<select aria-label={`${selectedGraphic.id} source fit`} disabled={nodeLocked} className="h-10 w-full rounded border bg-input px-2 text-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring" value={selectedGraphic.fit ?? "contain"} onChange={(event) => updateGraphic(selectedGraphic.id, { fit: event.target.value })}><option value="contain">Contain</option><option value="cover">Cover</option><option value="stretch">Stretch</option></select></label>
            </fieldset>}
            {!selectedGraphic && <p role="status" className="rounded border bg-muted/50 p-2 text-xs text-muted-foreground">Add a linked UV decal above or select one in the 3D inspector to position it here.</p>}
            <section className="flex flex-wrap items-end gap-2" aria-label="Pack UV islands" aria-busy={packing}>
              <label className="space-y-1 text-[11px] text-muted-foreground">Island padding<input aria-label="UV island padding" type="number" min="0" max="0.249" step="0.005" inputMode="decimal" className="h-10 w-24 rounded border bg-input px-2 text-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring" value={packPadding} onChange={(event) => setPackPadding(event.target.value)} /></label>
              <Button variant="outline" className="min-h-10" disabled={!islands.length || packing || !studio.pageId} onClick={() => void packIslands()}>{packing ? "Packing…" : "Pack islands"}</Button>
              {packMessage && <p role="status" className="text-xs text-muted-foreground">{packMessage}</p>}
              {packError && <p role="alert" className="w-full text-xs text-destructive">{packError}</p>}
            </section>
              <form className="flex flex-wrap items-end gap-2" onSubmit={(event) => { event.preventDefault(); applyProjection(); }}>
              <label className="min-w-36 flex-1 space-y-1 text-[11px] text-muted-foreground">Automatic projection<select aria-label="Automatic UV projection" className="h-8 w-full rounded border bg-input px-2 text-xs text-foreground" value={projection} onChange={(event) => setProjection(event.target.value)}><option value="planar">Planar</option><option value="box">Box</option><option value="cylindrical">Cylindrical</option><option value="spherical">Spherical</option></select></label>
              <Button type="submit" variant="outline" className="min-h-9" disabled={!digest}>Apply projection</Button>
            </form>
            {current.uv_sets > 0 && <>
              <form className="space-y-2 rounded border bg-card p-2" onSubmit={addIsland}>
                <h3 className="text-[11px] font-semibold uppercase tracking-wide text-muted-foreground">Create island</h3>
                <label className="block space-y-1 text-[11px] text-muted-foreground">Island ID<input className="h-8 w-full rounded border bg-input px-2 text-xs text-foreground" spellCheck={false} value={newId} onChange={(event) => setNewId(event.target.value)} /></label>
                <label className="block space-y-1 text-[11px] text-muted-foreground">Triangle indices<input className="h-8 w-full rounded border bg-input px-2 text-xs text-foreground" inputMode="numeric" placeholder="e.g. 0,1,2" value={triangleText} onChange={(event) => setTriangleText(event.target.value)} aria-describedby={formError ? "uv-island-error" : undefined} aria-invalid={formError ? true : undefined} /></label>
                {formError && <p id="uv-island-error" role="alert" className="text-xs text-destructive">{formError}</p>}
                <Button type="submit" variant="default" className="min-h-9" disabled={!digest}>Assign triangles</Button>
              </form>
              <section className="space-y-2" aria-label="UV islands">
                <h3 className="text-[11px] font-semibold uppercase tracking-wide text-muted-foreground">Islands · {islands.length}</h3>
                {!islands.length ? <Empty>Island selection is empty. Assign triangle indices above to create your first editable region.</Empty> : <div className="flex flex-wrap gap-2">{islands.map((island) => <Button key={island.id} variant={island.id === islandId ? "default" : "outline"} aria-pressed={island.id === islandId} className="min-h-9" onClick={() => { setIslandId(island.id); setGraphicId(""); }}>{island.id} · {island.triangle_count} tri{island.mirrored ? " · flipped" : ""}</Button>)}</div>}
                {selectedIsland && <div className="space-y-2 rounded border bg-card p-2">
                  <p className="truncate text-xs font-medium">Transform · {selectedIsland.id}</p>
                  <p className="text-xs text-muted-foreground">{selectedIsland.texel_density_px_per_model_unit == null ? "Texel density unavailable for this island." : `${selectedIsland.texel_density_px_per_model_unit.toLocaleString(undefined, { maximumFractionDigits: 1 })} px / model unit at ${selectedIsland.texture_resolution_px[0]}²`}</p>
                  {selectedIsland.below_minimum_texel_density && <p role="status" className="text-xs text-warning">Below your minimum density. Increase texture resolution or give this island more UV area.</p>}
                  {current.uv_sets > 1 && <Row label="UV set"><select aria-label="UV set for island" className="h-7 min-w-0 flex-1 rounded border bg-input px-2 text-xs" value={selectedIsland.uv_set} onChange={(event) => editIsland((item) => { item.uv_set = Number(event.target.value); }, "Change UV island set")}>{Array.from({ length: current.uv_sets }, (_, index) => <option key={index} value={index}>Set {index}</option>)}</select></Row>}
                  <Row label="Scale U"><NumberField value={selectedIsland.transform.scale[0]} min={-16} max={16} step={0.05} onCommit={(value) => editIsland((item) => { item.transform.scale[0] = value || 0.01; }, "Scale UV island")} ariaLabel="UV island horizontal scale" /></Row>
                  <Row label="Scale V"><NumberField value={selectedIsland.transform.scale[1]} min={-16} max={16} step={0.05} onCommit={(value) => editIsland((item) => { item.transform.scale[1] = value || 0.01; }, "Scale UV island")} ariaLabel="UV island vertical scale" /></Row>
                  <Row label="Offset U"><NumberField value={selectedIsland.transform.offset[0]} min={-4} max={4} step={0.01} onCommit={(value) => editIsland((item) => { item.transform.offset[0] = value; }, "Move UV island")} ariaLabel="UV island horizontal offset" /></Row>
                  <Row label="Offset V"><NumberField value={selectedIsland.transform.offset[1]} min={-4} max={4} step={0.01} onCommit={(value) => editIsland((item) => { item.transform.offset[1] = value; }, "Move UV island")} ariaLabel="UV island vertical offset" /></Row>
                  <Row label="Rotation"><NumberField value={selectedIsland.transform.rotate} min={-360} max={360} onCommit={(value) => editIsland((item) => { item.transform.rotate = value; }, "Rotate UV island")} ariaLabel="UV island rotation degrees" /></Row>
                  <div className="flex flex-wrap gap-2"><Button variant="outline" className="min-h-9" onClick={() => editIsland((item) => { item.transform.scale[0] *= -1; }, "Flip UV island horizontally")}>Flip U</Button><Button variant="outline" className="min-h-9" onClick={() => editIsland((item) => { item.transform.scale[1] *= -1; }, "Flip UV island vertically")}>Flip V</Button><Button variant="destructive" className="min-h-9" onClick={removeIsland}>Remove island</Button></div>
                </div>}
              </section>
            </>}
          </div>
        </div> : <div className="flex flex-1 items-center justify-center p-6"><Empty>This model has no renderable primitives.</Empty></div>}
    </section>
  </div>;
}
