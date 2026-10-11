import { useEffect, useRef, useState } from "react";
import { GROUPS, SLIDERS, getPath, type Develop, type PreviewReport } from "@/lib/photo";
import { Button } from "@/components/ui/button";
import { cn } from "@/lib/utils";
import { usePhoto } from "./photoStore";

const field = "h-7 w-full rounded border bg-card px-1.5 text-xs";

function Section({ title, open, children }: { title: string; open?: boolean; children: React.ReactNode }) {
  return (
    <details open={open} className="border-b py-2">
      <summary className="cursor-pointer text-xs font-semibold uppercase tracking-wide text-muted-foreground">{title}</summary>
      <div className="mt-2 space-y-2">{children}</div>
    </details>
  );
}

function Histogram({ report }: { report: PreviewReport | null }) {
  const ref = useRef<HTMLCanvasElement>(null);
  const setOverlay = usePhoto((s) => s.set);
  useEffect(() => {
    const canvas = ref.current;
    const context = canvas?.getContext("2d");
    if (!canvas || !context) return;
    context.clearRect(0, 0, canvas.width, canvas.height);
    if (!report) return;
    const { r, g, b, bins } = report.delivery.histogram;
    const peak = Math.max(1, ...r.slice(1, -1), ...g.slice(1, -1), ...b.slice(1, -1));
    context.globalCompositeOperation = "lighter";
    for (const [values, color] of [[r, "#ff4040"], [g, "#40ff40"], [b, "#4080ff"]] as const) {
      context.fillStyle = color;
      values.forEach((count, i) => {
        const height = Math.min(1, count / peak) * canvas.height;
        context.fillRect((i * canvas.width) / bins, canvas.height - height, canvas.width / bins, height);
      });
    }
    context.globalCompositeOperation = "source-over";
  }, [report]);
  return (
    <div className="space-y-1">
      <canvas ref={ref} width={256} height={80} className="w-full rounded border bg-black" aria-label="Histogram" />
      {report && (
        <>
          <div className="flex gap-1">
            <Button
              size="sm"
              variant="outline"
              className={cn(report.shadow_clipped_pixels > 0 && "border-blue-500")}
              title={`${report.shadow_clipped_pixels} pixels with a channel at zero`}
              onClick={() => setOverlay({ overlay: "clipping" })}
            >
              Shadows clipped
            </Button>
            <Button
              size="sm"
              variant="outline"
              className={cn(report.highlight_clipped_pixels > 0 && "border-red-500")}
              title={`${report.highlight_clipped_pixels} pixels with a channel at maximum`}
              onClick={() => setOverlay({ overlay: "clipping" })}
            >
              Highlights clipped
            </Button>
          </div>
          <p className="text-[11px] text-muted-foreground">
            {report.delivery.out_of_gamut_pixels} of {report.delivery.pixels} pixels outside {report.delivery.space} (mapped perceptually) · {report.width}×
            {report.height} of {report.developed_width}×{report.developed_height}
          </p>
        </>
      )}
    </div>
  );
}

function Slider({ label, min, max, step, value, onCommit, onReset }: { label: string; min: number; max: number; step: number; value: number; onCommit: (v: number) => void; onReset: () => void }) {
  const [local, setLocal] = useState(value);
  useEffect(() => setLocal(value), [value]);
  return (
    <label className="grid grid-cols-[84px_1fr_44px] items-center gap-2 text-xs">
      <span>{label}</span>
      <input
        type="range"
        min={min}
        max={max}
        step={step}
        value={local}
        title="Double-click to reset"
        onChange={(e) => setLocal(Number(e.target.value))}
        onPointerUp={(e) => onCommit(Number(e.currentTarget.value))}
        onKeyUp={(e) => onCommit(Number(e.currentTarget.value))}
        onDoubleClick={onReset}
      />
      <output className="text-right tabular-nums">{local}</output>
    </label>
  );
}

