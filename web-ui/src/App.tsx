import { useEffect, useRef, useState, type ReactNode } from "react";
import { Box, Brush, Camera, Download, FilePlus2, FolderOpen, Home, Image, Layers3, Moon, Newspaper, PenTool, Printer, Puzzle, Redo2, RefreshCw, Save, Sun, Undo2, PanelRightClose, PanelRightOpen, MoreHorizontal } from "lucide-react";
import { LATEST_FORMAT, collectIds, useStudio } from "@/store";
import { workspaces } from "@/lib/workspaces";
import { Button } from "@/components/ui/button";
import { Badge } from "@/components/ui/badge";
import { Tip, TooltipProvider } from "@/components/ui/tooltip";
import { Canvas } from "@/components/Canvas";
import { Outline } from "@/components/Outline";
import { Properties } from "@/components/Properties";
import { Toolbar } from "@/components/Toolbar";
import { DesignPanel, DocumentSettings, DesignAssets } from "@/workspaces/DesignPanel";
import { CompositePanel, ImagePanel, SceneConsole } from "@/workspaces/OperationsPanel";
import { PrintPanel } from "@/workspaces/PrintPanel";
import { LayoutPanel } from "@/workspaces/LayoutPanel";
import { DesignSystemsPanel } from "@/workspaces/DesignSystemsPanel";
import { ExtensionsPanel, ExtensionPalette } from "@/workspaces/ExtensionsPanel";
import { Planned } from "@/workspaces/Planned";
import { PhotoPanel } from "@/workspaces/PhotoPanel";
import { PhotoStage } from "@/workspaces/PhotoStage";
import { RasterPanel } from "@/workspaces/RasterPanel";
import { RasterStage } from "@/workspaces/RasterStage";
import { ImageAdjust } from "@/workspaces/ImageAdjust";
import { Scene3dPanel } from "@/workspaces/Scene3dPanel";
import { UvWorkspaceStage } from "@/workspaces/UvWorkspaceStage";
import { PackagingWorkspaceStage } from "@/workspaces/PackagingWorkspaceStage";
import { useLayout } from "@/workspaces/layout/layoutStore";
import { nextId, pageParent } from "@/workspaces/layout/layoutModel";
import { findNode, pageOf, pagesOf } from "@/lib/model";
import { exportAs } from "@/lib/actions";
import { cn } from "@/lib/utils";
import { layoutOps } from "@/lib/api";
import { useRenderStats } from "@/lib/wasmCore";
import { Welcome } from "@/components/Welcome";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { usePaint } from "@/workspaces/rasterStore";

const HINTS = {
  select: "Click to select, drag to move, drag handles to resize (Shift keeps proportions). Arrow keys nudge.",
  pen: "Click to add points, drag for curves, click the first point to close, Enter to finish, Esc to cancel.",
  rect: "Drag to draw a rectangle (Shift for a square).",
  ellipse: "Drag to draw an ellipse (Shift for a circle).",
  line: "Drag to draw a line.",
  text: "Click the page to place text, then edit it in Properties.",
  hand: "Drag to pan. Ctrl + wheel zooms.",
} as const;

const WORKFLOWS = [
  { id: "design", label: "Design", icon: PenTool, modes: [
    { id: "design", label: "Canvas", icon: PenTool },
    { id: "image", label: "Images", icon: Image },
    { id: "composite", label: "Composite", icon: Layers3 },
    { id: "paint", label: "Paint", icon: Brush },
  ] },
  { id: "photo", label: "Photo", icon: Camera, modes: [{ id: "photo", label: "Photo editing", icon: Camera }] },
  { id: "layout", label: "Layout", icon: Newspaper, modes: [{ id: "layout", label: "Pages and books", icon: Newspaper }] },
  { id: "3d", label: "Packaging", icon: Box, modes: [{ id: "3d", label: "3D mockup", icon: Box }] },
  { id: "print", label: "Print", icon: Printer, modes: [{ id: "print", label: "Preflight and export", icon: Printer }] },
  { id: "extensions", label: "Extensions", icon: Puzzle, modes: [{ id: "extensions", label: "Extensions", icon: Puzzle }] },
] as const;

function workflowFor(workspaceId: string) {
  return WORKFLOWS.find((workflow) => workflow.modes.some((mode) => mode.id === workspaceId)) ?? WORKFLOWS[0]!;
}

