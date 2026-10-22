import { useEffect, useRef, useState, type ReactNode } from "react";
import { ArrowLeft, Box, Brush, Camera, Download, FilePlus2, FolderOpen, Home, Image, Layers3, MessagesSquare, Moon, Newspaper, Search, Keyboard, Palette, Printer, Puzzle, Redo2, Sparkles, RefreshCw, Save, Shapes, SlidersHorizontal, Sun, Undo2, Users, PanelRightClose, PanelRightOpen, MoreHorizontal } from "lucide-react";
import type { LucideIcon } from "lucide-react";
import { LATEST_FORMAT, collectIds, useStudio } from "@/store";
import { Button } from "@/components/ui/button";
import { Badge } from "@/components/ui/badge";
import { Tip, TooltipProvider } from "@/components/ui/tooltip";
import { Canvas } from "@/components/Canvas";
import { Outline } from "@/components/Outline";
import { PhotoStrip } from "@/workspaces/PhotoStrip";
import { Properties } from "@/components/Properties";
import { Toolbar } from "@/components/Toolbar";
import { DocumentSettings, DesignAssets } from "@/workspaces/DesignPanel";
import { PageStrip } from "@/components/PageStrip";
import { CompositePanel, ImagePanel, SceneConsole } from "@/workspaces/OperationsPanel";
import { PrintPanel } from "@/workspaces/PrintPanel";
import { LayoutPanel } from "@/workspaces/LayoutPanel";
import { DesignSystemsPanel } from "@/workspaces/DesignSystemsPanel";
import { StatsPanel } from "@/workspaces/StatsPanel";
import { CollabPanel } from "@/workspaces/CollabPanel";
import { ExtensionsPanel } from "@/workspaces/ExtensionsPanel";
import { AiPanel } from "@/workspaces/AiPanel";
import { CommandPalette, ShortcutSheet } from "@/components/CommandPalette";
import { PhotoPanel } from "@/workspaces/PhotoPanel";
import { PhotoStage } from "@/workspaces/PhotoStage";
import { RasterPanel } from "@/workspaces/RasterPanel";
import { ImageAdjust } from "@/workspaces/ImageAdjust";
import { Scene3dPanel } from "@/workspaces/Scene3dPanel";
import { UvWorkspaceStage } from "@/workspaces/UvWorkspaceStage";
import { PackagingWorkspaceStage } from "@/workspaces/PackagingWorkspaceStage";
import { useLayout } from "@/workspaces/layout/layoutStore";
import { nextId, pageParent } from "@/workspaces/layout/layoutModel";
import { findNode, isScene, pageOf, pagesOf } from "@/lib/model";
import { exportAs } from "@/lib/actions";
import { cn } from "@/lib/utils";
import { getJson, layoutOps } from "@/lib/api";
import { useRenderStats } from "@/lib/wasmCore";
import { Welcome } from "@/components/Welcome";
import { ActivityToast, HistoryButton, HistoryPanel, RecentActors, VersionPreviewBar } from "@/components/Presence";
import { usePaint } from "@/workspaces/rasterStore";
import { startPainting } from "@/workspaces/paintLayer";

const HINTS = {
  select: "Click to select, drag to move, drag handles to resize (Shift keeps proportions). Arrow keys nudge.",
  pen: "Click to add points, drag for curves, click the first point to close, Enter to finish, Esc to cancel.",
  rect: "Drag to draw a rectangle (Shift for a square).",
  ellipse: "Drag to draw an ellipse (Shift for a circle).",
  line: "Drag to draw a line.",
  text: "Click the page to place text, then edit it in Properties.",
  hand: "Drag to pan. Ctrl + wheel zooms.",
} as const;

type Panel = { id: string; label: string; icon: LucideIcon; hint: string; focus?: boolean };

/**
 * One editor, one canvas. The rail picks the panel beside the canvas; only
 * Photo and Packaging/3D replace the canvas with a focus view of their own.
 */