export function PhotoPanel() {
  const s = usePhoto();
  const develop: Develop = s.developOf();
  const [temperature, setTemperature] = useState(5500);
  const [tint, setTint] = useState(0);
  const [newId, setNewId] = useState("");
  const [keywords, setKeywords] = useState("");
  const [syncTo, setSyncTo] = useState("");
  const [autoPer, setAutoPer] = useState(false);
  const [groups, setGroups] = useState<string[]>(GROUPS.filter((g) => !["crop", "local"].includes(g)));
  const [snapshot, setSnapshot] = useState("");

  useEffect(() => {
    setTemperature(develop.white_balance?.temperature ?? 5500);
    setTint(develop.white_balance?.tint ?? 0);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [s.detail, s.variant]);

  const photo = s.current;
  const variant = s.variant;
  const dev = (extra: Record<string, unknown>, message: string) => {
    if (!photo) return;
    void s.run(() => s.edit({ op: "develop", photo, variant, ...extra }, message));
  };
  const say = (status: string) => s.set({ status });
  const variants = s.detail?.photo.variants ?? [];
  const snapshots = s.detail?.photo.snapshots ?? [];
  const local: { id: string; name?: string; enabled?: boolean; params?: { exposure?: number }; mask: { components: { kind: string }[] } }[] = develop.local ?? [];
  const crop: number[] | undefined = develop.crop?.rect;

  if (!photo) return <p className="p-3 text-sm text-muted-foreground">Select a photo in the grid. Add DNGs with <code>pentool raw add</code>.</p>;

  const replaceLocal = (id: string, change: (a: (typeof local)[number]) => Partial<(typeof local)[number]>) =>
    dev({ set: { local: local.map((a) => (a.id === id ? { ...a, ...change(a) } : a)) } }, "Mask updated.");

  const newMask = (kind: "radial" | "linear" | "brush") => {
    let n = local.length + 1;
    while (local.some((a) => a.id === `${kind}-${n}`)) n++;
    const id = `${kind}-${n}`;
    const exposure = s.maskExposure;
    if (kind === "brush") {
      s.set({ brush: id, pendingBrush: { id, params: { exposure } }, cropping: false, status: `Paint on the photo to create ${id}.` });
      return;
    }
    const component =
      kind === "radial"
        ? { kind, mode: "add", center: [0.5, 0.5], radius: [0.25, 0.25], feather: 50 }
        : { kind, mode: "add", start: [0.5, 0], end: [0.5, 0.5] };
    dev({ set: { local: [...local, { id, mask: { components: [component] }, params: { exposure } }] } }, `Added ${id}.`);
  };

  const startCrop = () => {
    if (s.cropping) return s.set({ cropping: false });
    s.set({ cropping: true, brush: null, crop: develop.crop?.rect ? ([...develop.crop.rect] as [number, number, number, number]) : [0, 0, 1, 1] });
  };
  const applyCrop = () => {
    const c = s.crop;
    if (!s.cropping || !c || c[2] <= 0 || c[3] <= 0) return say("Drag a crop rectangle on the photo first.");
    const round = (v: number) => Math.round(v * 10000) / 10000;
    const x = round(c[0]);
    const y = round(c[1]);
    const rect = [x, y, Math.floor(Math.min(c[2], 1 - x) * 10000) / 10000, Math.floor(Math.min(c[3], 1 - y) * 10000) / 10000];
    s.set({ cropping: false });
    dev({ set: { "crop.rect": rect, "crop.aspect": s.cropAspect } }, "Crop applied.");
  };

  const sync = () => {
    if (!s.copied) return say("Copy settings from a photo first.");
    const to = syncTo.trim() || s.selected.filter((id) => `${id}/master` !== s.copied).join(",");
    if (!to) return say("Select target photos (ctrl/shift-click) or type IDs or a query.");
    void s.run(async () => {
      await s.edit({ op: "sync", source: s.copied, to, groups, auto_per_photo: autoPer }, `Synced ${groups.length} group(s) from ${s.copied}.`);
      s.clearThumbs();
    });
  };

  return (
    <div className="space-y-0 text-xs">
      <div className="grid grid-cols-2 gap-2 pb-2">
        <label className="space-y-1">
          <span className="text-muted-foreground">Variant</span>
          <select className={field} value={variant} onChange={(e) => s.set({ variant: e.target.value, brush: null })}>
            {variants.map((v) => (
              <option key={v.id} value={v.id}>
                {v.name ? `${v.name} (${v.id})` : v.id}
              </option>
            ))}
          </select>
        </label>
        <label className="space-y-1">
          <span className="text-muted-foreground">Compare with</span>
          <select className={field} value={s.compareB} onChange={(e) => s.set({ compareB: e.target.value })}>
            {variants.map((v) => (
              <option key={v.id} value={v.id}>
                {v.name ? `${v.name} (${v.id})` : v.id}
              </option>
            ))}
          </select>
        </label>
        <label className="space-y-1">
          <span className="text-muted-foreground">Preview space</span>
          <select
            className={field}
            value={s.space}
            onChange={(e) => {
              s.clearThumbs();
              s.set({ space: e.target.value as "srgb" | "display-p3" });
              void s.run(() => s.loadCatalog());
            }}
          >
            <option value="srgb">sRGB</option>
            <option value="display-p3">Display P3</option>
          </select>
        </label>
        <label className="space-y-1">
          <span className="text-muted-foreground">Overlay</span>
          <select className={field} value={s.overlay} onChange={(e) => s.set({ overlay: e.target.value as "none" | "clipping" | "gamut" })}>
            <option value="none">None</option>
            <option value="clipping">Clipping</option>
            <option value="gamut">Out of gamut</option>
          </select>
        </label>
      </div>
      <div className="flex gap-1 pb-2">
        <Button size="sm" variant="outline" onClick={() => void s.run(() => s.undo())}>
          Undo
        </Button>
        <Button size="sm" variant="outline" onClick={() => dev({ auto_tone: true }, "Auto tone applied.")}>
          Auto tone
        </Button>
      </div>

      <Histogram report={s.report} />

      <Section title="White balance" open>
        <div className="flex flex-wrap gap-1">
          <Button size="sm" variant="outline" onClick={() => dev({ white_balance: { mode: "as-shot" } }, "White balance as shot.")}>
            As shot
          </Button>
          <Button size="sm" variant="outline" onClick={() => dev({ white_balance: { mode: "suggest" } }, "Suggested white balance applied.")}>
            Auto
          </Button>
        </div>
        <div className="grid grid-cols-2 gap-2">
          <label className="space-y-1">
            <span>Temperature (K)</span>
            <input className={field} type="number" min={2000} max={50000} value={temperature} onChange={(e) => setTemperature(Number(e.target.value))} />
          </label>
          <label className="space-y-1">
            <span>Tint</span>
            <input className={field} type="number" min={-150} max={150} value={tint} onChange={(e) => setTint(Number(e.target.value))} />
          </label>
        </div>
        <Button size="sm" variant="outline" onClick={() => dev({ white_balance: { mode: "temperature", temperature, tint } }, "White balance set.")}>
          Apply
        </Button>
      </Section>

      {SLIDERS.map(([title, sliders]) => (
        <Section key={title} title={title} open={title === "Tone"}>
          {sliders.map(([path, label, min, max, step]) => (
            <Slider
              key={`${photo}/${variant}/${path}`}
              label={label}
              min={min}
              max={max}
              step={step}
              value={getPath(develop, path) ?? 0}
              onCommit={(value) => {
                if (value === (getPath(develop, path) ?? 0)) return;
                const set: Record<string, unknown> = { [path]: value };
                // Grain needs a seed so it is reproducible; keep any existing one.
                if (path === "effects.grain.amount" && getPath(develop, "effects.grain.seed") === undefined) set["effects.grain.seed"] = 1;
                dev({ set }, `${label} set to ${value}.`);
              }}
              onReset={() => dev({ unset: [path] }, `${label} reset.`)}
            />
          ))}
        </Section>
      ))}

      <Section title="Crop">
        <p className="text-muted-foreground">{crop ? `Crop ${crop.map((v) => Number(v).toFixed(3)).join(", ")}` : "No crop"}</p>
        <select
          aria-label="Crop aspect"
          className={field}
          value={s.cropAspect}
          onChange={(e) => s.set({ cropAspect: e.target.value })}
        >
          {["free", "1:1", "4:3", "3:2", "16:9", "5:4", "4:5"].map((a) => (
            <option key={a}>{a}</option>
          ))}
        </select>
        <div className="flex gap-1">
          <Button size="sm" variant="outline" onClick={startCrop}>
            {s.cropping ? "Cancel crop" : "Crop…"}
          </Button>
          <Button size="sm" variant="outline" onClick={applyCrop}>
            Apply
          </Button>
          <Button size="sm" variant="outline" onClick={() => dev({ unset: ["crop"] }, "Crop removed.")}>
            Clear
          </Button>
        </div>
      </Section>

      <Section title="Local adjustments">
        <label className="grid grid-cols-[84px_1fr_44px] items-center gap-2">
          <span>New exposure</span>
          <input type="range" min={-4} max={4} step={0.05} value={s.maskExposure} onChange={(e) => s.set({ maskExposure: Number(e.target.value) })} />
          <output className="text-right tabular-nums">{s.maskExposure}</output>
        </label>
        <div className="flex gap-1">
          {(["radial", "linear", "brush"] as const).map((kind) => (
            <Button key={kind} size="sm" variant="outline" className="capitalize" onClick={() => newMask(kind)}>
              {kind}
            </Button>
          ))}
        </div>
        {s.brush && (
          <div className="space-y-1 rounded border p-2">
            <p>Painting {s.brush}. Drag on the photo.</p>
            <label className="grid grid-cols-[84px_1fr_44px] items-center gap-2">
              <span>Size</span>
              <input type="range" min={1} max={40} value={s.brushSize} onChange={(e) => s.set({ brushSize: Number(e.target.value) })} />
              <output className="text-right tabular-nums">{s.brushSize}</output>
            </label>
            <label className="flex items-center gap-1">
              <input type="checkbox" checked={s.brushErase} onChange={(e) => s.set({ brushErase: e.target.checked })} /> Erase
            </label>
          </div>
        )}
        {local.map((a) => (
          <div key={a.id} className="space-y-1 rounded border p-2">
            <strong>
              {a.name || a.id} · {a.mask.components.map((c) => c.kind).join(" + ")}
            </strong>
            <label className="flex items-center gap-1">
              <input type="checkbox" checked={a.enabled !== false} onChange={(e) => replaceLocal(a.id, () => ({ enabled: e.target.checked }))} /> Enabled
            </label>
            <Slider
              key={`${a.id}/${a.params?.exposure ?? 0}`}
              label="Exposure"
              min={-4}
              max={4}
              step={0.05}
              value={a.params?.exposure ?? 0}
              onCommit={(value) => replaceLocal(a.id, (x) => ({ params: { ...x.params, exposure: value } }))}
              onReset={() => replaceLocal(a.id, (x) => ({ params: { ...x.params, exposure: 0 } }))}
            />
            <div className="flex gap-1">
              <Button size="sm" variant="outline" onClick={() => s.set({ brush: s.brush === a.id ? null : a.id, cropping: false })}>
                {s.brush === a.id ? "Stop editing" : "Edit / paint"}
              </Button>
              <Button
                size="sm"
                variant="destructive"
                onClick={() => {
                  if (s.brush === a.id) s.set({ brush: null });
                  dev({ set: { local: local.filter((x) => x.id !== a.id) } }, `Removed ${a.id}.`);
                }}
              >
                Delete
              </Button>
            </div>
          </div>
        ))}
      </Section>

      <Section title="Copy / sync">
        <div className="flex items-center gap-2">
          <Button size="sm" variant="outline" onClick={() => s.set({ copied: `${photo}/${variant}` })}>
            Copy settings
          </Button>
          <span className="truncate text-muted-foreground">{s.copied ? `Copied ${s.copied}` : ""}</span>
        </div>
        <div className="grid grid-cols-2 gap-x-2 gap-y-1">
          {GROUPS.map((g) => (
            <label key={g} className="flex items-center gap-1">
              <input
                type="checkbox"
                checked={groups.includes(g)}
                onChange={(e) => setGroups(e.target.checked ? [...groups, g] : groups.filter((x) => x !== g))}
              />
              {g}
            </label>
          ))}
        </div>
        <input className={field} placeholder="Sync to IDs or query (default: selection)" value={syncTo} onChange={(e) => setSyncTo(e.target.value)} />
        <label className="flex items-center gap-1">
          <input type="checkbox" checked={autoPer} onChange={(e) => setAutoPer(e.target.checked)} /> Auto tone per photo
        </label>
        <Button size="sm" variant="outline" onClick={sync}>
          Sync
        </Button>
      </Section>

      <Section title="Variants & snapshots">
        <input className={field} placeholder="New ID" value={newId} onChange={(e) => setNewId(e.target.value)} />
        <div className="flex gap-1">
          <Button
            size="sm"
            variant="outline"
            onClick={() => {
              const id = newId.trim();
              if (!id) return say("Type an ID for the new variant.");
              void s.run(async () => {
                await s.edit({ op: "variant", photo, id, from: variant }, `Variant ${id} created from ${variant}.`);
                s.set({ variant: id });
              });
            }}
          >
            Add variant
          </Button>
          <Button
            size="sm"
            variant="outline"
            onClick={() => {
              const id = newId.trim();
              if (!id) return say("Type an ID for the snapshot.");
              dev({ op: "snapshot", id }, `Snapshot ${id} saved.`);
            }}
          >
            Snapshot
          </Button>
        </div>
        <div className="flex gap-1">
          <select className={field} value={snapshot} onChange={(e) => setSnapshot(e.target.value)} aria-label="Snapshots">
            <option value="">Snapshots…</option>
            {snapshots.map((x) => (
              <option key={x.id} value={x.id}>
                {x.name ? `${x.name} (${x.id})` : x.id}
              </option>
            ))}
          </select>
          <Button
            size="sm"
            variant="outline"
            onClick={() => {
              if (!snapshot) return say("Choose a snapshot to restore.");
              void s.run(() => s.edit({ op: "restore", photo, snapshot }, `Restored ${snapshot}.`));
            }}
          >
            Restore
          </Button>
        </div>
      </Section>

      <Section title="Keywords">
        <p className="text-muted-foreground">{(s.detail?.photo.keywords ?? []).join(", ") || "No keywords"}</p>
        <input className={field} placeholder="comma, separated" value={keywords} onChange={(e) => setKeywords(e.target.value)} />
        <div className="flex gap-1">
          {(["add", "remove"] as const).map((mode) => (
            <Button
              key={mode}
              size="sm"
              variant="outline"
              className="capitalize"
              onClick={() => {
                const words = keywords.split(",").map((w) => w.trim()).filter(Boolean);
                if (!words.length) return;
                void s.run(() => s.edit({ op: "keyword", selection: s.selected.join(","), [mode]: words }, `${mode === "add" ? "Added" : "Removed"} ${words.join(", ")}.`));
              }}
            >
              {mode}
            </Button>
          ))}
        </div>
      </Section>
    </div>
  );
}
