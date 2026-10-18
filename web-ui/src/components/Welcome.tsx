import { useEffect, useState, type DragEvent } from "react";
import { ArrowLeftRight, ArrowRight, FileUp, FolderOpen, Plus } from "lucide-react";
import { api, type Doc } from "@/lib/api";
import { isScene, pagesOf } from "@/lib/model";
import { cn } from "@/lib/utils";

type Category = "social" | "screen" | "print";
const presets: { id: string; label: string; width: number; height: number; category: Category }[] = [
  { id: "post", label: "Social post", width: 1080, height: 1080, category: "social" },
  { id: "portrait", label: "Portrait post", width: 1080, height: 1350, category: "social" },
  { id: "story", label: "Story", width: 1080, height: 1920, category: "social" },
  { id: "thumb", label: "Video thumbnail", width: 1280, height: 720, category: "social" },
  { id: "desktop", label: "Desktop", width: 1440, height: 1024, category: "screen" },
  { id: "slide", label: "Presentation", width: 1920, height: 1080, category: "screen" },
  { id: "phone", label: "Phone", width: 390, height: 844, category: "screen" },
  { id: "tablet", label: "Tablet", width: 834, height: 1194, category: "screen" },
  { id: "a4", label: "A4", width: 794, height: 1123, category: "print" },
  { id: "letter", label: "US Letter", width: 816, height: 1056, category: "print" },
  { id: "poster", label: "Poster 18×24", width: 1728, height: 2304, category: "print" },
  { id: "card", label: "Business card", width: 1050, height: 600, category: "print" },
];
const tabs: { id: "all" | Category; label: string }[] = [
  { id: "all", label: "All" },
  { id: "social", label: "Social" },
  { id: "screen", label: "Screen" },
  { id: "print", label: "Print" },
];

type WelcomeProps = {
  busy: boolean;
  doc: Doc | null;
  dirty: boolean;
  version: string | null;
  onCreate: (width: number, height: number) => void;
  onOpen: () => void;
  onOpenFile: (file: File) => void;
  onContinue: () => void;
};

/** A page outline drawn to the preset's proportions, like a document picker. */
function Sheet({ width, height, box = 64 }: { width: number; height: number; box?: number }) {
  const k = box / Math.max(width, height);
  return <span className="welcome-sheet" style={{ width: Math.max(6, width * k), height: Math.max(6, height * k) }} />;
}

/** First-page thumbnail of the open project, rendered like the page strip. */
function useThumbnail(doc: Doc | null): string | null {
  const [url, setUrl] = useState<string | null>(null);
  useEffect(() => {
    if (!doc) return;
    let gone = false;
    let made: string | null = null;
    const page = pagesOf(doc)[0];
    api
      .renderPng(doc, isScene(doc) ? page?.id : undefined, 480)
      .then((blob) => {
        if (gone) return;
        made = URL.createObjectURL(blob);
        setUrl(made);
      })
      .catch(() => undefined);
    return () => {
      gone = true;
      if (made) URL.revokeObjectURL(made);
      setUrl(null);
    };
  }, [doc]);
  return url;
}

