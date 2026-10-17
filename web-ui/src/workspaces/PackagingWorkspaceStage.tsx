import { useEffect, useMemo, useRef, useState } from "react";
import { AlertCircle, Download, Pencil, RotateCw } from "lucide-react";
import { api } from "@/lib/api";
import type { Doc, SceneNode } from "@/lib/api";
import type { PointerEvent as ReactPointerEvent } from "react";
import { ARTWORK_SOURCE_DRAG_TYPE, parseArtworkSourceDrag } from "@/lib/artworkDrag";
import { children, findNode, layerNodes, pageOf } from "@/lib/model";
import { useStudio } from "@/store";
import { Button } from "@/components/ui/button";
import { Empty, NumberField } from "@/components/ui/kit";
import { Canvas } from "@/components/Canvas";

type PanelArtworkTransform = { scale: [number, number]; offset: [number, number]; rotate: number };
const defaultPanelArtworkTransform: PanelArtworkTransform = { scale: [1, 1], offset: [0.5, 0.5], rotate: 0 };
type FlatTransformGesture = { mode: "resize" | "rotate"; panel: Panel; pointerId: number; target: HTMLButtonElement; rect: DOMRect; start: [number, number]; center: [number, number]; startAngle: number; transform: PanelArtworkTransform; draft: PanelArtworkTransform; moved: boolean };
type Panel = { id: string; rect_mm: [number, number, number, number]; artwork?: { kind?: string; page?: string; node?: string }; artwork_transform?: PanelArtworkTransform; safe_zone_mm?: number };
type Dieline = { template?: string; panels?: Panel[]; fold_state?: number; dimensions_mm?: { bleed?: number } };
type FlatPreview = Awaited<ReturnType<typeof api.scene3dPackFlatPreview>>;
type PackPreflight = Awaited<ReturnType<typeof api.scene3dPackPreflight>>["result"];

// Keep these role swatches synchronized with Rust's `role_color` mapping in
// src/scene3d/pack.rs; they are the technical colors in the flat dieline SVG.
const productionLegend = [
  { label: "Cut line", color: "#dc2626" },
  { label: "Fold / crease", color: "#2563eb" },
  { label: "Perforation", color: "#7c3aed" },
  { label: "Bleed guide", color: "#f97316", dashed: true },
  { label: "Safe zone · not cut", color: "#16a34a", dashed: true },
  { label: "Glue flap", color: "#0891b2" },
];

function artworkNodes(doc: Doc): { page: string; node: SceneNode }[] {
  const found: { page: string; node: SceneNode }[] = [];
  const visit = (page: string, nodes: SceneNode[]) => nodes.forEach((node) => {
    found.push({ page, node });
    visit(page, children(node));
  });
  (doc.pages ?? []).forEach((page) => page.layers?.forEach((layer) => visit(page.id, layerNodes(layer))));
  return found;
}

function SvgPreview({ svg, alt }: { svg: string; alt: string }) {
  const [url, setUrl] = useState<string | null>(null);
  useEffect(() => {
    const next = URL.createObjectURL(new Blob([svg], { type: "image/svg+xml" }));
    setUrl(next);
    return () => URL.revokeObjectURL(next);
  }, [svg]);
  return url
    ? <img className="pointer-events-none absolute inset-0 h-full w-full object-fill" src={url} alt={alt} />
    : <div role="status" className="absolute inset-4 animate-pulse rounded border bg-muted motion-reduce:animate-none">Preparing flat dieline…</div>;
}

function flatSvgSizeMm(svg: string): [number, number] | null {
  const root = svg.match(/<svg\b[^>]*>/)?.[0];
  if (!root) return null;
  const read = (name: "width" | "height") => {
    const match = root.match(new RegExp(`\\b${name}="([0-9.eE+-]+)mm"`));
    const value = match ? Number(match[1]) : NaN;
    return Number.isFinite(value) && value > 0 ? value : null;
  };
  const width = read("width");
  const height = read("height");
  return width && height ? [width, height] : null;
}

