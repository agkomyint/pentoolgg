import { useCallback, useEffect, useRef, useState } from "react";
import { Button } from "@/components/ui/button";
import { Badge } from "@/components/ui/badge";
import { getJson, postJson } from "@/lib/api";
import type { AiStatus } from "@/lib/aiProvider";
import { findNode, pageOf, uid } from "@/lib/model";
import { useStudio } from "@/store";

type Run = { run: string; name: string; provider: string; model: string; candidates: number };
type Plan = { disclosure: { provider: string; model: string; calls: number; endpoint_host: string; prompt_sha256: string; sources: { node: string; sha256: string }[] } };
const field = "mt-1 min-h-9 w-full rounded border bg-background px-2 text-xs focus-visible:ring-2 focus-visible:ring-ring";
const errorMessage = (e: unknown) => e instanceof Error ? e.message : "The AI request failed. Retry or check the provider settings.";

export function AiGenerate({ status }: { status: Extract<AiStatus, { enabled: true }> }) {
  const studio = useStudio();
  const { doc, pageId, selectedId, layerId, shared, dirty, revision } = studio;
  const selected = findNode(pageOf(doc, pageId), selectedId)?.node;
  const imageSelected = selected?.kind === "image";
  const [provider, setProvider] = useState("");
  const [model, setModel] = useState("");
  const [prompt, setPrompt] = useState("");
  const [name, setName] = useState("generated-image");
  const [count, setCount] = useState(1);
  const [aspect, setAspect] = useState("1:1");
  const [size, setSize] = useState("1024x1024");
  const [mode, setMode] = useState("generate");
  const [runs, setRuns] = useState<Run[]>([]);
  const [plan, setPlan] = useState<{ body: Record<string, unknown>; report: Plan } | null>(null);
  const [busy, setBusy] = useState(false);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const job = useRef<string | null>(null);
  const cancelled = useRef(false);
  const configured = status.configured.find((p) => p.name === provider) ?? status.configured[0];
  const maxCalls = status.limits?.max_calls ?? 1;
  const blocked = !shared || dirty || !revision;
  const refresh = useCallback(async () => {
    try { setRuns((await getJson<{ runs: Run[] }>("/api/ai/runs")).runs); }
    catch (e) { setError(errorMessage(e)); }
    finally { setLoading(false); }
  }, []);
  useEffect(() => { void refresh(); }, [refresh]);
  useEffect(() => { setPlan(null); }, [provider, model, prompt, name, count, aspect, size, mode, selectedId, pageId]);
  useEffect(() => () => { if (job.current) void postJson("/api/ai/cancel", { job: job.current }).catch(() => {}); }, []);

  async function review() {
    setBusy(true); setError(null);
    try {
      const body = { name, provider: configured?.name, model: model || undefined, prompt,
        candidates: count, page: pageId,
        ...(mode === "generate" ? configured?.adapter === "gemini" ? { aspect } : { size }
          : { source: selectedId, kind: mode === "remove-background" ? mode : "edit" }),
      };
      const report = await postJson<Plan>("/api/ai/generate", { ...body, dry_run: true });
      setPlan({ body, report });
    } catch (e) { setError(errorMessage(e)); }
    finally { setBusy(false); }
  }
  async function generate() {
    if (!plan) return;
    setBusy(true); setError(null); cancelled.current = false;
    const id = uid("ai"); job.current = id;
    try {
      const result = await postJson<{ warnings?: string[] }>("/api/ai/generate", { ...plan.body, reviewed_disclosure: plan.report.disclosure, allow_model_call: true, job: id });
      if (!cancelled.current) { setPlan(null); await refresh(); studio.notify(["Candidates ready. Choose an image to accept.", ...(result.warnings ?? [])].join(" ")); }
    } catch (e) { if (!cancelled.current) setError(errorMessage(e)); }
    finally { if (job.current === id) { job.current = null; setBusy(false); } }
  }
  async function cancelGeneration() {
    try {
      const result = await postJson<{ cancelled: boolean }>("/api/ai/cancel", { job: job.current });
      cancelled.current = result.cancelled;
      // The provider call may take a while to unwind; do not leave the form disabled meanwhile.
      if (result.cancelled) { job.current = null; setBusy(false); }
      studio.notify(result.cancelled ? "Cancelled. A request already sent may still be billed." : "Cancellation was not acknowledged. Retry if the request is still running.");
    } catch (e) { setError(errorMessage(e)); }
  }
  async function accept(run: Run, candidate: number, replace: boolean) {
    setBusy(true); setError(null);
    try {
      studio.expectOwnChange();
      await postJson(`/api/ai/runs/${run.run}/accept`, {
        candidate, page: pageId, if_revision: revision,
        ...(replace ? { replace: selectedId } : { id: uid("generated-image"), layer: layerId }),
      });
      await studio.pull();
      if (doc) studio.rememberServerChange(doc);
      studio.notify(`Accepted ${run.provider}/${run.model}. Undo removes this change.`);
    } catch (e) { setError(errorMessage(e)); }
    finally { setBusy(false); }
  }
  async function keyout() {
    setBusy(true); setError(null);
    try {
      studio.expectOwnChange();
      await postJson("/api/ai/keyout", { source: selectedId, id: uid("cutout"), page: pageId, if_revision: revision });
      await studio.pull();
      if (doc) studio.rememberServerChange(doc);
      studio.notify("Removed magenta background locally. Undo removes this change.");
    } catch (e) { setError(errorMessage(e)); }
    finally { setBusy(false); }
  }
  async function discard(run: Run) {
    setBusy(true); setError(null);
    try { await postJson(`/api/ai/runs/${run.run}/discard`, {}); await refresh(); }
    catch (e) { setError(errorMessage(e)); }
    finally { setBusy(false); }
  }
  return <section aria-label="Generate images" className="space-y-3 border-y py-3 text-xs" aria-busy={busy}>
    <h4 className="font-semibold">Generate and edit images</h4>
    {blocked && <div className="space-y-2 text-muted-foreground">
      <p>{!shared ? "Open a document served by pentool serve before using AI." : dirty ? "You have unsaved edits. Save them to use AI." : "Waiting for the document revision. Refresh Studio if this persists."}</p>
      {shared && dirty && <Button variant="outline" onClick={() => void studio.save()}>Save now</Button>}
    </div>}
    {selected?.ai && typeof selected.ai === "object" ? <Badge variant="outline">AI: {String((selected.ai as Record<string, unknown>).provider)}/{String((selected.ai as Record<string, unknown>).model)}</Badge> : null}
    {imageSelected && <Button variant="outline" disabled={busy || blocked} onClick={() => void keyout()}>Remove magenta background (offline)</Button>}
    {(status.limits?.problems ?? []).map((problem) => <p key={problem.code} role="alert" className="text-destructive">{problem.message ?? problem.code} {problem.fix}</p>)}
    {!configured && <p className="text-muted-foreground">Connect a provider below to get started.</p>}
    <form onSubmit={(e) => { e.preventDefault(); void review(); }} className="space-y-2">
      <fieldset disabled={busy || blocked || !configured} className="space-y-2">
        <label className="block">Provider<select className={field} value={configured?.name ?? ""} onChange={(e) => { setProvider(e.target.value); setModel(""); }}>
          {status.configured.map((p) => <option key={p.name}>{p.name}</option>)}
        </select></label>
        <label className="block">Model<input className={field} value={model} onChange={(e) => setModel(e.target.value)} placeholder={configured?.default_model ?? "Provider default"} autoComplete="off" /></label>
        <label className="block">Action<select className={field} value={mode} onChange={(e) => setMode(e.target.value)}>
          <option value="generate">Generate an image</option>
          <option value="edit" disabled={!imageSelected || !configured?.capabilities.includes("edit")}>Create variations of selected image</option>
          <option value="remove-background" disabled={!imageSelected || !configured?.capabilities.includes("remove-background")}>Remove selected image background</option>
        </select></label>
        <label className="block">Run name<input className={field} value={name} onChange={(e) => setName(e.target.value)} required autoComplete="off" /></label>
        <label className="block">Prompt<textarea className={`${field} min-h-20 py-2`} value={prompt} onChange={(e) => setPrompt(e.target.value)} required={mode !== "remove-background"} placeholder="Describe the image you want" /></label>
        {mode === "generate" && (configured?.adapter === "gemini"
          ? <label className="block">Aspect ratio<select className={field} value={aspect} onChange={(e) => setAspect(e.target.value)}>{["1:1", "2:3", "3:2", "3:4", "4:3", "4:5", "5:4", "9:16", "16:9", "21:9"].map((a) => <option key={a}>{a}</option>)}</select></label>
          : <label className="block">Size<select className={field} value={size} onChange={(e) => setSize(e.target.value)}>{["1024x1024", "1024x1536", "1536x1024"].map((s) => <option key={s}>{s}</option>)}</select></label>)}
        <label className="block">Candidates<select className={field} value={count} onChange={(e) => setCount(Number(e.target.value))}>{Array.from({ length: Math.max(1, maxCalls) }, (_, i) => <option key={i} value={i + 1}>{i + 1}</option>)}</select></label>
        <Button type="submit" disabled={!!status.limits?.problems.length}>Review request</Button>
      </fieldset>
    </form>
    {plan && <div className="space-y-2 rounded border bg-muted/30 p-2" role="status">
      <p>{plan.report.disclosure.provider} / {plan.report.disclosure.model}</p>
      <p>{plan.report.disclosure.calls} paid model call(s) to {plan.report.disclosure.endpoint_host}</p>
      <p className="break-all">Prompt hash: {plan.report.disclosure.prompt_sha256}</p>
      {plan.report.disclosure.sources.map((s) => <p key={s.node} className="break-all">Source {s.node}: {s.sha256}</p>)}
      <p>The provider may charge for this request.</p>
      <Button disabled={busy || blocked} onClick={() => void generate()}>Confirm and generate</Button>
      <Button variant="ghost" disabled={busy} onClick={() => setPlan(null)}>Back</Button>
    </div>}
    {busy && <p role="status">{job.current ? "Waiting for the image provider…" : "Working…"}</p>}
    {busy && job.current && <Button variant="outline" onClick={() => void cancelGeneration()}>Cancel generation</Button>}
    {error && <div role="alert"><p className="text-destructive">{error}</p><Button variant="ghost" disabled={busy} onClick={() => void refresh()}>Refresh candidates</Button></div>}
    <h4 className="font-semibold">Candidates</h4>
    {loading ? <p role="status">Loading saved runs…</p> : runs.length === 0 && <p className="text-muted-foreground">No saved runs. Review a request to generate your first image.</p>}
    {runs.map((run) => <div key={run.run} className="space-y-2 rounded border p-2">
      <p>{run.name} <Badge variant="outline">{run.provider}/{run.model}</Badge></p>
      {Array.from({ length: run.candidates }, (_, index) => <div key={index} className="space-y-2">
        <img src={`/api/ai/runs/${run.run}/candidates/${index + 1}`} alt={`${run.name}, candidate ${index + 1}`} className="max-h-64 w-full rounded object-contain" loading="lazy" />
        <div className="flex flex-wrap gap-2">
          <Button disabled={busy || blocked} onClick={() => void accept(run, index + 1, false)}>Accept as new image</Button>
          <Button variant="outline" disabled={busy || blocked || !imageSelected} onClick={() => void accept(run, index + 1, true)}>Replace selected image</Button>
        </div>
      </div>)}
      <Button variant="ghost" disabled={busy} onClick={() => void discard(run)}>Discard run</Button>
    </div>)}
  </section>;
}
