import { useEffect, useRef, useState } from "react";
import { LABELS } from "@/lib/photo";
import { cn } from "@/lib/utils";
import { usePhoto } from "./photoStore";

const stars = (rating: number) => "★".repeat(rating) + "☆".repeat(5 - rating);
const LABEL_COLORS: Record<string, string> = {
  red: "text-red-500",
  yellow: "text-yellow-500",
  green: "text-green-500",
  blue: "text-blue-500",
  purple: "text-purple-500",
};

function Thumb({ id }: { id: string }) {
  const thumbnail = usePhoto((s) => s.thumbnail);
  const generation = usePhoto((s) => s.photos);
  const [url, setUrl] = useState<string | null>(null);
  const ref = useRef<HTMLDivElement>(null);
  useEffect(() => {
    const node = ref.current;
    if (!node) return;
    let cancelled = false;
    const observer = new IntersectionObserver((entries) => {
      if (entries.some((e) => e.isIntersecting)) {
        observer.disconnect();
        void thumbnail(id).then((value) => !cancelled && setUrl(value));
      }
    });
    observer.observe(node);
    return () => {
      cancelled = true;
      observer.disconnect();
    };
  }, [id, thumbnail, generation]);
  return (
    <div ref={ref} className="flex aspect-[3/2] items-center justify-center overflow-hidden rounded bg-muted">
      {url ? <img src={url} alt={id} className="h-full w-full object-contain" draggable={false} /> : <span className="text-[10px] text-muted-foreground">…</span>}
    </div>
  );
}

function Grid() {
  const { photos, selected, current, select, setView } = usePhoto();
  if (!photos.length) return <p className="p-6 text-sm text-muted-foreground">No photos match. Add DNGs with <code>pentool raw add</code>.</p>;
  return (
    <div className="grid grid-cols-[repeat(auto-fill,minmax(170px,1fr))] gap-2 overflow-auto p-3">
      {photos.map((photo) => (
        <button
          key={photo.id}
          type="button"
          aria-pressed={selected.includes(photo.id)}
          title={`${photo.id} · ${photo.width}×${photo.height}`}
          onClick={(e) => select(photo.id, e.shiftKey ? "range" : e.ctrlKey || e.metaKey ? "toggle" : undefined)}
          onDoubleClick={() => setView("loupe")}
          className={cn(
            "flex flex-col gap-1 rounded-md border p-1.5 text-left",
            selected.includes(photo.id) && "border-primary bg-accent",
            photo.id === current && "ring-2 ring-primary",
            photo.pick === "reject" && "opacity-50",
          )}
        >
          <Thumb id={photo.id} />
          <span className="flex items-center justify-between text-[11px]">
            <span>{stars(photo.rating)}</span>
            <span className={LABEL_COLORS[photo.label]}>{photo.pick === "pick" ? "⚑" : photo.pick === "reject" ? "✕" : photo.label !== "none" ? "●" : ""}</span>
          </span>
          <span className="truncate text-[11px] text-muted-foreground">{photo.id}</span>
        </button>
      ))}
    </div>
  );
}

function drawCrop(canvas: HTMLCanvasElement, crop: number[] | null, cropping: boolean) {
  const context = canvas.getContext("2d");
  if (!context) return;
  context.clearRect(0, 0, canvas.width, canvas.height);
  if (!cropping || !crop) return;
  const [x, y, w, h] = crop.map((v, i) => v * (i % 2 ? canvas.height : canvas.width)) as [number, number, number, number];
  context.fillStyle = "rgba(0,0,0,0.55)";
  context.fillRect(0, 0, canvas.width, canvas.height);
  context.clearRect(x, y, w, h);
  context.strokeStyle = "#b8f34a";
  context.lineWidth = 2;
  context.strokeRect(x, y, w, h);
  context.strokeStyle = "rgba(255,255,255,0.4)";
  context.lineWidth = 1;
  for (const t of [1 / 3, 2 / 3]) {
    context.beginPath();
    context.moveTo(x + w * t, y);
    context.lineTo(x + w * t, y + h);
    context.moveTo(x, y + h * t);
    context.lineTo(x + w, y + h * t);
    context.stroke();
  }
}

