import { useEffect, useMemo, useRef, useState } from "react";
import { Box, Loader2 } from "lucide-react";
import { api, type SceneNode } from "@/lib/api";
import { updateNode } from "@/lib/actions";
import { collectIds, findNode, pageOf, uid } from "@/lib/model";
import { useStudio } from "@/store";
import { Button } from "@/components/ui/button";
import { Check, Empty, NumberField, Row } from "@/components/ui/kit";

const heading = "text-[11px] font-semibold uppercase tracking-wide text-muted-foreground";

function embeddedData(file: File): Promise<string> {
  return new Promise((resolve, reject) => {
    const reader = new FileReader();
    reader.onerror = () => reject(reader.error ?? new Error("Could not read the model."));
    reader.onload = () => resolve(String(reader.result).split(",", 2)[1] ?? "");
    reader.readAsDataURL(file);
  });
}

function camera(node: SceneNode): Record<string, unknown> {
  return node.camera && typeof node.camera === "object" && !Array.isArray(node.camera) ? node.camera as Record<string, unknown> : {};
}

function vector(value: unknown): [number, number, number] {
  return Array.isArray(value) && value.length === 3 ? [Number(value[0]) || 0, Number(value[1]) || 0, Number(value[2]) || 0] : [0, 0, 0];
}

type Light = { kind: "directional" | "point" | "spot"; color: string; intensity: number; direction?: number[]; position?: number[]; inner_deg?: number; outer_deg?: number };

const newLight = (kind: Light["kind"]): Light => kind === "directional"
  ? { kind, color: "#ffffff", intensity: 2, direction: [-1, -2, -1] }
  : kind === "point"
    ? { kind, color: "#ffffff", intensity: 8, position: [2, 2, 3] }
    : { kind, color: "#ffffff", intensity: 8, position: [2, 2, 3], direction: [-1, -1, -1], inner_deg: 20, outer_deg: 30 };

