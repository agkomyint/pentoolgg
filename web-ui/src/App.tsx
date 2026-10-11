import { useEffect, useRef } from "react";
import { Download, FilePlus2, FolderOpen, Moon, Redo2, RefreshCw, Save, Sun, Undo2 } from "lucide-react";
import { useStudio } from "@/store";
import { workspaces } from "@/lib/workspaces";
import { Button } from "@/components/ui/button";
import { Badge } from "@/components/ui/badge";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { Tip, TooltipProvider } from "@/components/ui/tooltip";
import { Canvas } from "@/components/Canvas";
import { Outline } from "@/components/Outline";
import { Properties } from "@/components/Properties";
import { Toolbar, ToolHint } from "@/components/Toolbar";
import { DesignPanel } from "@/workspaces/DesignPanel";
import { CompositePanel, ImagePanel, SceneConsole } from "@/workspaces/OperationsPanel";
import { PrintPanel } from "@/workspaces/PrintPanel";
import { Planned } from "@/workspaces/Planned";
import { PhotoPanel } from "@/workspaces/PhotoPanel";
import { PhotoStage } from "@/workspaces/PhotoStage";
import { RasterPanel } from "@/workspaces/RasterPanel";
import { RasterStage } from "@/workspaces/RasterStage";
import { ImageAdjust } from "@/workspaces/ImageAdjust";
import { findNode, pageOf } from "@/lib/model";
import { exportAs } from "@/lib/actions";
import { cn } from "@/lib/utils";

function WorkspaceBody() {
  const { workspaceId } = useStudio();
  const workspace = workspaces.find((w) => w.id === workspaceId) ?? workspaces[0]!;
  switch (workspace.id) {
    case "design":
      return (
        <div className="space-y-5">
          <DesignPanel />
          <SceneConsole />
        </div>
      );
    case "image":
      return <ImagePanel />;
    case "composite":
      return <CompositePanel />;
    case "print":
      return <PrintPanel />;
    case "photo":
      return <PhotoPanel />;
    case "paint":
      return <RasterPanel />;
    default:
      return <Planned workspace={workspace} />;
  }
}

function Inspector() {
  const { workspaceId, selectedId, doc, pageId } = useStudio();
  const workspace = workspaces.find((w) => w.id === workspaceId) ?? workspaces[0]!;
  const picked = selectedId ? findNode(pageOf(doc, pageId), selectedId)?.node : null;
  const selectedImage = picked?.kind === "image" && doc && doc.version >= 5 ? picked : null;
  return (
    <aside className="flex w-80 shrink-0 flex-col border-l bg-card">
      <Tabs defaultValue={workspaceId === "photo" || workspaceId === "paint" ? "workspace" : "properties"} className="flex min-h-0 flex-1 flex-col" key={`${workspaceId}-${selectedId ? "sel" : "none"}`}>
        <TabsList>
          <TabsTrigger value="properties">Properties</TabsTrigger>
          <TabsTrigger value="outline">Layers</TabsTrigger>
          <TabsTrigger value="workspace">{workspace.label}</TabsTrigger>
        </TabsList>
        <TabsContent value="properties">
          <Properties />
          {selectedImage && (
            <div className="border-t p-3">
              <ImageAdjust key={selectedImage.id} node={selectedImage} />
            </div>
          )}
        </TabsContent>
        <TabsContent value="outline">
          <Outline />
        </TabsContent>
        <TabsContent value="workspace">
          <WorkspaceBody />
        </TabsContent>
      </Tabs>
    </aside>
  );
}