const PANELS: Panel[] = [
  { id: "design", label: "Properties", icon: SlidersHorizontal, hint: "Selection properties, or page and document settings" },
  { id: "assets", label: "Insert and assets", icon: Shapes, hint: "Images, text, fonts, imports and reusable assets" },
  { id: "image", label: "Images", icon: Image, hint: "Image adjustments and operations" },
  { id: "composite", label: "Composite", icon: Layers3, hint: "Blend modes, effects and compositing" },
  { id: "paint", label: "Paint", icon: Brush, hint: "Brush, retouch and selection options; paints on the canvas" },
  { id: "layout", label: "Layout", icon: Newspaper, hint: "Stories, frames, spreads, long documents and EPUB" },
  { id: "print", label: "Print", icon: Printer, hint: "Color roles, preflight, plates and print export" },
  { id: "systems", label: "Design systems", icon: Palette, hint: "Tokens, libraries, templates and token audit" },
  { id: "collab", label: "Collaboration", icon: MessagesSquare, hint: "People, comments, reviews and merge conflicts" },
  { id: "stats", label: "Stats", icon: Users, hint: "Contribution stats from the operation log" },
  { id: "extensions", label: "Extensions", icon: Puzzle, hint: "Sandboxed WebAssembly extensions" },
  { id: "ai", label: "AI", icon: Sparkles, hint: "Generate, review and accept images" },
  { id: "photo", label: "Photo", icon: Camera, hint: "Catalog, culling and RAW development (focus view)", focus: true },
  { id: "3d", label: "Packaging and 3D", icon: Box, hint: "Dielines, folding and 3D mockups (focus view)", focus: true },
];

function panelFor(id: string): Panel {
  return PANELS.find((panel) => panel.id === id) ?? PANELS[0]!;
}

function WorkspaceBody() {
  const { workspaceId, selectedId, doc, pageId } = useStudio();
  const picked = selectedId ? findNode(pageOf(doc, pageId), selectedId)?.node : null;
  const selectedImage = picked?.kind === "image" && doc && doc.version >= 5 ? picked : null;
  switch (workspaceId) {
    case "assets":
      return (
        <div className="space-y-4">
          <DesignAssets />
          <details className="rounded-md border bg-muted/20 p-2">
            <summary className="cursor-pointer text-xs font-medium text-muted-foreground">Advanced scene console</summary>
            <div className="pt-2"><SceneConsole /></div>
          </details>
        </div>
      );
    case "image":
      return <ImagePanel />;
    case "composite":
      return <CompositePanel />;
    case "print":
      return <PrintPanel />;
    case "layout":
      return <div className="-m-3"><LayoutPanel /></div>;
    case "photo":
      return <PhotoPanel />;
    case "paint":
      return <RasterPanel />;
    case "3d":
      return <Scene3dPanel />;
    case "extensions":
      return <div className="-m-3"><ExtensionsPanel /></div>;
    case "ai":
      return <div className="-m-3"><AiPanel /></div>;
    case "collab":
      return <div className="-m-3"><CollabPanel /></div>;
    case "stats":
      return <div className="-m-3"><StatsPanel /></div>;
    case "systems":
      return <div className="-m-3"><DesignSystemsPanel /></div>;
    default:
      return picked ? <><Properties />{selectedImage && <div className="mt-4 border-t pt-3"><ImageAdjust key={selectedImage.id} node={selectedImage} /></div>}</> : <DocumentSettings />;
  }
}

/** A visited stage stays mounted; inactive stages pause previews and shortcuts. */
function KeepStage({ active, children, label }: { active: boolean; children: ReactNode; label: string }) {
  const [visited, setVisited] = useState(active);
  useEffect(() => { if (active) setVisited(true); }, [active]);
  if (!active && !visited) return null;
  return <section className="studio-stage" hidden={!active} aria-label={label}>{children}</section>;
}

