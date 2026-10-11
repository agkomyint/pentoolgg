import { useState } from "react";
import { api, type PlateListing, type PreflightReport } from "@/lib/api";
import { useStudio } from "@/store";
import { Button } from "@/components/ui/button";
import { Badge } from "@/components/ui/badge";

/** Print workspace: every check runs in Rust, this only displays the result. */
export function PrintPanel() {
  const { pageId, selectedId, select, setView, notify } = useStudio();
  const [minPpi, setMinPpi] = useState(150);
  const [minBleed, setMinBleed] = useState(0.125);
  const [maxInk, setMaxInk] = useState<string>("");
  const [report, setReport] = useState<PreflightReport | null>(null);
  const [plates, setPlates] = useState<PlateListing | null>(null);
  const [busy, setBusy] = useState<string | null>(null);

  const guard = async (name: string, work: () => Promise<void>) => {
    setBusy(name);
    try {
      await work();
    } catch (error) {
      notify(error instanceof Error ? error.message : `${name} failed.`, true);
    } finally {
      setBusy(null);
    }
  };

  const runPreflight = () =>
    guard("Preflight", async () => {
      setReport(
        await api.preflight({
          page: pageId,
          min_ppi: minPpi,
          min_bleed_inches: minBleed,
          max_total_ink: maxInk === "" ? undefined : Number(maxInk),
        }),
      );
    });
  const loadPlates = () =>
    guard("Plates", async () => {
      const listing = await api.plates(pageId);
      setPlates(listing);
      const first = listing.plates[0];
      if (first) setView({ kind: "image", url: api.plateUrl(first.plate, pageId), label: `Plate ${first.plate}` });
    });

  return (
    <div className="space-y-5">
      <section className="space-y-2">
        <h3 className="text-xs font-semibold uppercase tracking-wide text-muted-foreground">Preflight</h3>
        <div className="grid grid-cols-3 gap-2">
          <label className="text-xs">
            Min ppi
            <input className="mt-0.5 h-7 w-full rounded border bg-card px-1.5" type="number" min={1} max={2400} value={minPpi} onChange={(e) => setMinPpi(Number(e.target.value))} />
          </label>
          <label className="text-xs">
            Min bleed in
            <input className="mt-0.5 h-7 w-full rounded border bg-card px-1.5" type="number" min={0} max={4} step={0.0625} value={minBleed} onChange={(e) => setMinBleed(Number(e.target.value))} />
          </label>
          <label className="text-xs">
            Max ink %
            <input className="mt-0.5 h-7 w-full rounded border bg-card px-1.5" type="number" min={100} max={400} placeholder="off" value={maxInk} onChange={(e) => setMaxInk(e.target.value)} />
          </label>
        </div>
        <Button onClick={runPreflight} disabled={busy !== null}>
          {busy === "Preflight" ? "Running..." : "Run preflight"}
        </Button>
        {report && (
          <div className="space-y-1.5">
            <div className="flex items-center gap-2">
              <Badge variant={report.ok ? "success" : "destructive"}>{report.ok ? "Ready" : "Blocked"}</Badge>
              <span className="text-xs text-muted-foreground">
                {report.errors} error(s), {report.warnings} warning(s)
              </span>
            </div>
            <ul className="space-y-1">
              {report.issues.map((issue, index) => (
                <li key={`${issue.code}-${issue.node ?? index}-${index}`}>
                  <button
                    className="w-full rounded border bg-card p-2 text-left hover:bg-accent disabled:cursor-default"
                    disabled={!issue.node}
                    onClick={() => issue.node && select(issue.node)}
                    aria-pressed={selectedId === issue.node}
                  >
                    <span className="flex items-center gap-1.5">
                      <Badge variant={issue.severity === "error" ? "destructive" : "warning"}>{issue.severity}</Badge>
                      <code className="text-[11px]">{issue.code}</code>
                      {issue.node && <span className="ml-auto text-[11px] text-muted-foreground">{issue.node}</span>}
                    </span>
                    <span className="mt-1 block text-xs">{issue.message}</span>
                    <span className="block text-xs text-muted-foreground">Fix: {issue.fix}</span>
                  </button>
                </li>
              ))}
            </ul>
          </div>
        )}
      </section>

      <section className="space-y-2">
        <h3 className="text-xs font-semibold uppercase tracking-wide text-muted-foreground">Plates and proof</h3>
        <div className="flex gap-2">
          <Button variant="outline" onClick={loadPlates} disabled={busy !== null}>
            {busy === "Plates" ? "Separating..." : "Load plates"}
          </Button>
          <Button
            variant="outline"
            onClick={() => setView({ kind: "image", url: api.proofUrl(pageId), label: "Soft proof (numeric, not a contract proof)" })}
          >
            Soft proof
          </Button>
          <Button variant="ghost" onClick={() => setView({ kind: "document" })}>
            Back to page
          </Button>
        </div>
        {plates && (
          <ul className="space-y-1">
            {plates.plates.map((p) => (
              <li key={p.plate}>
                <button
                  className="flex w-full items-center gap-2 rounded border bg-card px-2 py-1 text-left hover:bg-accent"
                  onClick={() => setView({ kind: "image", url: api.plateUrl(p.plate, pageId), label: `Plate ${p.plate}` })}
                >
                  <span className="font-medium">{p.plate}</span>
                  {p.spot && <Badge>spot</Badge>}
                  <span className="ml-auto text-xs tabular-nums text-muted-foreground">
                    {p.average_coverage_percent.toFixed(1)}% avg / {p.max_percent.toFixed(0)}% max
                  </span>
                </button>
              </li>
            ))}
          </ul>
        )}
      </section>
    </div>
  );
}