function WorkspaceBody() {
  const { workspaceId } = useStudio();
  const workspace = workspaces.find((w) => w.id === workspaceId) ?? workspaces[0]!;
  switch (workspace.id) {
    case "design":
      return (
        <div className="space-y-4 p-2">
          <DesignPanel />
          <details className="rounded-md border bg-muted/20 p-2">
            <summary className="cursor-pointer text-xs font-medium text-muted-foreground">Advanced scene console</summary>
            <div className="pt-2"><SceneConsole /></div>
          </details>
        </div>
      );
    case "image":
      return (
        <div className="p-2">
          <ImagePanel />
        </div>
      );
    case "composite":
      return (
        <div className="p-2">
          <CompositePanel />
        </div>
      );
    case "print":
      return (
        <div className="p-2">
          <PrintPanel />
        </div>
      );
    case "layout":
      return <LayoutPanel />;
    case "photo":
      return (
        <div className="p-2">
          <PhotoPanel />
        </div>
      );
    case "paint":
      return (
        <div className="p-2">
          <RasterPanel />
        </div>
      );
    case "3d":
      return (
        <div className="p-2">
          <Scene3dPanel />
        </div>
      );
    case "extensions":
      return <ExtensionsPanel />;
    case "systems":
      return <DesignSystemsPanel />;
    default:
      return (
        <div className="p-2">
          <Planned workspace={workspace} />
        </div>
      );
  }
}

/** A visited stage stays mounted; inactive stages pause previews and shortcuts. */
function KeepStage({ active, children, label }: { active: boolean; children: ReactNode; label: string }) {
  const [visited, setVisited] = useState(active);
  useEffect(() => { if (active) setVisited(true); }, [active]);
  if (!active && !visited) return null;
  return <section className="studio-stage" hidden={!active} aria-label={label}>{children}</section>;
}

function Dock({ mobileOpen, collapsed, onCollapse }: { mobileOpen: boolean; collapsed: boolean; onCollapse: () => void }) {
  const { workspaceId, selectedId, doc, pageId } = useStudio();
  const picked = selectedId ? findNode(pageOf(doc, pageId), selectedId)?.node : null;
  const selectedImage = picked?.kind === "image" && doc && doc.version >= 5 ? picked : null;
  const [tab, setTab] = useState("properties");
  const [width, setWidth] = useState(320);
  const [layerHeight, setLayerHeight] = useState(280);
  const dock = useRef<HTMLElement>(null);
  useEffect(() => { setTab(workspaceId === "design" ? "properties" : "tools"); }, [workspaceId]);
  useEffect(() => { if (selectedId) setTab("properties"); }, [selectedId]);
  const changeWidth = (value: number) => setWidth(Math.max(260, Math.min(440, value)));
  const changeLayers = (value: number) => setLayerHeight(Math.max(120, Math.min((dock.current?.clientHeight ?? 700) - 160, value)));
  return <aside ref={dock} id="mobile-panels" aria-label="Inspector and layers" style={{ width }}
    className={cn("studio-dock", collapsed && "is-collapsed", mobileOpen && "is-mobile-open")}>
    <div role="separator" tabIndex={0} aria-label="Resize inspector" aria-orientation="vertical" aria-valuemin={260} aria-valuemax={440} aria-valuenow={width}
      className="studio-dock-resizer" onKeyDown={(event) => { if (event.key === "ArrowLeft" || event.key === "ArrowRight") { event.preventDefault(); changeWidth(width + (event.key === "ArrowLeft" ? 16 : -16)); } }}
      onPointerDown={(event) => { event.currentTarget.setPointerCapture(event.pointerId); }}
      onPointerMove={(event) => { if (event.currentTarget.hasPointerCapture(event.pointerId)) changeWidth((dock.current?.getBoundingClientRect().right ?? window.innerWidth) - event.clientX); }}
      onPointerUp={(event) => { if (event.currentTarget.hasPointerCapture(event.pointerId)) event.currentTarget.releasePointerCapture(event.pointerId); }} />
    <div className="studio-inspector">
      <Tabs value={tab} onValueChange={setTab} className="flex min-h-0 flex-1 flex-col">
        <div className="flex shrink-0 items-center border-b">
          <TabsList className="grid flex-1 grid-cols-3 border-0 px-1">
            <TabsTrigger value="properties">Properties</TabsTrigger>
            <TabsTrigger value="tools">{workspaceId === "3d" ? "Packaging" : "Tools"}</TabsTrigger>
            <TabsTrigger value="assets">Assets</TabsTrigger>
          </TabsList>
          <Button variant="ghost" size="icon" className="size-8 shrink-0" aria-label="Hide panels" onClick={onCollapse}><PanelRightClose size={14} /></Button>
        </div>
        <TabsContent value="properties" className="m-0 min-h-0 flex-1 overflow-y-auto p-3">
          {picked ? <><Properties />{selectedImage && <div className="mt-4 border-t pt-3"><ImageAdjust key={selectedImage.id} node={selectedImage} /></div>}</> : <DocumentSettings />}
        </TabsContent>
        <TabsContent value="tools" className="m-0 min-h-0 flex-1 overflow-y-auto p-2"><WorkspaceBody /></TabsContent>
        <TabsContent value="assets" className="m-0 min-h-0 flex-1 overflow-y-auto p-3"><DesignAssets /></TabsContent>
      </Tabs>
    </div>
    <div role="separator" tabIndex={0} aria-label="Resize layers" aria-orientation="horizontal" aria-valuemin={120} aria-valuemax={Math.max(120, (dock.current?.clientHeight ?? 700) - 160)} aria-valuenow={layerHeight}
      className="studio-layers-resizer" onKeyDown={(event) => { if (event.key === "ArrowUp" || event.key === "ArrowDown") { event.preventDefault(); changeLayers(layerHeight + (event.key === "ArrowUp" ? 24 : -24)); } }}
      onPointerDown={(event) => event.currentTarget.setPointerCapture(event.pointerId)}
      onPointerMove={(event) => { if (event.currentTarget.hasPointerCapture(event.pointerId)) changeLayers((dock.current?.getBoundingClientRect().bottom ?? window.innerHeight) - event.clientY); }}
      onPointerUp={(event) => { if (event.currentTarget.hasPointerCapture(event.pointerId)) event.currentTarget.releasePointerCapture(event.pointerId); }} />
    <section aria-label="Document layers" className="studio-layers" style={{ height: layerHeight }}><Outline /></section>
  </aside>;
}