/** Layers, with the project's photos one tab away for dragging onto the canvas. */
function LayersDock() {
  const [tab, setTab] = useState<"layers" | "photos">(() => {
    try {
      return localStorage.getItem("pentool.dock-tab") === "photos" ? "photos" : "layers";
    } catch {
      return "layers";
    }
  });
  const choose = (next: "layers" | "photos") => {
    setTab(next);
    try {
      localStorage.setItem("pentool.dock-tab", next);
    } catch {
      // Remembering the tab is a convenience only.
    }
  };
  return <>
    <div role="tablist" aria-label="Layers and photos" className="mb-2 flex gap-1 border-b pb-1">
      {(["layers", "photos"] as const).map((id) => (
        <button key={id} type="button" role="tab" aria-selected={tab === id} onClick={() => choose(id)}
          className={cn("rounded px-2 py-0.5 text-[11px] font-semibold uppercase tracking-wide", tab === id ? "bg-accent text-foreground" : "text-muted-foreground hover:text-foreground")}>
          {id}
        </button>
      ))}
    </div>
    <div role="tabpanel">{tab === "layers" ? <Outline /> : <PhotoStrip />}</div>
  </>;
}

function Dock({ mobileOpen, collapsed, onCollapse }: { mobileOpen: boolean; collapsed: boolean; onCollapse: () => void }) {
  const { workspaceId, selectedId, setWorkspace } = useStudio();
  const [width, setWidth] = useState(320);
  const [layerHeight, setLayerHeight] = useState(280);
  const dock = useRef<HTMLElement>(null);
  const panel = panelFor(workspaceId);
  // Selecting an object while browsing assets shows its properties.
  useEffect(() => { if (selectedId && useStudio.getState().workspaceId === "assets") setWorkspace("design"); }, [selectedId, setWorkspace]);
  const changeWidth = (value: number) => setWidth(Math.max(260, Math.min(480, value)));
  const changeLayers = (value: number) => setLayerHeight(Math.max(120, Math.min((dock.current?.clientHeight ?? 700) - 160, value)));
  return <aside ref={dock} id="mobile-panels" aria-label="Inspector and layers" style={{ width }}
    className={cn("studio-dock", collapsed && "is-collapsed", mobileOpen && "is-mobile-open")}>
    <div role="separator" tabIndex={0} aria-label="Resize inspector" aria-orientation="vertical" aria-valuemin={260} aria-valuemax={480} aria-valuenow={width}
      className="studio-dock-resizer" onKeyDown={(event) => { if (event.key === "ArrowLeft" || event.key === "ArrowRight") { event.preventDefault(); changeWidth(width + (event.key === "ArrowLeft" ? 16 : -16)); } }}
      onPointerDown={(event) => { event.currentTarget.setPointerCapture(event.pointerId); }}
      onPointerMove={(event) => { if (event.currentTarget.hasPointerCapture(event.pointerId)) changeWidth((dock.current?.getBoundingClientRect().right ?? window.innerWidth) - event.clientX); }}
      onPointerUp={(event) => { if (event.currentTarget.hasPointerCapture(event.pointerId)) event.currentTarget.releasePointerCapture(event.pointerId); }} />
    <div className="studio-inspector flex-col">
      <div className="flex h-9 shrink-0 items-center gap-2 border-b pl-3 pr-1">
        <panel.icon size={14} aria-hidden="true" className="text-muted-foreground" />
        <h2 className="min-w-0 flex-1 truncate text-xs font-semibold">{panel.id === "design" && !selectedId ? "Page and document" : panel.label}</h2>
        <Button variant="ghost" size="icon" className="size-8 shrink-0" aria-label="Hide panels" onClick={onCollapse}><PanelRightClose size={14} /></Button>
      </div>
      <div role="region" aria-label={panel.label} className="min-h-0 flex-1 overflow-y-auto p-3"><WorkspaceBody /></div>
    </div>
    <div role="separator" tabIndex={0} aria-label="Resize layers" aria-orientation="horizontal" aria-valuemin={120} aria-valuemax={Math.max(120, (dock.current?.clientHeight ?? 700) - 160)} aria-valuenow={layerHeight}
      className="studio-layers-resizer" onKeyDown={(event) => { if (event.key === "ArrowUp" || event.key === "ArrowDown") { event.preventDefault(); changeLayers(layerHeight + (event.key === "ArrowUp" ? 24 : -24)); } }}
      onPointerDown={(event) => event.currentTarget.setPointerCapture(event.pointerId)}
      onPointerMove={(event) => { if (event.currentTarget.hasPointerCapture(event.pointerId)) changeLayers((dock.current?.getBoundingClientRect().bottom ?? window.innerHeight) - event.clientY); }}
      onPointerUp={(event) => { if (event.currentTarget.hasPointerCapture(event.pointerId)) event.currentTarget.releasePointerCapture(event.pointerId); }} />
    <section aria-label="Document layers" className="studio-layers" style={{ height: layerHeight }}><LayersDock /></section>
  </aside>;
}