export function Scene3dPanel() {
  const studio = useStudio();
  const input = useRef<HTMLInputElement>(null);
  const [importing, setImporting] = useState(false);
  const [material, setMaterial] = useState(0);
  const [lightIndex, setLightIndex] = useState(0);
  const [status, setStatus] = useState<{ adapter?: string; fallback_reason?: string } | null>(null);
  useEffect(() => {
    void api.scene3dStatus().then(setStatus).catch(() => setStatus({ fallback_reason: "Renderer status unavailable." }));
  }, []);
  const selected = useMemo(() => studio.doc ? findNode(pageOf(studio.doc, studio.pageId), studio.selectedId)?.node ?? null : null, [studio.doc, studio.pageId, studio.selectedId]);
  const node = selected?.kind === "scene3d" ? selected : null;
  const cam = node ? camera(node) : {};
  const target = vector(cam.target);
  const lights = Array.isArray(node?.lights) ? node.lights as Light[] : [];
  const light = lights[lightIndex];
  const change = (mutate: (draft: SceneNode) => void) => node && updateNode(node.id, mutate);
  const cameraValue = (key: string, value: unknown) => change((draft) => {
    const next = { ...camera(draft), [key]: value };
    for (const stale of ["eye", "up", "near", "far"]) delete next[stale];
    draft.camera = next;
  });
  const override = (key: string, value: unknown) => change((draft) => {
    const all = { ...((draft.material_overrides as Record<string, Record<string, unknown>> | undefined) ?? {}) };
    all[material] = { ...(all[material] ?? {}), [key]: value };
    draft.material_overrides = all;
  });
  const currentOverride = (key: "metallic" | "roughness", fallback: number) => Number((node?.material_overrides as Record<string, Record<string, unknown>> | undefined)?.[material]?.[key] ?? fallback);
  const overrideRecord = (node?.material_overrides as Record<string, Record<string, unknown>> | undefined)?.[material] ?? {};
  const editLight = (patch: Partial<Light>) => change((draft) => {
    const next = [...((draft.lights as Light[] | undefined) ?? [])];
    if (next[lightIndex]) next[lightIndex] = { ...next[lightIndex], ...patch };
    draft.lights = next;
  });

  const importFile = async (file: File) => {
    if (!studio.doc || !studio.layerId) return;
    const format = file.name.split(".").pop()?.toLowerCase();
    if (format !== "glb" && format !== "gltf" && format !== "obj") return studio.notify("Choose a .glb, .gltf, or .obj model.", true);
    setImporting(true);
    try {
      let id = uid("scene3d");
      const ids = collectIds(studio.doc);
      while (ids.has(id)) id = uid("scene3d");
      const result = await api.scene3dImport({ document: studio.doc, data: await embeddedData(file), format, id, layer: studio.layerId, page: studio.pageId, x: 80, y: 80, width: 480, height: 480 });
      studio.replaceDoc(result.document, "Import 3D model");
      studio.select(id);
      studio.notify(`Imported ${file.name}.`);
    } catch (error) {
      studio.notify(error instanceof Error ? error.message : "Model import failed.", true);
    } finally {
      setImporting(false);
      if (input.current) input.current.value = "";
    }
  };

  return <div className="space-y-4">
    <section className="space-y-2">
      <h3 className={heading}>Static 3D</h3>
      <Button className="min-h-10" variant="outline" disabled={importing || !studio.doc || studio.doc.version < 10} onClick={() => input.current?.click()}>
        {importing ? <Loader2 className="animate-spin" size={14} /> : <Box size={14} />}{importing ? "Importing…" : "Import model…"}
      </Button>
      <input ref={input} hidden type="file" accept=".glb,.gltf,.obj,model/gltf-binary,model/gltf+json" onChange={(event) => { const file = event.target.files?.[0]; if (file) void importFile(file); }} />
      {studio.doc && studio.doc.version < 10 && <p className="text-xs text-muted-foreground">3D needs format v10. Use the format upgrade in the top bar.</p>}
      <div role="status" className="rounded border bg-muted px-2 py-1.5 text-[11px]"><span className="font-medium">Renderer: reference CPU</span>{status?.adapter && <span className="block text-muted-foreground">Adapter: {status.adapter}</span>}{status?.fallback_reason && <span className="block text-muted-foreground">{status.fallback_reason}</span>}</div>
    </section>
    {!node ? <Empty>Import or select a 3D object to edit its view, lights, and materials.</Empty> : <>
      <section className="space-y-2">
        <h3 className={heading}>Camera · {node.id}</h3>
        <Row label="Orbit X"><NumberField value={Number(cam.azimuth ?? 30)} min={-360} max={360} onCommit={(v) => cameraValue("azimuth", v)} ariaLabel="Camera azimuth" /></Row>
        <Row label="Orbit Y"><NumberField value={Number(cam.elevation ?? 15)} min={-89} max={89} onCommit={(v) => cameraValue("elevation", v)} ariaLabel="Camera elevation" /></Row>
        <Row label="Zoom"><NumberField value={Number(cam.fov ?? 40)} min={5} max={120} onCommit={(v) => cameraValue("fov", v)} ariaLabel="Camera field of view" /></Row>
        <div className="grid grid-cols-3 gap-1" aria-label="Pan camera target">{["X", "Y", "Z"].map((axis, index) => <Button key={axis} variant="outline" className="min-h-10" onClick={() => cameraValue("target", target.map((v, i) => i === index ? v + 0.1 : v))}>Pan {axis}+</Button>)}</div>
        <Button variant="outline" className="min-h-10 w-full" onClick={() => change((draft) => { delete draft.camera; delete draft.physical_camera; })}>Frame model</Button>
      </section>
      <section className="space-y-2">
        <h3 className={heading}>Lighting</h3>
        <Row label="Exposure"><NumberField value={Number(node.exposure_ev ?? 0)} min={-10} max={10} step={0.25} onCommit={(v) => change((draft) => { draft.exposure_ev = v; })} ariaLabel="Exposure in EV" /></Row>
        {light && <>
          <Row label="Light"><select aria-label="Selected light" className="h-8 w-full rounded border bg-input px-2 text-xs" value={lightIndex} onChange={(event) => setLightIndex(Number(event.target.value))}>{lights.map((entry, index) => <option key={index} value={index}>{index + 1}. {entry.kind}</option>)}</select></Row>
          <Row label="Intensity"><NumberField value={Number(light.intensity)} min={0} max={100000} step={0.25} onCommit={(v) => editLight({ intensity: v })} ariaLabel="Light intensity" /></Row>
          <Row label="Color"><input aria-label="Light color" type="color" className="h-10 w-full rounded border bg-input p-1" value={light.color} onChange={(event) => editLight({ color: event.target.value })} /></Row>
          <Button className="min-h-10 w-full" variant="outline" onClick={() => change((draft) => { const next = [...((draft.lights as Light[] | undefined) ?? [])]; next.splice(lightIndex, 1); draft.lights = next; setLightIndex(Math.max(0, lightIndex - 1)); })}>Remove light</Button>
        </>}
        <div className="grid grid-cols-3 gap-1">{(["directional", "point", "spot"] as const).map((kind) => <Button key={kind} className="min-h-10 capitalize" variant="outline" onClick={() => change((draft) => { const next = [...((draft.lights as Light[] | undefined) ?? []), newLight(kind)]; draft.lights = next; setLightIndex(next.length - 1); })}>{kind}</Button>)}</div>
        <div className="flex gap-1"><Button className="min-h-10 flex-1" variant="outline" onClick={() => change((draft) => { delete draft.lights; })}>Default rig</Button><Button className="min-h-10 flex-1" variant="outline" onClick={() => change((draft) => { draft.lights = []; })}>No lights</Button></div>
      </section>
      <section className="space-y-2">
        <h3 className={heading}>Material override</h3>
        <Row label="Material"><NumberField value={material} min={0} step={1} onCommit={(v) => setMaterial(Math.floor(v))} ariaLabel="Material index" /></Row>
        <Row label="Metallic"><NumberField value={currentOverride("metallic", 0)} min={0} max={1} step={0.05} onCommit={(v) => override("metallic", v)} ariaLabel="Material metallic value" /></Row>
        <Row label="Roughness"><NumberField value={currentOverride("roughness", 1)} min={0} max={1} step={0.05} onCommit={(v) => override("roughness", v)} ariaLabel="Material roughness value" /></Row>
        <Row label="Base"><input aria-label="Material base color" type="color" className="h-10 w-full rounded border bg-input p-1" value={String(overrideRecord.base_color ?? "#ffffff").slice(0, 7)} onChange={(event) => override("base_color", event.target.value)} /></Row>
        <Row label="Emissive"><input aria-label="Material emissive color" type="color" className="h-10 w-full rounded border bg-input p-1" value={String(overrideRecord.emissive ?? "#000000").slice(0, 7)} onChange={(event) => override("emissive", event.target.value)} /></Row>
        <Check checked={Boolean(overrideRecord.double_sided)} onChange={(value) => override("double_sided", value)}>Double-sided</Check>
        <Button variant="outline" className="min-h-10 w-full" onClick={() => change((draft) => { delete draft.material_overrides; })}>Reset overrides</Button>
      </section>
    </>}
  </div>;
}