export default function App() {
  const s = useStudio();
  const render = useRenderStats();
  const paint = usePaint();
  const openInput = useRef<HTMLInputElement>(null);
  const exportMenu = useRef<HTMLDetailsElement>(null);
  const [mobilePanelsOpen, setMobilePanelsOpen] = useState(false);
  const [showWelcome, setShowWelcome] = useState(true);
  const [panelsCollapsed, setPanelsCollapsed] = useState(false);
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
      const target = event.target as HTMLElement;
      const tag = target.tagName;
      if (tag === "INPUT" || tag === "TEXTAREA" || tag === "SELECT" || target.isContentEditable) return;
      const key = event.key.toLowerCase();
      const state = useStudio.getState();
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

  const confirmDiscard = () => !s.dirty || window.confirm("Discard unsaved changes?");
  const pages = pagesOf(s.doc);
  const pageIndex = pages.findIndex((p) => p.id === s.pageId);
  const stage = s.workspaceId === "photo" ? "photo" : s.workspaceId === "paint" ? "paint" : s.workspaceId === "3d" ? "3d" : "canvas";
  const selectedNode = s.doc ? findNode(pageOf(s.doc, s.pageId), s.selectedId)?.node ?? null : null;
  const hasSelectedDieline = selectedNode?.kind === "scene3d" && typeof selectedNode.dieline === "string";
  const activeWorkflow = workflowFor(s.workspaceId);
  const downloadLayout = (format: "pdf" | "epub") => {
    exportMenu.current?.removeAttribute("open");
    if (!s.doc) return;
    void layoutOps.download(s.doc, format).catch((error) => s.notify(error instanceof Error ? error.message : `Could not export ${format.toUpperCase()}.`, true));
  };
  return (
    <TooltipProvider>
      <input
        ref={openInput}
        hidden
        type="file"
        accept=".pen,application/json"
        onChange={async (e) => {
          const file = e.target.files?.[0];
          e.target.value = "";
          if (!file || !confirmDiscard()) return;
          try {
            s.openDoc(JSON.parse(await file.text()));
            setShowWelcome(false);
          } catch {
            s.notify("That file is not a valid .pen document.", true);
          }
        }}
      />
      {showWelcome ? (
        <Welcome
          busy={s.busy}
          hasDocument={Boolean(s.doc)}
          version={s.version}
          onCreate={(width, height) => {
            if (!confirmDiscard()) return;
            void s.newDoc(width, height);
            setShowWelcome(false);
          }}
          onOpen={() => openInput.current?.click()}
          onContinue={() => setShowWelcome(false)}
        />
      ) : (
      <div className="studio-shell flex h-full flex-col bg-background text-foreground">
        <ExtensionPalette />
        <header className="studio-appbar flex h-12 shrink-0 items-center gap-2 border-b bg-card px-2 sm:px-3">
          <Tip label="Start screen">
            <Button variant="ghost" size="icon" className="size-10 shrink-0" aria-label="Start screen" onClick={() => setShowWelcome(true)}>
              <Home size={15} />
            </Button>
          </Tip>
          <div className="studio-brand shrink-0"><strong>Pentool Studio</strong></div>
          {s.version && <Badge className="shrink-0 px-1.5 text-[10px]" title="Pentool version" aria-label={`Pentool version ${s.version}`}>v{s.version}</Badge>}
          <select aria-label="Workspace" title={activeWorkflow.label} className="studio-header-select" value={s.workspaceId} onChange={(event) => s.setWorkspace(event.target.value)}>
            {WORKFLOWS.map((workflow) => <optgroup key={workflow.id} label={workflow.label}>
              {workflow.modes.map((mode) => <option key={mode.id} value={mode.id}>{mode.id === "design" ? "Design" : mode.id === "paint" ? "Paint" : mode.id === "3d" ? "Packaging" : mode.label}</option>)}
            </optgroup>)}
          </select>
          <select aria-label="Page" className="studio-header-select studio-header-page" value={s.pageId ?? ""} onChange={(event) => s.setPage(event.target.value)}>
            {pages.map((page) => <option key={page.id} value={page.id}>{page.name ?? page.id}</option>)}
          </select>
          {stage === "canvas" && s.tool !== "select" && s.tool !== "hand" && <div className="studio-brush-options" aria-label="Drawing options">
            <label>Fill <input aria-label="New object fill" type="color" value={s.look.fill.startsWith("#") ? s.look.fill : "#000000"} onChange={(event) => s.setLook({ fill: event.target.value })} /></label>
            <label>Stroke <input aria-label="New object stroke" type="color" value={s.look.stroke.startsWith("#") ? s.look.stroke : "#000000"} onChange={(event) => s.setLook({ stroke: event.target.value })} /></label>
            <label>Width <input aria-label="New object stroke width" type="number" min={0} max={1000} value={s.look.strokeWidth} onChange={(event) => s.setLook({ strokeWidth: Math.max(0, Math.min(1000, Number(event.target.value) || 0)) })} /></label>
          </div>}
          {stage === "paint" && <div className="studio-brush-options" aria-label="Brush options">
            <label>Size <input aria-label="Brush size" type="number" min={1} max={200} value={paint.size} onChange={(event) => paint.set({ size: Math.max(1, Math.min(200, Number(event.target.value) || 1)) })} /></label>
            <label>Flow <input aria-label="Brush flow" type="number" min={5} max={100} value={Math.round(paint.flow * 100)} onChange={(event) => paint.set({ flow: Math.max(0.05, Math.min(1, Number(event.target.value) / 100)) })} />%</label>
            <input aria-label="Paint color" type="color" value={paint.color} onChange={(event) => paint.set({ color: event.target.value })} />
          </div>}

          <span className="min-w-0 flex-1 truncate text-xs text-muted-foreground" title={String(s.doc?.name ?? "Untitled")}>{String(s.doc?.name ?? "Untitled")}{s.dirty ? " · unsaved" : ""}</span>
          <div className="studio-app-actions hidden shrink-0 items-center gap-0.5 md:flex">
            <Tip label="New document"><Button variant="ghost" size="icon" className="size-9" aria-label="New document" onClick={() => confirmDiscard() && s.newDoc()}><FilePlus2 size={15} /></Button></Tip>
            <Tip label="Open .pen file"><Button variant="ghost" size="icon" className="size-9" aria-label="Open .pen file" onClick={() => openInput.current?.click()}><FolderOpen size={15} /></Button></Tip>
          </div>
          <span className="mx-1 hidden h-6 w-px bg-border sm:block" />
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
          <div className="studio-stage-area">
            <KeepStage active={stage === "canvas"} label="Design canvas"><Canvas active={stage === "canvas"} /></KeepStage>
            <KeepStage active={stage === "paint"} label="Paint canvas"><RasterStage active={stage === "paint"} /></KeepStage>
            <KeepStage active={stage === "photo"} label="Photo development"><PhotoStage active={stage === "photo"} /></KeepStage>
            <KeepStage active={stage === "3d"} label="Packaging canvas">
              {hasSelectedDieline ? <PackagingWorkspaceStage active={stage === "3d"} /> : <UvWorkspaceStage active={stage === "3d"} />}
            </KeepStage>
          </div>
          <Dock mobileOpen={mobilePanelsOpen} collapsed={panelsCollapsed} onCollapse={() => { setPanelsCollapsed(true); setMobilePanelsOpen(false); }} />
        </div>

        {/* Status bar. */}
        <footer className="flex h-6 shrink-0 items-center gap-3 border-t bg-card px-3 text-[11px] text-muted-foreground">
          <span className="min-w-0 flex-1 truncate">{stage === "canvas" ? HINTS[s.tool] : stage === "paint" ? (paint.busy ? "Applying strokes…" : paint.status) : stage === "3d" ? "Create a package or select a model to place artwork." : ""}</span>
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
          <span className="tabular-nums">{Math.round(s.zoom * 100)}%</span>
          <span className="tabular-nums">{pages.length ? `Page ${pageIndex + 1} of ${pages.length}` : "No pages"}</span>
        </footer>
      </div>
      )}
    </TooltipProvider>
  );
}