export function Welcome({ busy, doc, dirty, version, onCreate, onOpen, onOpenFile, onContinue }: WelcomeProps) {
  const [tab, setTab] = useState<"all" | Category>("all");
  const [width, setWidth] = useState(1200);
  const [height, setHeight] = useState(800);
  const [dropping, setDropping] = useState(false);
  const thumb = useThumbnail(doc);
  const shown = presets.filter((p) => tab === "all" || p.category === tab);
  const valid = width >= 1 && height >= 1 && width <= 10000 && height <= 10000;
  const pages = doc ? pagesOf(doc).length : 0;
  const canvas = doc ? pagesOf(doc)[0]?.canvas : undefined;

  const drop = (event: DragEvent) => {
    event.preventDefault();
    setDropping(false);
    const file = event.dataTransfer.files[0];
    if (file) onOpenFile(file);
  };

  return (
    <main
      className={cn("welcome-screen", dropping && "is-dropping")}
      aria-labelledby="welcome-title"
      onDragOver={(e) => {
        if (!e.dataTransfer.types.includes("Files")) return;
        e.preventDefault();
        setDropping(true);
      }}
      onDragLeave={(e) => {
        if (e.currentTarget === e.target) setDropping(false);
      }}
      onDrop={drop}
    >
      <header className="welcome-topline">
        <span className="welcome-mark" aria-hidden>P</span>
        <span>Pentool Studio</span>
        {version && <span className="welcome-version">v{version}</span>}
        <button className="welcome-ghost" onClick={onOpen} disabled={busy}>
          <FolderOpen size={15} /> Open file
        </button>
      </header>

      <div className="welcome-body">
        <section aria-labelledby="welcome-title">
          <div className="welcome-section-head">
            <h1 id="welcome-title">Start a new design</h1>
            <div className="welcome-tabs" role="tablist" aria-label="Preset categories">
              {tabs.map((t) => (
                <button key={t.id} role="tab" aria-selected={tab === t.id} className={cn(tab === t.id && "is-active")} onClick={() => setTab(t.id)}>
                  {t.label}
                </button>
              ))}
            </div>
            <form
              className="welcome-custom"
              aria-label="Custom size"
              onSubmit={(e) => {
                e.preventDefault();
                if (valid) onCreate(width, height);
              }}
            >
              <span className="welcome-custom-label">Custom</span>
              <input aria-label="Canvas width in pixels" type="number" min={1} max={10000} value={width} onChange={(e) => setWidth(Number(e.target.value))} />
              <button type="button" className="welcome-swap" aria-label="Swap width and height" title="Swap orientation" onClick={() => { setWidth(height); setHeight(width); }}>
                <ArrowLeftRight size={12} />
              </button>
              <input aria-label="Canvas height in pixels" type="number" min={1} max={10000} value={height} onChange={(e) => setHeight(Number(e.target.value))} />
              <span className="welcome-custom-unit">px</span>
              <button type="submit" className="welcome-create-button" disabled={busy || !valid}>
                <Plus size={14} /> Create
              </button>
            </form>
          </div>
          <div className="welcome-presets" role="group" aria-label="Canvas presets">
            {shown.map((p) => (
              <button key={p.id} className="welcome-preset" disabled={busy} onClick={() => onCreate(p.width, p.height)} aria-label={`New ${p.label}, ${p.width} by ${p.height} pixels`}>
                <span className="welcome-preset-art"><Sheet width={p.width} height={p.height} /></span>
                <strong>{p.label}</strong>
                <small>{p.width} × {p.height} px</small>
              </button>
            ))}
          </div>
        </section>

        <section aria-labelledby="recent-heading">
          <div className="welcome-section-head"><h2 id="recent-heading">Continue</h2></div>
          <div className="welcome-recent">
            {doc && (
              <button className="welcome-file" onClick={onContinue} disabled={busy}>
                <span className="welcome-file-thumb">
                  {thumb ? <img src={thumb} alt="" /> : <Sheet width={canvas?.width ?? 4} height={canvas?.height ?? 3} box={96} />}
                </span>
                <span className="welcome-file-meta">
                  <strong>{String(doc.name ?? "Untitled")}{dirty && <em> · unsaved</em>}</strong>
                  <small>
                    {pages} {pages === 1 ? "page" : "pages"}
                    {canvas ? ` · ${canvas.width} × ${canvas.height}` : ""} · format v{doc.version}
                  </small>
                </span>
                <span className="welcome-file-go">Open <ArrowRight size={14} /></span>
              </button>
            )}
            <button className="welcome-drop" onClick={onOpen} disabled={busy}>
              <FileUp size={22} />
              <strong>Open a .pen file</strong>
              <small>or drop it anywhere on this screen</small>
            </button>
          </div>
        </section>
      </div>

      <footer className="welcome-footer">
        <span>Works offline · No account needed</span>
        <span>Open, readable .pen format</span>
      </footer>
      {dropping && <div className="welcome-dropzone" aria-hidden><FileUp size={28} /> Drop to open</div>}
    </main>
  );
}