/** Panel switcher at the window edge, like Photoshop's panel icons or Blender's property tabs. */
function PanelRail({ open, onOpen, onToggle, aiEnabled }: { open: boolean; onOpen: () => void; onToggle: () => void; aiEnabled: boolean }) {
  const { workspaceId, setWorkspace, doc } = useStudio();
  const choose = (panel: Panel) => {
    if (panel.id === workspaceId && open && !panel.focus) return onToggle();
    if (panel.id === "paint") {
      if (isScene(doc)) startPainting(usePaint.getState().tool);
      else setWorkspace("paint");
    } else setWorkspace(panel.id);
    onOpen();
  };
  return <nav aria-label="Panels" className="studio-panelrail">
    {PANELS.filter((panel) => panel.id !== "ai" || aiEnabled).map((panel, index) => <div key={panel.id} className="contents">
      {panel.focus && !PANELS[index - 1]?.focus && <span aria-hidden="true" className="studio-toolrail-divider" />}
      <Tip label={`${panel.label}: ${panel.hint}`} side="left">
        <button type="button" aria-label={panel.label} aria-pressed={workspaceId === panel.id && (open || panel.focus)} onClick={() => choose(panel)}
          className={cn("studio-tool-button", workspaceId === panel.id && (open || panel.focus) && "is-active")}>
          <panel.icon size={17} aria-hidden="true" />
        </button>
      </Tip>
    </div>)}
  </nav>;
}