export default function App() {
  const s = useStudio();
  const openInput = useRef<HTMLInputElement>(null);
  useEffect(() => {
    void s.load();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);
  useEffect(() => {
    document.documentElement.classList.toggle("dark", s.dark);
  }, [s.dark]);
  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (!(event.ctrlKey || event.metaKey)) return;
      const tag = (event.target as HTMLElement).tagName;
      if (tag === "INPUT" || tag === "TEXTAREA" || tag === "SELECT") return;
      const key = event.key.toLowerCase();
      if (key === "s") {
        event.preventDefault();
        void s.save();
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [s]);
  useEffect(() => {
    const warn = (event: BeforeUnloadEvent) => {
      if (useStudio.getState().dirty) event.preventDefault();
    };
    window.addEventListener("beforeunload", warn);
    return () => window.removeEventListener("beforeunload", warn);
  }, []);

  const confirmDiscard = () => !s.dirty || window.confirm("Discard unsaved changes?");
  const pages = s.doc?.pages ?? [];
  return (
    <TooltipProvider>
      <div className="flex h-full">
        <nav aria-label="Workspaces" className="flex w-14 shrink-0 flex-col items-center gap-1 border-r bg-card py-2">
          {workspaces.map((w) => (
            <Tip key={w.id} label={`${w.label} — ${w.status === "planned" ? `coming soon (${w.since})` : w.status}`}>
              <button
                aria-label={w.label}
                aria-current={s.workspaceId === w.id}
                onClick={() => s.setWorkspace(w.id)}
                className={cn(
                  "flex h-10 w-10 flex-col items-center justify-center rounded-md text-muted-foreground hover:bg-accent",
                  s.workspaceId === w.id && "bg-accent text-foreground",
                  w.status === "planned" && "opacity-60",
                )}
              >
                <w.icon size={18} />
                <span className="text-[9px] leading-none">{w.label}</span>
              </button>
            </Tip>
          ))}
        </nav>
        <div className="flex min-w-0 flex-1 flex-col">
          <header className="flex h-11 shrink-0 items-center gap-2 border-b bg-card px-3">
            <strong>Pentool Studio</strong>
            {s.version && <Badge>v{s.version}</Badge>}
            <span className="text-xs text-muted-foreground">
              {String(s.doc?.name ?? "")}
              {s.dirty ? " •" : ""}
              {s.doc && !s.shared ? " (standalone)" : ""}
            </span>
            <select aria-label="Page" className="ml-3 h-7 rounded border bg-card px-1.5" value={s.pageId ?? ""} onChange={(e) => s.setPage(e.target.value)}>
              {pages.map((p) => (
                <option key={p.id} value={p.id}>
                  {p.name ?? p.id}
                </option>
              ))}
            </select>
            <div className="ml-auto flex items-center gap-1">
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
                  } catch {
                    s.notify("That file is not a valid .pen document.", true);
                  }
                }}
              />
              <Tip label="Save (Ctrl+S)">
                <Button variant="ghost" size="icon" aria-label="Save" onClick={() => void s.save()}>
                  <Save size={15} />
                </Button>
              </Tip>
              <Tip label="Export PNG">
                <Button variant="ghost" size="icon" aria-label="Export PNG" onClick={() => void exportAs("png")}>
                  <Download size={15} />
                </Button>
              </Tip>
              <Button variant="ghost" size="sm" onClick={() => void exportAs("svg")}>
                SVG
              </Button>
              <span className="mx-1 h-5 w-px bg-border" />
              <Button variant="ghost" size="icon" aria-label="Undo" onClick={s.undo} disabled={!s.past.length}>
                <Undo2 size={15} />
              </Button>
              <Button variant="ghost" size="icon" aria-label="Redo" onClick={s.redo} disabled={!s.future.length}>
                <Redo2 size={15} />
              </Button>
              <Button variant="ghost" size="icon" aria-label="Reload from file" onClick={() => confirmDiscard() && void s.load()} disabled={s.busy}>
                <RefreshCw size={15} />
              </Button>
              <Button variant="ghost" size="icon" aria-label="Toggle theme" onClick={s.toggleDark}>
                {s.dark ? <Sun size={15} /> : <Moon size={15} />}
              </Button>
            </div>
          </header>
          {s.message && (
            <div role="status" className={cn("px-3 py-1 text-xs", s.message.error ? "bg-destructive/15 text-destructive" : "bg-accent")}>
              {s.message.text}
            </div>
          )}
          <div className="flex min-h-0 flex-1">
            {s.workspaceId === "photo" ? (
              <PhotoStage />
            ) : s.workspaceId === "paint" ? (
              <RasterStage />
            ) : (
              <>
                <Toolbar />
                <main className="flex min-w-0 flex-1 flex-col">
                  <Canvas />
                  <ToolHint />
                </main>
              </>
            )}
            <Inspector />
          </div>
        </div>
      </div>
    </TooltipProvider>
  );
}
