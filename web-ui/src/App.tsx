import { useEffect, useRef, useState } from "react";
import { Download, FilePlus2, FolderOpen, Home, Moon, Redo2, RefreshCw, Save, Sun, Undo2 } from "lucide-react";
import { LATEST_FORMAT, collectIds, useStudio } from "@/store";
import { workspaces } from "@/lib/workspaces";
import { Button } from "@/components/ui/button";
import { Badge } from "@/components/ui/badge";
import { Section } from "@/components/ui/kit";
import { Tip, TooltipProvider } from "@/components/ui/tooltip";
import { Canvas } from "@/components/Canvas";
import { Outline } from "@/components/Outline";
import { Properties } from "@/components/Properties";
import { Toolbar } from "@/components/Toolbar";
import { DesignPanel } from "@/workspaces/DesignPanel";
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
import { findNode, pageOf } from "@/lib/model";
import { exportAs } from "@/lib/actions";
import { cn } from "@/lib/utils";
import { useRenderStats } from "@/lib/wasmCore";
import { Welcome } from "@/components/Welcome";

const HINTS = {
  select: "Click to select, drag to move, drag handles to resize (Shift keeps proportions). Arrow keys nudge.",
  pen: "Click to add points, drag for curves, click the first point to close, Enter to finish, Esc to cancel.",
  rect: "Drag to draw a rectangle (Shift for a square).",
  ellipse: "Drag to draw an ellipse (Shift for a circle).",
  line: "Drag to draw a line.",
  text: "Click the page to place text, then edit it in Properties.",
  hand: "Drag to pan. Ctrl + wheel zooms.",
} as const;