export default function App() {
  const s = useStudio();
  const [aiEnabled, setAiEnabled] = useState(false);
  useEffect(() => { void getJson<{ enabled: boolean }>("/api/ai/status").then((status) => setAiEnabled(status.enabled)).catch(() => {}); }, []);
  const render = useRenderStats();
  const paint = usePaint();
  const openInput = useRef<HTMLInputElement>(null);
  const exportMenu = useRef<HTMLDetailsElement>(null);
  const [mobilePanelsOpen, setMobilePanelsOpen] = useState(false);
  const [showWelcome, setShowWelcome] = useState(true);
  const [panelsCollapsed, setPanelsCollapsed] = useState(false);
  const [palette, setPalette] = useState(false);
  const [sheet, setSheet] = useState(false);
  const [narrow, setNarrow] = useState(() => window.matchMedia("(max-width: 1023px)").matches);
  useEffect(() => {
    const query = window.matchMedia("(max-width: 1023px)");
    const update = () => setNarrow(query.matches);
    query.addEventListener("change", update);
    return () => query.removeEventListener("change", update);
  }, []);
  useEffect(() => {
    void s.load().then(() => {
      // When pentool serve opens an existing project, take the user straight
      // into the editor. Keep the welcome screen for a bare server and Home.
      if (useStudio.getState().shared) setShowWelcome(false);
    });
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);
  useEffect(() => {
    document.documentElement.classList.toggle("dark", s.dark);
  }, [s.dark]);
  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      // Ctrl/Cmd+K works everywhere, even while typing in a field.
      if ((event.ctrlKey || event.metaKey) && !event.altKey && event.key.toLowerCase() === "k") {
        event.preventDefault();
        setSheet(false);
        setPalette((open) => !open);
        return;
      }
      const target = event.target as HTMLElement;
      const tag = target.tagName;
      if (tag === "INPUT" || tag === "TEXTAREA" || tag === "SELECT" || target.isContentEditable) return;
      const key = event.key.toLowerCase();
      const state = useStudio.getState();
      if (event.key === "?" && !event.ctrlKey && !event.metaKey) {
        event.preventDefault();
        setSheet((open) => !open);
        return;
      }
      if (event.ctrlKey || event.metaKey) {
        if (key === "s") {
          event.preventDefault();
          void state.save();
        } else if (key === "p" && event.shiftKey && state.workspaceId === "layout" && useLayout.getState().indesignKeys && state.doc) {
          event.preventDefault();
          const doc = state.doc;
          const current = pagesOf(doc).find((p) => p.id === state.pageId);
          const id = nextId("page", collectIds(doc));
          void useLayout
            .getState()
            .run({ type: "page-add", id, after: current?.id, parent: pageParent(current) }, "Add page")
            .then((ok) => ok && useStudio.getState().setPage(id));
        }
        return;
      }
      if (state.workspaceId !== "layout" || !useLayout.getState().indesignKeys || event.altKey) return;
      if (key === "v") state.setTool("select");
      else if (key === "t") state.setTool("text");
      else if (key === "pageup" || key === "pagedown") {
        const pages = state.doc?.pages ?? [];
        const index = pages.findIndex((p) => p.id === state.pageId);
        const next = pages[index + (key === "pageup" ? -1 : 1)];
        if (next) {
          event.preventDefault();
          state.setPage(next.id);
        }
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);
  useEffect(() => {
    const warn = (event: BeforeUnloadEvent) => {
      if (useStudio.getState().dirty) event.preventDefault();
    };
    window.addEventListener("beforeunload", warn);
    return () => window.removeEventListener("beforeunload", warn);
  }, []);

  // The palette opens panels the same way the rail does.
  const openPanel = (id: string) => {
    if (id === "paint" && isScene(s.doc)) startPainting(usePaint.getState().tool);
    else s.setWorkspace(id);
    if (narrow) setMobilePanelsOpen(true);
    else setPanelsCollapsed(false);
  };
  const confirmDiscard = () => !s.dirty || window.confirm("Discard unsaved changes?");
  const pages = pagesOf(s.doc);
  const pageIndex = pages.findIndex((p) => p.id === s.pageId);
  const stage = s.workspaceId === "photo" ? "photo" : s.workspaceId === "3d" ? "3d" : "canvas";
  const painting = stage === "canvas" && s.paintMode;
  const selectedNode = s.doc ? findNode(pageOf(s.doc, s.pageId), s.selectedId)?.node ?? null : null;
  const hasSelectedDieline = selectedNode?.kind === "scene3d" && typeof selectedNode.dieline === "string";
  const downloadLayout = (format: "pdf" | "epub") => {
    exportMenu.current?.removeAttribute("open");
    if (!s.doc) return;
    void layoutOps.download(s.doc, format).catch((error) => s.notify(error instanceof Error ? error.message : `Could not export ${format.toUpperCase()}.`, true));
  };
  const openFile = async (file: File) => {
    if (!confirmDiscard()) return;
    try {
      s.openDoc(JSON.parse(await file.text()));
      setShowWelcome(false);
    } catch {
      s.notify("That file is not a valid .pen document.", true);
    }
  };
  return (
    <TooltipProvider>
      <input
        ref={openInput}
        hidden
        type="file"
        accept=".pen,application/json"
        onChange={(e) => {
          const file = e.target.files?.[0];
          e.target.value = "";
          if (file) void openFile(file);
        }}
      />
      {showWelcome ? (
        <Welcome
          busy={s.busy}
          doc={s.doc}
          dirty={s.dirty}
          version={s.version}
          onCreate={(width, height) => {
            if (!confirmDiscard()) return;
            void s.newDoc(width, height);
            setShowWelcome(false);
          }}
          onOpen={() => openInput.current?.click()}
          onOpenFile={(file) => void openFile(file)}
          onContinue={() => setShowWelcome(false)}
        />
      ) : (
      <div className="studio-shell flex h-full flex-col bg-background text-foreground">
        <CommandPalette open={palette} onClose={() => setPalette(false)} openPanel={openPanel} panels={PANELS.filter((panel) => panel.id !== "ai" || aiEnabled)} />
        <ShortcutSheet open={sheet} onClose={() => setSheet(false)} />
        <ActivityToast />
        <HistoryPanel />
        <VersionPreviewBar />
        <header className="studio-appbar flex h-12 shrink-0 items-center gap-2 border-b bg-card px-2 sm:px-3">
          <Tip label="Start screen">
            <Button variant="ghost" size="icon" className="size-10 shrink-0" aria-label="Start screen" onClick={() => setShowWelcome(true)}>
              <Home size={15} />
            </Button>
          </Tip>
          <div className="studio-brand shrink-0"><strong>Pentool Studio</strong></div>
          {s.version && <Badge className="shrink-0 px-1.5 text-[10px]" title="Pentool version" aria-label={`Pentool version ${s.version}`}>v{s.version}</Badge>}
          {stage === "canvas" && !painting && s.tool !== "select" && s.tool !== "hand" && <div className="studio-brush-options" aria-label="Drawing options">
            <label>Fill <input aria-label="New object fill" type="color" value={s.look.fill.startsWith("#") ? s.look.fill : "#000000"} onChange={(event) => s.setLook({ fill: event.target.value })} /></label>
            <label>Stroke <input aria-label="New object stroke" type="color" value={s.look.stroke.startsWith("#") ? s.look.stroke : "#000000"} onChange={(event) => s.setLook({ stroke: event.target.value })} /></label>
            <label>Width <input aria-label="New object stroke width" type="number" min={0} max={1000} value={s.look.strokeWidth} onChange={(event) => s.setLook({ strokeWidth: Math.max(0, Math.min(1000, Number(event.target.value) || 0)) })} /></label>
          </div>}
          {painting && <div className="studio-brush-options" aria-label="Brush options">
            <label>Size <input aria-label="Brush size" type="number" min={1} max={200} value={paint.size} onChange={(event) => paint.set({ size: Math.max(1, Math.min(200, Number(event.target.value) || 1)) })} /></label>
            <label>Flow <input aria-label="Brush flow" type="number" min={5} max={100} value={Math.round(paint.flow * 100)} onChange={(event) => paint.set({ flow: Math.max(0.05, Math.min(1, Number(event.target.value) / 100)) })} />%</label>
            <input aria-label="Paint color" type="color" value={paint.color} onChange={(event) => paint.set({ color: event.target.value })} />
          </div>}

          <span className="min-w-0 flex-1 truncate text-xs text-muted-foreground" title={String(s.doc?.name ?? "Untitled")}>{String(s.doc?.name ?? "Untitled")}{s.dirty ? " · unsaved" : ""}</span>
          <RecentActors />
          <div className="studio-app-actions hidden shrink-0 items-center gap-0.5 md:flex">
            <Tip label="New document"><Button variant="ghost" size="icon" className="size-9" aria-label="New document" onClick={() => confirmDiscard() && s.newDoc()}><FilePlus2 size={15} /></Button></Tip>
            <Tip label="Open .pen file"><Button variant="ghost" size="icon" className="size-9" aria-label="Open .pen file" onClick={() => openInput.current?.click()}><FolderOpen size={15} /></Button></Tip>
          </div>
          <Tip label="Search commands (Ctrl+K)">
            <Button variant="outline" size="sm" className="hidden h-9 gap-2 px-2 text-xs text-muted-foreground lg:inline-flex" aria-label="Search commands" onClick={() => setPalette(true)}>
              <Search size={14} aria-hidden="true" /><span>Search</span><kbd className="text-[10px]">Ctrl K</kbd>
            </Button>
          </Tip>
          <span className="mx-1 hidden h-6 w-px bg-border sm:block" />
          <HistoryButton />
          <Tip label="Undo"><Button variant="ghost" size="icon" className="size-9" aria-label="Undo" onClick={s.undo} disabled={!s.past.length}><Undo2 size={15} /></Button></Tip>
          <Tip label="Redo"><Button variant="ghost" size="icon" className="size-9" aria-label="Redo" onClick={s.redo} disabled={!s.future.length}><Redo2 size={15} /></Button></Tip>
          <Tip label="Save (Ctrl+S)">
            <Button variant="default" size="sm" className="h-9 px-3" aria-label="Save" onClick={() => void s.save()}>
              <Save size={15} />
              <span className="ml-1 hidden sm:inline">Save</span>
            </Button>
          </Tip>
          <details ref={exportMenu} className="studio-export-menu relative shrink-0" onKeyDown={(event) => {
            if (event.key === "Escape") {
              exportMenu.current?.removeAttribute("open");
              (event.currentTarget.querySelector("summary") as HTMLElement | null)?.focus();
            }
          }} onBlur={(event) => {
            if (!event.currentTarget.contains(event.relatedTarget as Node | null)) exportMenu.current?.removeAttribute("open");
          }}>
            <summary className="flex h-9 cursor-pointer items-center gap-1.5 rounded-md border bg-background px-2 text-xs font-medium hover:bg-accent focus-visible:outline-2 focus-visible:outline-primary focus-visible:outline-offset-2">
              <Download size={14} aria-hidden="true" /><span>Export</span>
            </summary>
            <div className="studio-export-popover absolute right-0 top-full z-50 mt-2 grid min-w-48 gap-1 rounded-md border bg-card p-2 shadow-lg">
              <button type="button" className="studio-export-option" onClick={() => { exportMenu.current?.removeAttribute("open"); void exportAs("png"); }}>PNG <small>Current page image</small></button>
              <button type="button" className="studio-export-option" onClick={() => { exportMenu.current?.removeAttribute("open"); void exportAs("svg"); }}>SVG <small>Current page vectors</small></button>
              <button type="button" className="studio-export-option" disabled={!s.doc} onClick={() => downloadLayout("pdf")}>PDF <small>Print ready pages</small></button>
              <button type="button" className="studio-export-option" disabled={!s.doc} onClick={() => downloadLayout("epub")}>EPUB <small>Book publication</small></button>
            </div>
          </details>
          <Button variant="ghost" size="icon" className="size-9 shrink-0" aria-label="Toggle panels" aria-expanded={narrow ? mobilePanelsOpen : !panelsCollapsed} aria-controls="mobile-panels" onClick={() => { if (narrow) setMobilePanelsOpen((value) => !value); else setPanelsCollapsed((value) => !value); }}><PanelRightOpen size={16} /></Button>
          <Tip label="Reload project"><Button variant="ghost" size="icon" className="size-9 shrink-0" aria-label="Reload project" disabled={s.busy} onClick={() => confirmDiscard() && void s.load()}><RefreshCw size={14} /></Button></Tip>
          <details className="studio-compact-menu relative hidden">
            <summary className="grid size-9 cursor-pointer list-none place-items-center rounded hover:bg-accent" aria-label="More actions"><MoreHorizontal size={16} /></summary>
            <div className="absolute right-0 top-full z-50 mt-2 grid min-w-40 gap-1 rounded border bg-card p-2 shadow-lg">
              <button type="button" className="studio-export-option" disabled={!s.future.length} onClick={s.redo}>Redo</button>
              <button type="button" className="studio-export-option" disabled={s.busy} onClick={() => confirmDiscard() && void s.load()}>Reload project</button>
              <button type="button" className="studio-export-option" onClick={s.toggleDark}>Switch theme</button>
            </div>
          </details>
          <Tip label="Toggle theme">
            <Button variant="ghost" size="icon" className="size-9" aria-label="Toggle theme" onClick={s.toggleDark}>
              {s.dark ? <Sun size={15} /> : <Moon size={15} />}
            </Button>
          </Tip>
        </header>

        <div className="studio-editor-body flex min-h-0 flex-1 flex-row">
          <Toolbar />
          <div className="studio-stage-column">
            {stage !== "canvas" && <div className="studio-focusbar" role="toolbar" aria-label="Focus view">
              <Button variant="ghost" size="sm" className="h-8 gap-1.5" onClick={() => s.setWorkspace("design")}><ArrowLeft size={14} aria-hidden="true" />Back to canvas</Button>
              <span className="text-xs font-medium">{stage === "photo" ? "Photo" : "Packaging and 3D"}</span>
              <span className="min-w-0 truncate text-[11px] text-muted-foreground">{stage === "photo" ? "Develop photos, then place them on a page." : "Fold dielines and preview the 3D mockup. Drag a page from the page strip onto artwork."}</span>
            </div>}
            <div className="studio-stage-area">
              <KeepStage active={stage === "canvas"} label="Design canvas"><Canvas active={stage === "canvas"} /></KeepStage>
              <KeepStage active={stage === "photo"} label="Photo development"><PhotoStage active={stage === "photo"} /></KeepStage>
              <KeepStage active={stage === "3d"} label="Packaging canvas">
                {hasSelectedDieline ? <PackagingWorkspaceStage active={stage === "3d"} /> : <UvWorkspaceStage active={stage === "3d"} />}
              </KeepStage>
            </div>
            {stage !== "photo" && <PageStrip />}
          </div>
          <Dock mobileOpen={mobilePanelsOpen} collapsed={panelsCollapsed} onCollapse={() => { setPanelsCollapsed(true); setMobilePanelsOpen(false); }} />
          <PanelRail aiEnabled={aiEnabled} open={narrow ? mobilePanelsOpen : !panelsCollapsed}
            onOpen={() => { if (narrow) setMobilePanelsOpen(true); else setPanelsCollapsed(false); }}
            onToggle={() => { if (narrow) setMobilePanelsOpen((value) => !value); else setPanelsCollapsed((value) => !value); }} />
        </div>

        {/* Status bar. */}
        <footer className="flex h-6 shrink-0 items-center gap-3 border-t bg-card px-3 text-[11px] text-muted-foreground">
          <span className="min-w-0 flex-1 truncate">{painting ? (paint.busy ? "Applying strokes…" : paint.status) : stage === "canvas" ? HINTS[s.tool] : stage === "3d" ? "Create a package or select a model to place artwork." : ""}</span>
          {s.message && <span role="status" className={cn("max-w-64 truncate", !s.message.error && "hidden xl:inline", s.message.error && "text-destructive")}>{s.message.text}</span>}
          {s.doc && s.doc.version < LATEST_FORMAT && <button className="shrink-0 text-primary hover:underline" disabled={s.busy} onClick={() => { if (window.confirm(`Upgrade to format v${LATEST_FORMAT}? Save to keep the upgrade; Undo reverts it.`)) void s.upgradeDoc(); }}>Upgrade format</button>}

          <Tip
            label={
              render.available === false
                ? "Local renderer not built into this binary; the server renders previews."
                : "Render previews in the browser with the WebAssembly core. Exports always use the native renderer."
            }
          >
            <button
              aria-pressed={render.enabled}
              disabled={render.available === false}
              onClick={() => render.setEnabled(!render.enabled)}
              className={cn("hidden shrink-0 rounded px-1.5 hover:text-foreground xl:inline", render.enabled && render.available !== false && "text-primary")}
            >
              Local render {render.available === false ? "n/a" : render.enabled ? "on" : "off"}
            </button>
          </Tip>
          {render.last && (
            <span className="hidden tabular-nums xl:inline" title="Time to produce the last preview">
              {render.last.source === "wasm" ? "wasm" : "server"} {render.last.ms} ms
            </span>
          )}
          <button type="button" className="hidden shrink-0 items-center gap-1 hover:text-foreground md:inline-flex" aria-label="Keyboard shortcuts" title="Keyboard shortcuts (?)" onClick={() => setSheet(true)}><Keyboard size={12} aria-hidden="true" />?</button>
          {stage === "canvas" && <span className="tabular-nums" title="Zoom (Ctrl+0 fits, Ctrl+1 is 100%)">{Math.round(s.scale * 100)}%</span>}
          <span className="tabular-nums">{pages.length ? `Page ${pageIndex + 1} of ${pages.length}` : "No pages"}</span>
        </footer>
      </div>
      )}
    </TooltipProvider>
  );
}
