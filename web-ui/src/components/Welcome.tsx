import { useState } from "react";
import { ArrowRight, BookOpen, FilePlus2, FolderOpen, Image, Layers3, Printer, Sparkles } from "lucide-react";
import { cn } from "@/lib/utils";

const sizes = [
  { id: "square", label: "Social square", detail: "1080 × 1080 px", width: 1080, height: 1080, icon: Image },
  { id: "poster", label: "Poster", detail: "1080 × 1350 px", width: 1080, height: 1350, icon: Sparkles },
  { id: "screen", label: "Screen", detail: "1920 × 1080 px", width: 1920, height: 1080, icon: Layers3 },
  { id: "a4", label: "A4 page", detail: "794 × 1123 px", width: 794, height: 1123, icon: Printer },
] as const;

type WelcomeProps = {
  busy: boolean;
  hasDocument: boolean;
  onCreate: (width: number, height: number) => void;
  onOpen: () => void;
  onContinue: () => void;
};

export function Welcome({ busy, hasDocument, onCreate, onOpen, onContinue }: WelcomeProps) {
  const [selected, setSelected] = useState<string>(sizes[0].id);
  const preset = sizes.find((size) => size.id === selected) ?? sizes[0];
  const [width, setWidth] = useState<number>(preset.width);
  const [height, setHeight] = useState<number>(preset.height);
  const choose = (id: string) => {
    setSelected(id);
    const next = sizes.find((size) => size.id === id);
    if (next) {
      setWidth(next.width);
      setHeight(next.height);
    }
  };

  return (
    <main className="welcome-screen" aria-labelledby="welcome-title">
      <div className="welcome-topline"><span className="welcome-mark">P</span><span>Pentool Studio</span><span className="welcome-version">Creative workspace</span></div>
      <div className="welcome-content">
        <section className="welcome-intro">
          <p className="welcome-eyebrow">A workspace for the whole project</p>
          <h1 id="welcome-title">Make the pieces.<br /><span>Bring them together.</span></h1>
          <p className="welcome-description">Design, imagery, paint, page layout and packaging share one editable document. Start with a canvas or pick up where you left off.</p>
          {hasDocument && (
            <button className="welcome-continue" onClick={onContinue} disabled={busy}>
              <span className="welcome-action-icon"><BookOpen size={18} /></span>
              <span className="welcome-action-copy"><strong>Continue working</strong><small>Open the current project in Studio</small></span>
              <ArrowRight size={17} />
            </button>
          )}
          <button className="welcome-open" onClick={onOpen} disabled={busy}>
            <FolderOpen size={16} /> Open an existing .pen project
          </button>
        </section>

        <section className="welcome-create" aria-labelledby="create-heading">
          <div className="welcome-create-heading"><div><p className="welcome-eyebrow">New project</p><h2 id="create-heading">Choose a starting canvas</h2></div><FilePlus2 size={19} /></div>
          <div className="welcome-presets" role="group" aria-label="Canvas presets">
            {sizes.map((size) => (
              <button key={size.id} className={cn("welcome-preset", selected === size.id && "is-selected")} aria-pressed={selected === size.id} onClick={() => choose(size.id)}>
                <span className="welcome-preset-icon"><size.icon size={17} /></span><span><strong>{size.label}</strong><small>{size.detail}</small></span>
              </button>
            ))}
          </div>
          <div className="welcome-custom">
            <span>Custom size</span>
            <label>Width<input aria-label="Canvas width in pixels" type="number" min={1} max={10000} value={width} onChange={(event) => { setSelected("custom"); setWidth(Number(event.target.value)); }} /></label>
            <span className="welcome-times">×</span>
            <label>Height<input aria-label="Canvas height in pixels" type="number" min={1} max={10000} value={height} onChange={(event) => { setSelected("custom"); setHeight(Number(event.target.value)); }} /></label>
          </div>
          <button className="welcome-create-button" disabled={busy || width < 1 || height < 1 || width > 10000 || height > 10000} onClick={() => onCreate(width, height)}>
            <FilePlus2 size={16} /> Create canvas <ArrowRight size={16} />
          </button>
          <p className="welcome-note">You can add vector artwork, photos, paint layers, layouts and 3D packaging to the same project.</p>
        </section>
      </div>
      <footer className="welcome-footer"><span>Offline-first · Your project stays in your hands</span><span>Open formats · Editable by people and agents</span></footer>
    </main>
  );
}