function Loupe() {
  const s = usePhoto();
  const host = useRef<HTMLDivElement>(null);
  const image = useRef<HTMLImageElement>(null);
  const canvas = useRef<HTMLCanvasElement>(null);
  const [size, setSize] = useState<{ w: number; h: number } | null>(null);
  const points = useRef<[number, number][] | null>(null);
  const start = useRef<[number, number] | null>(null);
  const shot = s.shots[0]?.shot;

  const fit = () => {
    const img = image.current;
    const box = host.current;
    if (!img || !box || !img.naturalWidth) return;
    const scale = Math.min((box.clientWidth - 16) / img.naturalWidth, (box.clientHeight - 16) / img.naturalHeight);
    setSize({ w: Math.max(1, Math.floor(img.naturalWidth * scale)), h: Math.max(1, Math.floor(img.naturalHeight * scale)) });
  };

  useEffect(() => {
    const box = host.current;
    if (!box) return;
    let timer: ReturnType<typeof setTimeout>;
    const observer = new ResizeObserver(() => {
      s.set({ box: { w: box.clientWidth, h: box.clientHeight } });
      fit();
      clearTimeout(timer);
      timer = setTimeout(() => void s.run(() => usePhoto.getState().renderLoupe()), 300);
    });
    observer.observe(box);
    return () => {
      clearTimeout(timer);
      observer.disconnect();
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  useEffect(() => {
    if (canvas.current && size) drawCrop(canvas.current, s.crop, s.cropping);
  }, [s.crop, s.cropping, size, shot]);

  const at = (event: React.PointerEvent): [number, number] => {
    const box = canvas.current!.getBoundingClientRect();
    return [Math.min(1, Math.max(0, (event.clientX - box.left) / box.width)), Math.min(1, Math.max(0, (event.clientY - box.top) / box.height))];
  };
  const constrain = (crop: [number, number, number, number]) => {
    if (s.cropAspect === "free") return crop;
    const [w, h] = s.cropAspect.split(":").map(Number) as [number, number];
    const aspect = w / h / ((s.detail?.width || 1) / (s.detail?.height || 1));
    const height = Math.min(1 - crop[1], crop[2] / aspect);
    return [crop[0], crop[1], height * aspect, height] as [number, number, number, number];
  };

  const down = (event: React.PointerEvent<HTMLCanvasElement>) => {
    if (!s.cropping && !s.brush) return;
    event.currentTarget.setPointerCapture(event.pointerId);
    if (s.cropping) start.current = at(event);
    else points.current = [at(event)];
  };
  const move = (event: React.PointerEvent<HTMLCanvasElement>) => {
    const el = canvas.current!;
    if (start.current) {
      const [x, y] = at(event);
      const origin = start.current;
      const crop = constrain([Math.min(origin[0], x), Math.min(origin[1], y), Math.abs(x - origin[0]), Math.abs(y - origin[1])]);
      s.set({ crop });
    }
    if (points.current) {
      const p = at(event);
      points.current.push(p);
      const context = el.getContext("2d")!;
      context.fillStyle = "rgba(255,64,64,0.5)";
      const radius = (s.brushSize / 100) * Math.max(el.width, el.height) / 2;
      context.beginPath();
      context.arc(p[0] * el.width, p[1] * el.height, radius, 0, Math.PI * 2);
      context.fill();
    }
  };
  const up = () => {
    start.current = null;
    const stroke = points.current;
    points.current = null;
    if (!stroke || !s.current || !s.brush) return;
    const op: Record<string, unknown> = {
      op: "paint",
      photo: s.current,
      variant: s.variant,
      samples: stroke.map(([x, y]) => ({ x, y })),
      size: s.brushSize / 100,
      erase: s.brushErase,
      brush: { hardness: 0.3 },
    };
    if (s.pendingBrush?.id === s.brush) op.create = s.pendingBrush;
    else op.adjustment = s.brush;
    void s.run(async () => {
      await usePhoto.getState().edit(op, "Stroke painted.");
      usePhoto.setState({ pendingBrush: null });
    });
  };

  if (s.view === "compare") {
    return (
      <div ref={host} className="flex min-h-0 flex-1 gap-2 p-2">
        {s.shots.map((entry, i) => (
          <figure key={i} className="flex min-w-0 flex-1 flex-col items-center gap-1">
            <img src={entry.shot.url} alt={entry.name} className="min-h-0 flex-1 object-contain" draggable={false} />
            <figcaption className="text-xs text-muted-foreground">{entry.name}</figcaption>
          </figure>
        ))}
      </div>
    );
  }
  return (
    <div ref={host} className="flex min-h-0 flex-1 items-center justify-center overflow-hidden p-2">
      {shot ? (
        <div className="relative" style={size ? { width: size.w, height: size.h } : undefined}>
          <img
            ref={image}
            src={shot.url}
            alt={`${s.current} ${s.variant}`}
            draggable={false}
            onLoad={fit}
            style={size ? { width: size.w, height: size.h } : undefined}
            className="block"
          />
          {size && (
            <canvas
              ref={canvas}
              width={size.w}
              height={size.h}
              onPointerDown={down}
              onPointerMove={move}
              onPointerUp={up}
              className={cn("absolute inset-0", (s.cropping || s.brush) && "cursor-crosshair")}
            />
          )}
        </div>
      ) : (
        <p className="text-sm text-muted-foreground">{s.status || "Select a photo."}</p>
      )}
    </div>
  );
}

export function PhotoStage() {
  const s = usePhoto();
  // Load once on entry; a changed filter reloads from the panel.
  useEffect(() => {
    void usePhoto.getState().run(() => usePhoto.getState().loadCatalog());
  }, []);
  // Re-develop when the view-affecting settings change.
  useEffect(() => {
    void usePhoto.getState().run(() => usePhoto.getState().renderLoupe());
  }, [s.variant, s.compareB, s.overlay, s.space, s.cropping, s.brush]);

  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      const target = event.target as HTMLElement;
      if (target.matches("input, textarea, select")) return;
      const st = usePhoto.getState();
      if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === "z" && !event.shiftKey) {
        event.preventDefault();
        void st.run(() => st.undo());
        return;
      }
      if (event.ctrlKey || event.metaKey || event.altKey) return;
      const ids = st.photos.map((p) => p.id);
      const at = st.current ? ids.indexOf(st.current) : -1;
      const rate = (fields: Record<string, unknown>) => {
        event.preventDefault();
        void st.run(() => st.edit({ op: "rate", selection: st.selected.join(","), ...fields }, `Updated ${st.selected.length} photo(s).`));
      };
      const key = event.key.toLowerCase();
      if (/^[0-5]$/.test(key)) rate({ rating: Number(key) });
      else if (/^[6-9]$/.test(key)) rate({ label: LABELS[Number(key) - 5] });
      else if (key === "p") rate({ pick: "pick" });
      else if (key === "x") rate({ pick: "reject" });
      else if (key === "u") rate({ pick: "none" });
      else if ((event.key === "ArrowRight" || event.key === "ArrowLeft") && ids.length) {
        event.preventDefault();
        st.select(ids[(at + (event.key === "ArrowRight" ? 1 : ids.length - 1)) % ids.length]!);
      } else if (key === "g") st.setView("grid");
      else if (key === "e" || event.key === "Enter") st.setView("loupe");
      else if (key === "c") st.setView("compare");
      else if (key === "j") st.set({ overlay: st.overlay === "clipping" ? "none" : "clipping" });
      else if (event.key === "Escape") st.set({ cropping: false, brush: null });
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);

  return (
    <main className="flex min-w-0 flex-1 flex-col bg-muted/30" aria-busy={s.busy}>
      <div className="flex h-9 shrink-0 items-center gap-1 border-b bg-card px-2 text-xs">
        {(["grid", "loupe", "compare"] as const).map((v) => (
          <button
            key={v}
            type="button"
            aria-pressed={s.view === v}
            onClick={() => s.setView(v)}
            className={cn("rounded px-2 py-1 capitalize hover:bg-accent", s.view === v && "bg-accent font-medium")}
          >
            {v}
          </button>
        ))}
        <form
          className="ml-3 flex items-center gap-1"
          onSubmit={(e) => {
            e.preventDefault();
            void s.run(() => s.loadCatalog());
          }}
        >
          <input
            aria-label="Search photos"
            placeholder="query, e.g. rating>=4"
            value={s.query}
            onChange={(e) => s.set({ query: e.target.value })}
            className="h-6 w-44 rounded border bg-card px-1.5"
          />
          <button className="rounded px-2 py-1 hover:bg-accent" type="submit">
            Search
          </button>
        </form>
        <span className="text-muted-foreground">{s.count}</span>
        <span className="ml-auto truncate text-muted-foreground">{s.status}</span>
      </div>
      {s.view === "grid" ? <Grid /> : <Loupe />}
    </main>
  );
}