/** Paired packaging views: the folded document canvas and Rust-rendered flat net. */
export function PackagingWorkspaceStage({ active = true }: { active?: boolean } = {}) {
  const studio = useStudio();
  const request = useRef(0);
  const selected = useMemo(() => studio.doc
    ? findNode(pageOf(studio.doc, studio.pageId), studio.selectedId)?.node ?? null
    : null, [studio.doc, studio.pageId, studio.selectedId]);
  const node = selected?.kind === "scene3d" ? selected : null;
  const dielineId = typeof node?.dieline === "string" ? node.dieline : null;
  const dielines = (studio.doc?.dielines ?? {}) as Record<string, Dieline>;
  const dieline = dielineId ? dielines[dielineId] : undefined;
  const previewDieline = dielineId
    ? (studio.preview?.dielines as Record<string, Dieline> | undefined)?.[dielineId]
    : undefined;
  const panels = dieline?.panels ?? [];
  const selectedPanel = studio.selectedPackagePanel;
  const foldedPercent = Math.round(((previewDieline ?? dieline)?.fold_state ?? 1) * 100);
  const [preview, setPreview] = useState<FlatPreview | null>(null);
  const [preflight, setPreflight] = useState<PackPreflight | null>(null);
  const [preflightError, setPreflightError] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);
  const [exporting, setExporting] = useState(false);
  const [printPpi, setPrintPpi] = useState(300);
  const [exportMessage, setExportMessage] = useState<{ text: string; error?: boolean } | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [dragOffset, setDragOffset] = useState<[number, number] | null>(null);
  const [placing, setPlacing] = useState(false);
  const [linkingPanel, setLinkingPanel] = useState(false);
  const [dragOverPanel, setDragOverPanel] = useState<string | null>(null);
  const [panelArtworkMessage, setPanelArtworkMessage] = useState<{ text: string; error?: boolean } | null>(null);
  const [transformDraft, setTransformDraft] = useState<{ panel: string; value: PanelArtworkTransform } | null>(null);
  const transformGesture = useRef<FlatTransformGesture | null>(null);
  const dragMoved = useRef(false);

  useEffect(() => {
    if (!active) return;
    if (!studio.doc || !dielineId) {
      request.current += 1;
      setPreview(null);
      setPreflight(null);
      setPreflightError(null);
      setLoading(false);
      setError(null);
      return;
    }
    const requestId = ++request.current;
    setPreview(null);
    setPreflight(null);
    setPreflightError(null);
    setLoading(true);
    setError(null);
    void api.scene3dPackPreflight({ document: studio.doc, dieline: dielineId, min_ppi: printPpi })
      .then((result) => { if (requestId === request.current) setPreflight(result.result); })
      .catch((cause) => {
        if (requestId === request.current) setPreflightError(cause instanceof Error ? cause.message : "Packaging preflight could not be completed.");
      });
    void api.scene3dPackFlatPreview({ document: studio.doc, dieline: dielineId, min_ppi: printPpi })
      .then((result) => {
        if (requestId === request.current) setPreview(result);
      })
      .catch((cause) => {
        if (requestId === request.current) {
          setPreview(null);
          setError(cause instanceof Error ? cause.message : "The flat dieline could not be rendered.");
        }
      })
      .finally(() => {
        if (requestId === request.current) setLoading(false);
      });
    return () => {
      if (request.current === requestId) request.current += 1;
    };
  }, [studio.doc, dielineId, printPpi, active]);

  useEffect(() => {
    if (!active) return;
    if (!dielineId || !panels.length) {
      if (selectedPanel !== null) studio.setSelectedPackagePanel(null);
      return;
    }
    if (!panels.some((panel) => panel.id === selectedPanel)) {
      studio.setSelectedPackagePanel(panels[0]?.id ?? null);
    }
  }, [dielineId, panels, selectedPanel, studio.setSelectedPackagePanel, active]);

  const bounds = panels.reduce((result, panel) => ({
    width: Math.max(result.width, panel.rect_mm[0] + panel.rect_mm[2]),
    height: Math.max(result.height, panel.rect_mm[1] + panel.rect_mm[3]),
  }), { width: 1, height: 1 });
  const bleedMm = Math.max(0, dieline?.dimensions_mm?.bleed ?? 0);
  const flatSize = preview ? flatSvgSizeMm(preview.svg) : null;
  const flatWidth = flatSize?.[0] ?? bounds.width + 2 * bleedMm;
  const flatHeight = flatSize?.[1] ?? bounds.height + 2 * bleedMm;
  const panelIssues = (panel: Panel) => {
    const issues: string[] = [];
    for (const check of preflight?.fold_hinge_checks ?? []) {
      if (!check.joined && check.panels.includes(panel.id)) issues.push(`${check.kind === "wrap-seam" ? "Wrap seam" : "Fold hinge"} gap ${check.gap_mm.toFixed(4)} mm`);
    }
    if (preflight?.panels_without_artwork.includes(panel.id)) issues.push("No linked artwork");
    const safe = preflight?.safe_zone_checks.find((check) => check.panel === panel.id);
    if (safe && !safe.within_safe_zone) issues.push(safe.inset_mm == null
      ? "No safe-area inset configured"
      : `Artwork extends outside the ${safe.inset_mm} mm safe area`);
    const seam = preflight?.wrap_seam_checks.find((check) => check.panel === panel.id);
    if (seam?.touches_seam) issues.push(`Artwork touches the ${seam.seam_angle_deg}° wrap seam`);
    const resolution = preflight?.rasterized_ppi.find((item) => item.panel === panel.id);
    if (preflight && resolution && resolution.effective_ppi < preflight.minimum_ppi) issues.push(`Raster art is ${resolution.effective_ppi.toFixed(1)} ppi; target is ${preflight.minimum_ppi} ppi`);
    return issues;
  };
  const retry = () => {
    if (studio.doc && dielineId && !loading) {
      setPreview(null);
      setPreflight(null);
      setPreflightError(null);
      setError(null);
      setLoading(true);
      const requestId = ++request.current;
      void api.scene3dPackPreflight({ document: studio.doc, dieline: dielineId, min_ppi: printPpi })
        .then((result) => { if (requestId === request.current) setPreflight(result.result); })
        .catch((cause) => {
          if (requestId === request.current) setPreflightError(cause instanceof Error ? cause.message : "Packaging preflight could not be completed.");
        });
      void api.scene3dPackFlatPreview({ document: studio.doc, dieline: dielineId, min_ppi: printPpi })
        .then((result) => { if (requestId === request.current) setPreview(result); })
        .catch((cause) => { if (requestId === request.current) setError(cause instanceof Error ? cause.message : "The flat dieline could not be rendered."); })
        .finally(() => { if (requestId === request.current) setLoading(false); });
    }
  };

  const exportPrintBundle = async () => {
    if (!studio.doc || !dielineId || exporting) return;
    setExporting(true);
    setExportMessage(null);
    try {
      await api.scene3dPackExportBundle({
        document: studio.doc,
        dieline: dielineId,
        min_ppi: printPpi,
        ppi: printPpi,
        fold: (previewDieline ?? dieline)?.fold_state ?? 1,
      });
      setExportMessage({ text: "Print bundle downloaded. Use the included guide to send artwork and cut/fold plates to production." });
    } catch (cause) {
      setExportMessage({ text: cause instanceof Error ? cause.message : "The print bundle could not be exported. Try again.", error: true });
    } finally {
      setExporting(false);
    }
  };

  const updateArtworkPosition = async (panel: Panel, offset: [number, number]) => {
    const doc = useStudio.getState().doc;
    if (!doc || !dielineId || placing) return;
    const transform = panel.artwork_transform ?? defaultPanelArtworkTransform;
    const bounded: [number, number] = [
      Math.max(transform.scale[0] / 2, Math.min(1 - transform.scale[0] / 2, offset[0])),
      Math.max(transform.scale[1] / 2, Math.min(1 - transform.scale[1] / 2, offset[1])),
    ];
    setPlacing(true);
    setError(null);
    try {
      const result = await api.scene3dPanelArtTransform({ document: doc, dieline: dielineId, panel: panel.id, ...transform, offset: bounded });
      studio.replaceDoc(result.document, "Move packaging panel artwork");
      setDragOffset(null);
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : "Panel artwork could not be moved.");
      setDragOffset(null);
    } finally {
      setPlacing(false);
    }
  };

  const setPanelArtwork = async (panel: Panel, value: string) => {
    const doc = useStudio.getState().doc;
    if (!doc || !dielineId || linkingPanel) return;
    let source: { kind: "page" | "node"; page: string; node?: string } | undefined;
    if (value.startsWith("page:")) source = { kind: "page", page: value.slice(5) };
    else if (value.startsWith("node:")) {
      const found = artworkNodes(doc).find(({ page, node: item }) => `node:${page}:${item.id}` === value);
      if (found) source = { kind: "node", page: found.page, node: found.node.id };
    }
    setLinkingPanel(true);
    setPanelArtworkMessage(null);
    try {
      const result = await api.scene3dPanelArt({ document: doc, dieline: dielineId, panel: panel.id, source });
      studio.replaceDoc(result.document, "Link packaging panel artwork");
      setPanelArtworkMessage({ text: source ? `Artwork linked to panel ${panel.id}.` : `Artwork removed from panel ${panel.id}.` });
    } catch (cause) {
      setPanelArtworkMessage({ text: cause instanceof Error ? cause.message : "Panel artwork could not be linked.", error: true });
    } finally {
      setLinkingPanel(false);
    }
  };

  const setPanelSafeZone = async (panel: Panel, inset: number) => {
    const doc = useStudio.getState().doc;
    if (!doc || !dielineId || placing || linkingPanel || selectedNodeLocked) return;
    setPlacing(true);
    setPanelArtworkMessage(null);
    try {
      const result = await api.scene3dPanelSafeZone({ document: doc, dieline: dielineId, panel: panel.id, inset_mm: inset });
      studio.replaceDoc(result.document, "Set packaging panel safe area");
      setPanelArtworkMessage({ text: `Safe-area inset updated for panel ${panel.id}.` });
    } catch (cause) {
      setPanelArtworkMessage({ text: cause instanceof Error ? cause.message : "Panel safe area could not be updated.", error: true });
    } finally {
      setPlacing(false);
    }
  };

  const editPanelArtwork = (panel: Panel) => {
    const source = panel.artwork;
    if (!source?.page) return;
    const state = useStudio.getState();
    state.setWorkspace("canvas");
    state.setPage(source.page);
    if (source.kind === "node" && source.node) state.select(source.node);
  };
  const selectedPanelRecord = panels.find((panel) => panel.id === selectedPanel);
  const selectedNodeLocked = !!(studio.doc && node && findNode(pageOf(studio.doc, studio.pageId), node.id)?.layer.locked);

  const updateArtworkTransform = async (panel: Panel, transform: PanelArtworkTransform, label: string) => {
    const doc = useStudio.getState().doc;
    if (!doc || !dielineId || placing || selectedNodeLocked) return;
    setPlacing(true);
    setError(null);
    try {
      const result = await api.scene3dPanelArtTransform({ document: doc, dieline: dielineId, panel: panel.id, ...transform });
      studio.replaceDoc(result.document, label);
      setDragOffset(null);
      setPanelArtworkMessage({ text: `Placement updated for panel ${panel.id}.` });
    } catch (cause) {
      setPanelArtworkMessage({ text: cause instanceof Error ? cause.message : "Panel artwork transform could not be saved.", error: true });
    } finally {
      setPlacing(false);
    }
  };

  const adjustSelectedArtworkScale = (delta: number) => {
    if (!selectedPanelRecord?.artwork) return;
    const current = selectedPanelRecord.artwork_transform ?? defaultPanelArtworkTransform;
    const scale: [number, number] = [
      Math.max(0.01, Math.min(1, current.scale[0] + delta)),
      Math.max(0.01, Math.min(1, current.scale[1] + delta)),
    ];
    void updateArtworkTransform(selectedPanelRecord, {
      ...current,
      scale,
      offset: [
        Math.max(scale[0] / 2, Math.min(1 - scale[0] / 2, current.offset[0])),
        Math.max(scale[1] / 2, Math.min(1 - scale[1] / 2, current.offset[1])),
      ],
    }, "Scale packaging panel artwork");
  };

  const rotateSelectedArtwork = (delta: number) => {
    if (!selectedPanelRecord?.artwork) return;
    const current = selectedPanelRecord.artwork_transform ?? defaultPanelArtworkTransform;
    void updateArtworkTransform(selectedPanelRecord, {
      ...current,
      rotate: Math.max(-360, Math.min(360, current.rotate + delta)),
    }, "Rotate packaging panel artwork");
  };

  const editSelectedArtworkTransform = (patch: Partial<PanelArtworkTransform>, label: string) => {
    if (!selectedPanelRecord?.artwork) return;
    const current = selectedPanelRecord.artwork_transform ?? defaultPanelArtworkTransform;
    void updateArtworkTransform(selectedPanelRecord, { ...current, ...patch }, label);
  };

  const beginFlatTransform = (mode: FlatTransformGesture["mode"], panel: Panel, transform: PanelArtworkTransform, event: ReactPointerEvent<HTMLButtonElement>) => {
    if (placing || linkingPanel || selectedNodeLocked) return;
    event.preventDefault();
    event.stopPropagation();
    const region = event.currentTarget.parentElement?.querySelector<HTMLButtonElement>("button[data-flat-panel]");
    const rect = region?.getBoundingClientRect();
    if (!rect || rect.width <= 0 || rect.height <= 0) return;
    event.currentTarget.setPointerCapture(event.pointerId);
    studio.setSelectedPackagePanel(panel.id);
    const center: [number, number] = [rect.left + transform.offset[0] * rect.width, rect.top + transform.offset[1] * rect.height];
    transformGesture.current = {
      mode, panel, pointerId: event.pointerId, target: event.currentTarget, rect,
      start: [event.clientX, event.clientY], center,
      startAngle: Math.atan2(event.clientY - center[1], event.clientX - center[0]),
      transform: { ...transform, scale: [...transform.scale], offset: [...transform.offset] },
      draft: { ...transform, scale: [...transform.scale], offset: [...transform.offset] }, moved: false,
    };
    setTransformDraft({ panel: panel.id, value: transform });
  };

  const moveFlatTransform = (event: ReactPointerEvent<HTMLButtonElement>) => {
    const gesture = transformGesture.current;
    if (!gesture || gesture.pointerId !== event.pointerId) return;
    event.preventDefault();
    event.stopPropagation();
    const dx = event.clientX - gesture.start[0];
    const dy = event.clientY - gesture.start[1];
    if (dx * dx + dy * dy >= 9) gesture.moved = true;
    let value = gesture.draft;
    if (gesture.mode === "resize") {
      const angle = gesture.transform.rotate * Math.PI / 180;
      const localX = (dx * Math.cos(angle) + dy * Math.sin(angle)) / gesture.rect.width;
      const localY = (-dx * Math.sin(angle) + dy * Math.cos(angle)) / gesture.rect.height;
      const scale: [number, number] = [
        Math.max(0.01, Math.min(1, gesture.transform.scale[0] + localX * 2)),
        Math.max(0.01, Math.min(1, gesture.transform.scale[1] + localY * 2)),
      ];
      value = {
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
      value = { ...gesture.transform, rotate: Math.max(-360, Math.min(360, gesture.transform.rotate + delta * 180 / Math.PI)) };
    }
    gesture.draft = value;
    setTransformDraft({ panel: gesture.panel.id, value });
  };

  const finishFlatTransform = (event: ReactPointerEvent<HTMLButtonElement>, commit: boolean) => {
    const gesture = transformGesture.current;
    if (!gesture || gesture.pointerId !== event.pointerId) return;
    event.preventDefault();
    event.stopPropagation();
    if (event.currentTarget.hasPointerCapture(event.pointerId)) event.currentTarget.releasePointerCapture(event.pointerId);
    transformGesture.current = null;
    setTransformDraft(null);
    if (commit && gesture.moved) {
      void updateArtworkTransform(gesture.panel, gesture.draft, gesture.mode === "resize" ? "Resize packaging panel artwork" : "Rotate packaging panel artwork");
    }
  };

  const keyboardFlatTransform = (mode: FlatTransformGesture["mode"], panel: Panel, transform: PanelArtworkTransform, event: React.KeyboardEvent<HTMLButtonElement>) => {
    const step = event.shiftKey ? 0.05 : 0.01;
    if (mode === "resize") {
      if (!["ArrowLeft", "ArrowRight", "ArrowUp", "ArrowDown"].includes(event.key)) return;
      event.preventDefault();
      const dx = event.key === "ArrowLeft" ? -step : event.key === "ArrowRight" ? step : 0;
      const dy = event.key === "ArrowUp" ? -step : event.key === "ArrowDown" ? step : 0;
      const scale: [number, number] = [Math.max(0.01, Math.min(1, transform.scale[0] + dx)), Math.max(0.01, Math.min(1, transform.scale[1] + dy))];
      void updateArtworkTransform(panel, { ...transform, scale, offset: [Math.max(scale[0] / 2, Math.min(1 - scale[0] / 2, transform.offset[0])), Math.max(scale[1] / 2, Math.min(1 - scale[1] / 2, transform.offset[1]))] }, "Resize packaging panel artwork");
    } else {
      if (event.key !== "ArrowLeft" && event.key !== "ArrowRight") return;
      event.preventDefault();
      const delta = event.key === "ArrowLeft" ? -1 : 1;
      void updateArtworkTransform(panel, { ...transform, rotate: Math.max(-360, Math.min(360, transform.rotate + delta * (event.shiftKey ? 15 : 1))) }, "Rotate packaging panel artwork");
    }
  };

  if (!node || !dielineId || !dieline) {
    return <div className="flex min-h-0 min-w-0 flex-1 items-center justify-center p-6">
      <Empty>Select a dieline-backed package on the page to open its linked Flat / Folded workspace.</Empty>
    </div>;
  }

  return <div className="flex min-h-0 min-w-0 flex-1 flex-col overflow-y-auto lg:flex-row lg:overflow-hidden">
    <section aria-label={dieline.template === "tube" ? "Wrapped package view" : "Folded package view"} className="flex min-h-[320px] min-w-0 flex-none flex-col border-b lg:min-h-0 lg:flex-1 lg:border-b-0 lg:border-r">
      <header className="flex h-10 shrink-0 items-center gap-2 border-b bg-card px-3 text-xs">
        <span className="font-medium">{dieline.template === "tube" ? "Wrapped" : "Folded"}</span>
        <span className="min-w-0 flex-1 truncate text-muted-foreground">{node.name || node.id}{dieline.template === "tube" ? " · cylindrical label preview · click a face to select its panel" : ` · ${foldedPercent}% folded · click a face to select its panel`}</span>
        {selectedPanel && <span className="max-w-36 truncate rounded border bg-muted px-2 py-1" aria-live="polite">Face · {selectedPanel}</span>}
      </header>
      <div className="flex min-h-0 min-w-0 flex-1 flex-col"><Canvas active={active} /></div>
    </section>
    <section aria-label="Flat dieline view" aria-busy={loading} className="flex min-h-[360px] min-w-0 flex-none flex-col lg:min-h-0 lg:flex-1">
      <header className="flex h-10 shrink-0 items-center gap-2 border-b bg-card px-3 text-xs">
        <span className="font-medium">Flat dieline</span>
        <span className="min-w-0 flex-1 truncate text-muted-foreground">{dielineId} · panel geometry and linked artwork</span>
        <Button type="button" variant="outline" className="min-h-10" aria-label="Export packaging print bundle" disabled={exporting} aria-busy={exporting} onClick={() => void exportPrintBundle()}>
          <Download size={14} aria-hidden="true" /> {exporting ? "Preparing…" : "Export print bundle"}
        </Button>
        <Button type="button" variant="outline" className="min-h-10" aria-label="Refresh flat dieline" disabled={loading} onClick={retry}>
          <RotateCw size={14} aria-hidden="true" /> Refresh
        </Button>
      </header>
      <div className="space-y-1.5 border-b px-3 py-2">
        <div className="flex flex-wrap items-center gap-x-2 gap-y-1 text-[11px]">
          <label className="flex items-center gap-1.5 text-muted-foreground">
            <span>Print raster PPI</span>
            <NumberField className="h-10 min-h-10 w-20 flex-none rounded border border-input bg-background px-2 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2" value={printPpi} min={72} max={1200} step={50} disabled={exporting} onCommit={(value) => setPrintPpi(Math.round(value))} ariaLabel="Packaging print resolution in pixels per inch" />
          </label>
          <span className="text-muted-foreground">Used by preview, preflight, and export. Export supports up to 1200 PPI; vector artwork stays vector.</span>
        </div>
        <ul aria-label="Dieline mark legend" className="flex flex-wrap gap-x-4 gap-y-1.5 text-[11px] text-foreground">
          {productionLegend.map((item) => <li key={item.label} className="inline-flex items-center gap-1.5">
            <span
              aria-hidden="true"
              className={`inline-block h-0.5 w-4 shrink-0 ${item.dashed ? "border-t-2 border-dashed bg-transparent" : ""}`}
              style={item.dashed ? { borderColor: item.color } : { backgroundColor: item.color }}
            />
            <span>{item.label}</span>
          </li>)}
        </ul>
        <ol aria-label="Packaging print and finishing steps" className="grid gap-2 text-[11px] text-muted-foreground sm:grid-cols-3">
          <li className="flex gap-2 rounded border bg-muted/30 p-2">
            <span aria-hidden="true" className="flex size-5 shrink-0 items-center justify-center rounded-full border text-[10px] font-semibold text-foreground">1</span>
            <span><strong className="font-medium text-foreground">Review the folded mockup.</strong> Check the assembled look and panel artwork. The PNG is a preview, not print output.</span>
          </li>
          <li className="flex gap-2 rounded border bg-muted/30 p-2">
            <span aria-hidden="true" className="flex size-5 shrink-0 items-center justify-center rounded-full border text-[10px] font-semibold text-foreground">2</span>
            <span><strong className="font-medium text-foreground">Print the artwork PDF at 100%.</strong> Do not fit to page. Confirm stock and bleed with your printer.</span>
          </li>
          <li className="flex gap-2 rounded border bg-muted/30 p-2">
            <span aria-hidden="true" className="flex size-5 shrink-0 items-center justify-center rounded-full border text-[10px] font-semibold text-foreground">3</span>
            <span><strong className="font-medium text-foreground">Send vector spot plates to finishing.</strong> Red cuts through; blue scores/folds; purple perforates. The PDF is not a cutter file—confirm tools and settings with your converter.</span>
          </li>
        </ol>
        <p className="text-[11px] text-muted-foreground">Templates are conceptual and are not manufacturing-certified. Verify dimensions, material, and assembly with your packaging supplier before production.</p>
        {preflight?.pdfx_output_role && !preflight.pdfx_output_role.configured && <div role="note" aria-label="PDF/X output profile required" className="flex items-start gap-2 rounded border border-warning/40 bg-warning/5 p-2 text-xs text-foreground">
          <AlertCircle size={15} aria-hidden="true" className="mt-0.5 shrink-0 text-warning" />
          <span>{preflight.pdfx_output_role.message ?? "Print bundle PDF/X needs an ICC CMYK output role."} Import a licensed profile and assign the document’s output role with <code>pentool color profile import</code> and <code>pentool color role assign … output</code> before exporting.</span>
        </div>}
        <div className="flex flex-wrap items-center gap-x-2 gap-y-1 text-[11px] text-muted-foreground" aria-live="polite">
          {exportMessage && <span role={exportMessage.error ? "alert" : "status"} className={exportMessage.error ? "text-destructive" : "text-foreground"}>{exportMessage.text}</span>}
        </div>
      </div>
      {loading && !preview && <div role="status" className="m-4 min-h-64 flex-1 animate-pulse rounded border bg-muted motion-reduce:animate-none">Rendering linked artwork and dieline…</div>}
      {error && <div role="alert" className="m-4 flex flex-1 flex-col items-start justify-center gap-3 rounded border border-destructive/30 bg-destructive/5 p-4 text-sm">
        <div className="flex items-center gap-2 text-destructive"><AlertCircle size={16} aria-hidden="true" /><p className="font-medium">Flat preview failed</p></div>
        <p className="break-words text-muted-foreground">{error}</p>
        <Button type="button" variant="outline" className="min-h-10" disabled={loading} onClick={retry}>Try again</Button>
      </div>}
      {!loading && !error && !preview && <div className="flex flex-1 items-center justify-center p-6"><Empty>The flat view will appear when Rust finishes rendering the package net.</Empty></div>}
      {preview && <div className="flex min-h-0 flex-1 flex-col overflow-auto p-3">
        <div className="mx-auto w-full max-w-5xl border bg-card" style={{ aspectRatio: `${flatWidth} / ${flatHeight}` }}>
          <div className="relative size-full">
            <SvgPreview svg={preview.svg} alt={`${dielineId} flat dieline with linked panel artwork`} />
            {panels.map((panel) => {
              const transform = panel.artwork_transform ?? defaultPanelArtworkTransform;
              const isSelected = selectedPanel === panel.id;
              const shownTransform = transformDraft?.panel === panel.id ? transformDraft.value : transform;
              const offset = isSelected && dragOffset ? dragOffset : shownTransform.offset;
              const issues = panelIssues(panel);
              const angle = shownTransform.rotate * Math.PI / 180;
              const aspect = panel.rect_mm[3] / panel.rect_mm[2];
              const resizeHandle: [number, number] = [
                offset[0] + Math.cos(angle) * shownTransform.scale[0] / 2 - Math.sin(angle) * shownTransform.scale[1] / 2 * aspect,
                offset[1] + Math.sin(angle) * shownTransform.scale[0] / 2 / aspect + Math.cos(angle) * shownTransform.scale[1] / 2,
              ];
              const rotateReach = shownTransform.scale[1] / 2 + 0.12;
              const rotateHandle: [number, number] = [offset[0] - Math.sin(angle) * rotateReach * aspect, offset[1] + Math.cos(angle) * rotateReach];
              return <div
              key={panel.id}
              className="absolute"
              style={{ left: `${(panel.rect_mm[0] + bleedMm) / flatWidth * 100}%`, top: `${(panel.rect_mm[1] + bleedMm) / flatHeight * 100}%`, width: `${panel.rect_mm[2] / flatWidth * 100}%`, height: `${panel.rect_mm[3] / flatHeight * 100}%` }}
              >
              <button
              type="button"
              data-flat-panel={panel.id}
              aria-label={`Select flat panel ${panel.id}; drop a page or drawable node to link artwork${panel.artwork ? "; drag to reposition linked artwork" : ""}${issues.length ? `; preflight warnings: ${issues.join("; ")}` : ""}`}
              aria-pressed={isSelected}
              title={issues.length ? `${panel.id}: ${issues.join(" · ")} · Drop a page or drawable node to link artwork` : `Panel ${panel.id} · Drop a page or drawable node to link artwork`}
              className={`absolute inset-0 min-h-10 min-w-10 touch-none border-2 focus-visible:z-10 focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2 ${dragOverPanel === panel.id ? "z-10 border-primary bg-primary/20 ring-2 ring-ring" : isSelected ? "border-primary bg-primary/10" : "border-transparent hover:border-primary/70 hover:bg-primary/5"}`}
              onClick={() => studio.setSelectedPackagePanel(panel.id)}
              onDragOver={(event) => {
                if (!event.dataTransfer.types.includes(ARTWORK_SOURCE_DRAG_TYPE)) return;
                event.preventDefault();
                event.stopPropagation();
                event.dataTransfer.dropEffect = "copy";
                setDragOverPanel(panel.id);
              }}
              onDragLeave={(event) => {
                const nextTarget = event.relatedTarget;
                if (!(nextTarget instanceof globalThis.Node) || !event.currentTarget.contains(nextTarget)) setDragOverPanel(null);
              }}
              onDrop={(event) => {
                if (!event.dataTransfer.types.includes(ARTWORK_SOURCE_DRAG_TYPE)) return;
                event.preventDefault();
                event.stopPropagation();
                setDragOverPanel(null);
                studio.setSelectedPackagePanel(panel.id);
                const source = parseArtworkSourceDrag(event.dataTransfer.getData(ARTWORK_SOURCE_DRAG_TYPE));
                const doc = useStudio.getState().doc;
                const pageExists = !!doc?.pages?.some((page) => page.id === source?.page);
                const nodeExists = source?.kind !== "node" || !!doc && artworkNodes(doc).some(({ page, node: item }) => page === source.page && item.id === source.node);
                if (!source || !pageExists || !nodeExists) {
                  setPanelArtworkMessage({ text: "That page or node is no longer available. Choose an artwork source from the picker.", error: true });
                  return;
                }
                const value = source.kind === "page" ? `page:${source.page}` : `node:${source.page}:${source.node}`;
                void setPanelArtwork(panel, value);
              }}
              onKeyDown={(event) => {
                if (!["ArrowLeft", "ArrowRight", "ArrowUp", "ArrowDown", "Home", "End"].includes(event.key)) return;
                const buttons = Array.from(event.currentTarget.closest(".relative")?.querySelectorAll<HTMLButtonElement>("button[data-flat-panel]") ?? []);
                const index = buttons.indexOf(event.currentTarget);
                if (index < 0 || buttons.length < 2) return;
                event.preventDefault();
                event.stopPropagation();
                const nextIndex = event.key === "Home" ? 0
                  : event.key === "End" ? buttons.length - 1
                    : (index + (event.key === "ArrowLeft" || event.key === "ArrowUp" ? -1 : 1) + buttons.length) % buttons.length;
                const next = buttons[nextIndex];
                next.focus();
                const nextPanel = next.dataset.flatPanel;
                if (nextPanel) studio.setSelectedPackagePanel(nextPanel);
              }}
              onPointerDown={(event) => {
                studio.setSelectedPackagePanel(panel.id);
                if (!panel.artwork || placing) return;
                event.currentTarget.setPointerCapture(event.pointerId);
                dragMoved.current = false;
                setDragOffset(transform.offset);
              }}
              onPointerMove={(event) => {
                if (!panel.artwork || !event.currentTarget.hasPointerCapture(event.pointerId)) return;
                dragMoved.current = true;
                const rect = event.currentTarget.getBoundingClientRect();
                const x = (event.clientX - rect.left) / Math.max(1, rect.width);
                const y = (event.clientY - rect.top) / Math.max(1, rect.height);
                setDragOffset([
                  Math.max(transform.scale[0] / 2, Math.min(1 - transform.scale[0] / 2, x)),
                  Math.max(transform.scale[1] / 2, Math.min(1 - transform.scale[1] / 2, y)),
                ]);
              }}
              onPointerUp={(event) => {
                if (!panel.artwork || !event.currentTarget.hasPointerCapture(event.pointerId)) return;
                event.currentTarget.releasePointerCapture(event.pointerId);
                if (!dragMoved.current) {
                  setDragOffset(null);
                  return;
                }
                const rect = event.currentTarget.getBoundingClientRect();
                const x = (event.clientX - rect.left) / Math.max(1, rect.width);
                const y = (event.clientY - rect.top) / Math.max(1, rect.height);
                void updateArtworkPosition(panel, [x, y]);
              }}
              onPointerCancel={() => setDragOffset(null)}
            >{isSelected && panel.artwork && <span aria-hidden="true" className="pointer-events-none absolute border border-dashed border-primary bg-primary/15" style={{ left: `${(offset[0] - shownTransform.scale[0] / 2) * 100}%`, top: `${(offset[1] - shownTransform.scale[1] / 2) * 100}%`, width: `${shownTransform.scale[0] * 100}%`, height: `${shownTransform.scale[1] * 100}%`, transform: `rotate(${shownTransform.rotate}deg)` }} />}{dragOverPanel === panel.id && <span aria-hidden="true" className="pointer-events-none absolute inset-0 border-4 border-primary bg-primary/20" />}{issues.length > 0 && <span aria-hidden="true" className="pointer-events-none absolute right-1 top-1 grid size-6 place-items-center rounded-full border border-warning/50 bg-background/90 text-warning shadow-sm"><AlertCircle size={15} /></span>}</button>
              {isSelected && panel.artwork && <>
                <button type="button" disabled={placing || linkingPanel || selectedNodeLocked} aria-label={`Resize artwork on ${panel.id}; drag or use arrow keys`} title="Drag to resize · arrow keys for fine adjustment" className="absolute z-20 grid size-12 touch-none -translate-x-1/2 -translate-y-1/2 place-items-center rounded-full border-2 border-primary bg-card text-primary shadow-sm focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2" style={{ left: `${resizeHandle[0] * 100}%`, top: `${resizeHandle[1] * 100}%` }} onPointerDown={(event) => beginFlatTransform("resize", panel, shownTransform, event)} onPointerMove={moveFlatTransform} onPointerUp={(event) => finishFlatTransform(event, true)} onPointerCancel={(event) => finishFlatTransform(event, false)} onKeyDown={(event) => keyboardFlatTransform("resize", panel, shownTransform, event)}><span aria-hidden="true">↘</span></button>
                <button type="button" disabled={placing || linkingPanel || selectedNodeLocked} aria-label={`Rotate artwork on ${panel.id}; drag or use arrow keys`} title="Drag to rotate · left/right arrow keys for fine adjustment" className="absolute z-20 grid size-12 touch-none -translate-x-1/2 -translate-y-1/2 place-items-center rounded-full border-2 border-primary bg-card text-primary shadow-sm focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2" style={{ left: `${rotateHandle[0] * 100}%`, top: `${rotateHandle[1] * 100}%` }} onPointerDown={(event) => beginFlatTransform("rotate", panel, shownTransform, event)} onPointerMove={moveFlatTransform} onPointerUp={(event) => finishFlatTransform(event, true)} onPointerCancel={(event) => finishFlatTransform(event, false)} onKeyDown={(event) => keyboardFlatTransform("rotate", panel, shownTransform, event)}><span aria-hidden="true">↻</span></button>
              </>}
              </div>;
            })}
          </div>
        </div>
        <p className="mx-auto mt-2 w-full max-w-5xl text-xs text-muted-foreground">Select a panel to synchronize its folded face. Drag page or node artwork from Layers / Design onto a panel to link it. Drag inside linked art to position it; use Size / angle and Safe area to refine print placement.</p>
        {selectedPanel && <div className="mx-auto mt-3 flex w-full max-w-5xl flex-wrap items-center gap-2 rounded border bg-card p-2">
          <label htmlFor="flat-panel-art-source" className="text-xs font-medium">Artwork · {selectedPanel}</label>
          <div role="group" aria-label={`${selectedPanel} panel artwork transforms`} className="flex flex-wrap items-center gap-1">
            <span className="px-1 text-xs text-muted-foreground">Size / angle</span>
            <Button type="button" variant="outline" className="min-h-10" aria-label={`Shrink artwork on ${selectedPanel}`} disabled={!selectedPanelRecord?.artwork || placing || linkingPanel || selectedNodeLocked} onClick={() => adjustSelectedArtworkScale(-0.05)}>−</Button>
            <Button type="button" variant="outline" className="min-h-10" aria-label={`Enlarge artwork on ${selectedPanel}`} disabled={!selectedPanelRecord?.artwork || placing || linkingPanel || selectedNodeLocked} onClick={() => adjustSelectedArtworkScale(0.05)}>+</Button>
            <Button type="button" variant="outline" className="min-h-10" aria-label={`Rotate artwork on ${selectedPanel} counterclockwise`} disabled={!selectedPanelRecord?.artwork || placing || linkingPanel || selectedNodeLocked} onClick={() => rotateSelectedArtwork(-15)}>↶ 15°</Button>
            <Button type="button" variant="outline" className="min-h-10" aria-label={`Rotate artwork on ${selectedPanel} clockwise`} disabled={!selectedPanelRecord?.artwork || placing || linkingPanel || selectedNodeLocked} onClick={() => rotateSelectedArtwork(15)}>↷ 15°</Button>
          </div>
          <select id="flat-panel-art-source" aria-label={`${selectedPanel} panel artwork source`} className="min-h-10 min-w-48 flex-1 rounded border bg-input px-2 text-xs text-foreground focus-visible:ring-2 focus-visible:ring-ring" disabled={linkingPanel} value={(() => {
            const artwork = panels.find((panel) => panel.id === selectedPanel)?.artwork;
            if (artwork?.kind === "page" && artwork.page) return `page:${artwork.page}`;
            if (artwork?.kind === "node" && artwork.page && artwork.node) return `node:${artwork.page}:${artwork.node}`;
            return "";
          })()} onChange={(event) => {
            const panel = panels.find((item) => item.id === selectedPanel);
            if (panel) void setPanelArtwork(panel, event.target.value);
          }}>
            <option value="">No artwork</option>
            {(studio.doc?.pages ?? []).map((page) => <option key={`page:${page.id}`} value={`page:${page.id}`}>Page · {page.name || page.id}</option>)}
            {studio.doc && artworkNodes(studio.doc).filter(({ node: item }) => item.id !== node.id && item.kind !== "scene3d").map(({ page, node: item }) => <option key={`node:${page}:${item.id}`} value={`node:${page}:${item.id}`}>Node · {item.name || item.id}</option>)}
          </select>
          {selectedPanelRecord && <label className="flex min-h-10 items-center gap-2 rounded border px-2 text-xs text-muted-foreground">
            <span className="whitespace-nowrap">Safe area · mm</span>
            <NumberField className="min-h-10 w-20 flex-none rounded bg-input" value={selectedPanelRecord.safe_zone_mm ?? 0} min={0} max={Math.max(0, Math.min(selectedPanelRecord.rect_mm[2], selectedPanelRecord.rect_mm[3]) / 2 - 0.000001)} step={0.5} disabled={placing || linkingPanel || selectedNodeLocked} onCommit={(value) => void setPanelSafeZone(selectedPanelRecord, value)} ariaLabel={`${selectedPanel} safe-area inset in millimeters`} />
          </label>}
          {selectedPanelRecord?.artwork && (() => {
            const transform = selectedPanelRecord.artwork_transform ?? defaultPanelArtworkTransform;
            const disabled = placing || linkingPanel || selectedNodeLocked;
            return <fieldset className="grid w-full grid-cols-2 gap-x-2 gap-y-1 rounded border p-2 sm:w-auto sm:min-w-[20rem]">
              <legend className="px-1 text-[11px] font-medium">Placement · normalized</legend>
              <label className="flex min-h-10 items-center gap-1 text-[10px] text-muted-foreground">Scale X
                <NumberField className="min-h-10 rounded bg-input" value={transform.scale[0]} min={0.01} max={1} step={0.01} disabled={disabled} onCommit={(value) => editSelectedArtworkTransform({ scale: [value, transform.scale[1]] }, "Set packaging artwork scale")} ariaLabel={`${selectedPanel} artwork horizontal scale`} />
              </label>
              <label className="flex min-h-10 items-center gap-1 text-[10px] text-muted-foreground">Scale Y
                <NumberField className="min-h-10 rounded bg-input" value={transform.scale[1]} min={0.01} max={1} step={0.01} disabled={disabled} onCommit={(value) => editSelectedArtworkTransform({ scale: [transform.scale[0], value] }, "Set packaging artwork scale")} ariaLabel={`${selectedPanel} artwork vertical scale`} />
              </label>
              <label className="flex min-h-10 items-center gap-1 text-[10px] text-muted-foreground">Center X
                <NumberField className="min-h-10 rounded bg-input" value={transform.offset[0]} min={transform.scale[0] / 2} max={1 - transform.scale[0] / 2} step={0.01} disabled={disabled} onCommit={(value) => editSelectedArtworkTransform({ offset: [value, transform.offset[1]] }, "Set packaging artwork position")} ariaLabel={`${selectedPanel} artwork horizontal center`} />
              </label>
              <label className="flex min-h-10 items-center gap-1 text-[10px] text-muted-foreground">Center Y
                <NumberField className="min-h-10 rounded bg-input" value={transform.offset[1]} min={transform.scale[1] / 2} max={1 - transform.scale[1] / 2} step={0.01} disabled={disabled} onCommit={(value) => editSelectedArtworkTransform({ offset: [transform.offset[0], value] }, "Set packaging artwork position")} ariaLabel={`${selectedPanel} artwork vertical center`} />
              </label>
              <label className="col-span-2 flex min-h-10 items-center gap-2 text-[10px] text-muted-foreground">Rotation · degrees
                <NumberField className="min-h-10 rounded bg-input" value={transform.rotate} min={-360} max={360} step={1} disabled={disabled} onCommit={(value) => editSelectedArtworkTransform({ rotate: value }, "Set packaging artwork rotation")} ariaLabel={`${selectedPanel} artwork rotation degrees`} />
              </label>
            </fieldset>;
          })()}
          {panels.find((panel) => panel.id === selectedPanel)?.artwork?.page && <Button type="button" variant="outline" className="min-h-10" onClick={() => {
            const panel = panels.find((item) => item.id === selectedPanel);
            if (panel) editPanelArtwork(panel);
          }}><Pencil size={14} aria-hidden="true" /> Edit source artwork</Button>}
          {linkingPanel && <span role="status" className="text-xs text-muted-foreground">Linking…</span>}
        </div>}
        {panelArtworkMessage && <p role={panelArtworkMessage.error ? "alert" : "status"} className={`mx-auto mt-2 w-full max-w-5xl text-xs ${panelArtworkMessage.error ? "text-destructive" : "text-muted-foreground"}`}>{panelArtworkMessage.text}</p>}
        {placing && <p role="status" className="mx-auto mt-2 w-full max-w-5xl text-xs text-muted-foreground">Saving panel artwork…</p>}
        {preflightError && <p role="status" className="mx-auto mt-2 w-full max-w-5xl text-xs text-warning">Panel-level preflight warnings are unavailable: {preflightError}</p>}
        {preview.report.warnings.length > 0 && <ul className="mx-auto mt-2 w-full max-w-5xl list-disc space-y-1 pl-4 text-xs text-muted-foreground">{preview.report.warnings.map((warning) => <li key={warning}>{warning}</li>)}</ul>}
      </div>}
    </section>
  </div>;
}