function WorkspaceBody() {
  const { workspaceId } = useStudio();
  const workspace = workspaces.find((w) => w.id === workspaceId) ?? workspaces[0]!;
  switch (workspace.id) {
    case "design":
      return (
        <div className="space-y-4 p-2">
          <DesignPanel />
          <SceneConsole />
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

/** Right dock: stacked, collapsible panels (Properties, Layers, module). */
function Dock({ mobileOpen }: { mobileOpen: boolean }) {
  const { workspaceId, selectedId, doc, pageId } = useStudio();
  const workspace = workspaces.find((w) => w.id === workspaceId) ?? workspaces[0]!;
  const picked = selectedId ? findNode(pageOf(doc, pageId), selectedId)?.node : null;
  const selectedImage = picked?.kind === "image" && doc && doc.version >= 5 ? picked : null;
  const moduleFirst = workspaceId === "photo" || workspaceId === "paint" || workspaceId === "layout";
  const module = (
    <Section key="module" id={`dock-${workspace.id}`} title={workspace.label}>
      <div className="-mx-2 -my-1.5">
        <WorkspaceBody />
      </div>
    </Section>
  );
  return (
    <aside id="mobile-panels" aria-label="Panels" className={cn(
      "w-full shrink-0 flex-col overflow-y-auto border-t bg-card lg:flex lg:w-72 lg:border-l lg:border-t-0",
      mobileOpen ? "flex max-h-[45vh]" : "hidden",
    )}>
      {moduleFirst && module}
      <Section id="dock-properties" title="Properties">
        <div className="-mx-2 -my-1.5">
          <Properties />
          {selectedImage && (
            <div className="border-t p-2">
              <ImageAdjust key={selectedImage.id} node={selectedImage} />
            </div>
          )}
        </div>
      </Section>
      <Section id="dock-layers" title="Layers" defaultOpen={!moduleFirst}>
        <div className="-mx-2 -my-1.5">
          <Outline />
        </div>
      </Section>
      {!moduleFirst && module}
    </aside>
  );
}

export default function App() {
  const s = useStudio();
  const render = useRenderStats();
  const openInput = useRef<HTMLInputElement>(null);
  const [mobilePanelsOpen, setMobilePanelsOpen] = useState(false);
  const [showWelcome, setShowWelcome] = useState(true);
  useEffect(() => {
    void s.load();
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
        } else if (key === "p" && event.shiftKey && useLayout.getState().indesignKeys && state.doc) {
          event.preventDefault();
          const doc = state.doc;
          const current = doc.pages.find((p) => p.id === state.pageId);
          const id = nextId("page", collectIds(doc));
          void useLayout
            .getState()
            .run({ type: "page-add", id, after: current?.id, parent: pageParent(current) }, "Add page")
            .then((ok) => ok && useStudio.getState().setPage(id));
        }
        return;
      }
      if (!useLayout.getState().indesignKeys || event.altKey) return;
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
  const pages = s.doc?.pages ?? [];
  const pageIndex = pages.findIndex((p) => p.id === s.pageId);
  const stage = s.workspaceId === "photo" ? "photo" : s.workspaceId === "paint" ? "paint" : s.workspaceId === "3d" ? "3d" : "canvas";
  const selectedNode = s.doc ? findNode(pageOf(s.doc, s.pageId), s.selectedId)?.node ?? null : null;
  const hasSelectedDieline = stage === "3d" && selectedNode?.kind === "scene3d" && typeof selectedNode.dieline === "string";
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
          onCreate={(width, height) => {
            if (!confirmDiscard()) return;
            void s.newDoc(width, height);
            setShowWelcome(false);
          }}
          onOpen={() => openInput.current?.click()}
          onContinue={() => setShowWelcome(false)}
        />
      ) : (
      <div className="flex h-full flex-col bg-background text-foreground">
        <ExtensionPalette />
        {/* Application bar: file actions, module switcher, history. */}
        <header className="flex h-10 shrink-0 items-center gap-1 border-b bg-card px-2">
          <Tip label="Start screen">
            <Button variant="ghost" size="icon" aria-label="Start screen" onClick={() => setShowWelcome(true)}>
              <Home size={15} />
            </Button>
          </Tip>
          <strong className="mr-1 text-[13px] tracking-tight">Pentool</strong>
          <Tip label="New document">
            <Button variant="ghost" size="icon" aria-label="New" onClick={() => confirmDiscard() && s.newDoc()}>
              <FilePlus2 size={15} />
            </Button>
          </Tip>
          <Tip label="Open .pen file">
            <Button variant="ghost" size="icon" aria-label="Open" onClick={() => openInput.current?.click()}>
              <FolderOpen size={15} />
            </Button>
          </Tip>
          <Tip label="Save (Ctrl+S)">
            <Button variant="ghost" size="icon" aria-label="Save" onClick={() => void s.save()}>
              <Save size={15} />
            </Button>
          </Tip>
          <span className="mx-1 h-5 w-px bg-border" />
          <Tip label="Undo">
            <Button variant="ghost" size="icon" aria-label="Undo" onClick={s.undo} disabled={!s.past.length}>
              <Undo2 size={15} />
            </Button>
          </Tip>
          <Tip label="Redo">
            <Button variant="ghost" size="icon" aria-label="Redo" onClick={s.redo} disabled={!s.future.length}>
              <Redo2 size={15} />
            </Button>
          </Tip>

          <nav aria-label="Modules" className="mx-auto flex min-w-0 flex-1 items-center justify-center gap-0.5 overflow-x-auto">
            {workspaces.map((w) => (
              <Tip key={w.id} label={`${w.label} — ${w.status === "planned" ? `coming soon (${w.since})` : w.status}`}>
                <button
                  aria-label={w.label}
                  aria-current={s.workspaceId === w.id ? "page" : undefined}
                  title={w.label}
                  onClick={() => s.setWorkspace(w.id)}
                  className={cn(
                    "flex h-10 w-10 shrink-0 items-center justify-center gap-1.5 rounded px-2 text-[12px] text-muted-foreground hover:text-foreground lg:h-7 lg:w-auto",
                    s.workspaceId === w.id && "bg-accent text-foreground",
                    w.status === "planned" && "opacity-50",
                  )}
                >
                  <w.icon size={14} />
                  <span className="hidden lg:inline">{w.label}</span>
                </button>
              </Tip>
            ))}
          </nav>

          <Tip label="Export PNG">
            <Button variant="ghost" size="icon" aria-label="Export PNG" onClick={() => void exportAs("png")}>
              <Download size={15} />
            </Button>
          </Tip>
          <Button variant="ghost" size="sm" onClick={() => void exportAs("svg")}>
            SVG
          </Button>
          <Tip label="Reload from file">
            <Button variant="ghost" size="icon" aria-label="Reload from file" onClick={() => confirmDiscard() && void s.load()} disabled={s.busy}>
              <RefreshCw size={15} />
            </Button>
          </Tip>
          <Tip label="Toggle theme">
            <Button variant="ghost" size="icon" aria-label="Toggle theme" onClick={s.toggleDark}>
              {s.dark ? <Sun size={15} /> : <Moon size={15} />}
            </Button>
          </Tip>
        </header>

        {/* Options bar: document, page and format context. */}
        <div className="flex h-8 shrink-0 items-center gap-2 border-b bg-card/60 px-3 text-[12px]">
          <span className="max-w-56 truncate font-medium">{String(s.doc?.name ?? "Untitled")}</span>
          {s.dirty && <span className="text-warning">unsaved</span>}
          {s.doc && !s.shared && <span className="hidden text-muted-foreground sm:inline">standalone</span>}
          <span className="mx-1 h-4 w-px bg-border" />
          <select
            aria-label="Page"
            className="h-6 rounded border bg-input px-1.5 text-[12px]"
            value={s.pageId ?? ""}
            onChange={(e) => s.setPage(e.target.value)}
          >
            {pages.map((p) => (
              <option key={p.id} value={p.id}>
                {p.name ?? p.id}
              </option>
            ))}
          </select>
          <Button
            type="button"
            variant="outline"
            size="sm"
            className="min-h-10 focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2 lg:hidden"
            aria-expanded={mobilePanelsOpen}
            aria-controls="mobile-panels"
            onClick={() => setMobilePanelsOpen((open) => !open)}
          >
            {mobilePanelsOpen ? "Hide panels" : "Panels"}
          </Button>
          {s.doc && (
            <Tip
              label={
                s.doc.version < LATEST_FORMAT
                  ? `Document format v${s.doc.version}. Upgrade to v${LATEST_FORMAT} for current 3D packaging, layout and production features.`
                  : `Document format v${s.doc.version} (latest)`
              }
            >
              <Button
                variant="outline"
                size="sm"
                className="h-6 shrink-0 px-2 text-[11px]"
                aria-label={s.doc.version >= LATEST_FORMAT
                  ? `Document format v${s.doc.version} (latest)`
                  : `Upgrade document from format v${s.doc.version} to v${LATEST_FORMAT}`}
                disabled={s.doc.version >= LATEST_FORMAT || s.busy}
                onClick={() => {
                  if (
                    window.confirm(
                      `Upgrade this document to format v${LATEST_FORMAT}? Tools older than this release cannot open it. Undo reverts until you save.`,
                    )
                  )
                    void s.upgradeDoc();
                }}
              >
                {s.doc.version >= LATEST_FORMAT
                  ? <><span className="sm:hidden">v{s.doc.version}</span><span className="hidden sm:inline">Format v{s.doc.version}</span></>
                  : <><span className="sm:hidden">v{s.doc.version} ↑</span><span className="hidden sm:inline">Format v{s.doc.version} · Upgrade</span></>}
              </Button>
            </Tip>
          )}
          {s.message && (
            <span role="status" className={cn("ml-auto truncate", s.message.error ? "text-destructive" : "text-muted-foreground")}>
              {s.message.text}
            </span>
          )}
        </div>

        <div className="flex min-h-0 flex-1 flex-col lg:flex-row">
          {stage === "3d" ? (
            hasSelectedDieline ? <PackagingWorkspaceStage /> : <UvWorkspaceStage />
          ) : stage === "photo" ? (
            <PhotoStage />
          ) : stage === "paint" ? (
            <RasterStage />
          ) : (
            <>
              <Toolbar />
              <main className="flex min-w-0 flex-1 flex-col">
                <Canvas />
              </main>
            </>
          )}
          <Dock mobileOpen={mobilePanelsOpen} />
        </div>

        {/* Status bar. */}
        <footer className="flex h-6 shrink-0 items-center gap-3 border-t bg-card px-3 text-[11px] text-muted-foreground">
          <span className="min-w-0 flex-1 truncate">{stage === "canvas" ? HINTS[s.tool] : "Module stage"}</span>
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
              className={cn("rounded px-1.5 hover:text-foreground", render.enabled && render.available !== false && "text-primary")}
            >
              Local render {render.available === false ? "n/a" : render.enabled ? "on" : "off"}
            </button>
          </Tip>
          {render.last && (
            <span className="tabular-nums" title="Time to produce the last preview">
              {render.last.source === "wasm" ? "wasm" : "server"} {render.last.ms} ms
            </span>
          )}
          <span className="tabular-nums">{Math.round(s.zoom * 100)}%</span>
          <span className="tabular-nums">{pages.length ? `Page ${pageIndex + 1} of ${pages.length}` : "No pages"}</span>
          {s.version && <Badge>v{s.version}</Badge>}
        </footer>
      </div>
      )}
    </TooltipProvider>
  );
}
