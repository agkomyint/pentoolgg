import { useState } from "react";
import { layoutApi, type LayoutOverview, type LayoutPreflightReport } from "@/lib/api";
import { useStudio } from "@/store";
import { Button } from "@/components/ui/button";
import { Badge } from "@/components/ui/badge";

const link = "inline-flex h-8 items-center rounded border bg-card px-3 text-sm hover:bg-accent";

/** Layout workspace: composition and every check run in Rust; this displays results. */
export function LayoutPanel() {
  const { select, selectedId, notify } = useStudio();
  const [bleed, setBleed] = useState(0);
  const [maxOverrides, setMaxOverrides] = useState(20);
  const [noAlt, setNoAlt] = useState(false);
  const [report, setReport] = useState<LayoutPreflightReport | null>(null);
  const [overview, setOverview] = useState<LayoutOverview | null>(null);
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

  return (
    <div className="space-y-5">
      <section className="space-y-2">
        <h3 className="text-xs font-semibold uppercase tracking-wide text-muted-foreground">Stories</h3>
        <Button
          variant="outline"
          disabled={busy !== null}
          onClick={() => guard("Stories", async () => setOverview(await layoutApi.overview()))}
        >
          {busy === "Stories" ? "Composing..." : "Compose stories"}
        </Button>
        {overview && (
          <ul className="space-y-1">
            {overview.stories.length === 0 && (
              <li className="text-xs text-muted-foreground">No stories in this document.</li>
            )}
            {overview.stories.map((s) => (
              <li key={s.id}>
                <button
                  className="w-full rounded border bg-card p-2 text-left hover:bg-accent disabled:cursor-default"
                  disabled={s.frames.length === 0}
                  onClick={() => s.frames[0] && select(s.frames[0])}
                  aria-pressed={s.frames.includes(selectedId ?? "")}
                >
                  <span className="flex items-center gap-1.5">
                    <code className="text-[11px]">{s.id}</code>
                    {s.overset && <Badge variant="destructive">overset</Badge>}
                    {!s.placed && <Badge variant="warning">unplaced</Badge>}
                    <span className="ml-auto text-[11px] text-muted-foreground">
                      {s.paragraphs} para · {s.characters} chars · {s.frames.length} frame(s)
                    </span>
                  </span>
                  {s.overset && (
                    <span className="mt-1 block text-xs">{s.overset_characters} character(s) do not fit.</span>
                  )}
                </button>
              </li>
            ))}
          </ul>
        )}
      </section>

      <section className="space-y-2">
        <h3 className="text-xs font-semibold uppercase tracking-wide text-muted-foreground">Layout preflight</h3>
        <div className="grid grid-cols-3 gap-2">
          <label className="text-xs">
            Bleed pt
            <input
              className="mt-0.5 h-7 w-full rounded border bg-card px-1.5"
              type="number"
              min={0}
              max={1000}
              value={bleed}
              onChange={(e) => setBleed(Number(e.target.value))}
            />
          </label>
          <label className="text-xs">
            Overrides %
            <input
              className="mt-0.5 h-7 w-full rounded border bg-card px-1.5"
              type="number"
              min={0}
              max={100}
              value={maxOverrides}
              onChange={(e) => setMaxOverrides(Number(e.target.value))}
            />
          </label>
          <label className="flex items-end gap-1 pb-1 text-xs">
            <input type="checkbox" checked={noAlt} onChange={(e) => setNoAlt(e.target.checked)} />
            Skip alt
          </label>
        </div>
        <Button
          disabled={busy !== null}
          onClick={() =>
            guard("Preflight", async () =>
              setReport(await layoutApi.preflight({ bleed, max_overrides: maxOverrides, no_alt: noAlt })),
            )
          }
        >
          {busy === "Preflight" ? "Running..." : "Run layout preflight"}
        </Button>
        {report && (
          <div className="space-y-1.5">
            <div className="flex items-center gap-2">
              <Badge variant={report.ok ? "success" : "destructive"}>{report.ok ? "Ready" : "Blocked"}</Badge>
              <span className="text-xs text-muted-foreground">
                {report.errors} error(s), {report.warnings} warning(s)
                {report.truncated ? `, ${report.issues_total} total` : ""}
              </span>
            </div>
            <ul className="space-y-1">
              {report.issues.map((issue, index) => {
                const target = issue.frame ?? issue.node;
                return (
                  <li key={`${issue.code}-${target ?? index}-${index}`}>
                    <button
                      className="w-full rounded border bg-card p-2 text-left hover:bg-accent disabled:cursor-default"
                      disabled={!target}
                      onClick={() => target && select(target)}
                    >
                      <span className="flex items-center gap-1.5">
                        <Badge variant={issue.severity === "error" ? "destructive" : "warning"}>
                          {issue.severity}
                        </Badge>
                        <code className="text-[11px]">{issue.code}</code>
                        {issue.page && (
                          <span className="ml-auto text-[11px] text-muted-foreground">{issue.page}</span>
                        )}
                      </span>
                      <span className="mt-1 block text-xs">{issue.message}</span>
                      <span className="block text-xs text-muted-foreground">Fix: {issue.action}</span>
                    </button>
                  </li>
                );
              })}
            </ul>
          </div>
        )}
      </section>

      <section className="space-y-2">
        <h3 className="text-xs font-semibold uppercase tracking-wide text-muted-foreground">Publish</h3>
        <div className="flex flex-wrap gap-2">
          <a className={link} href={layoutApi.exportUrl("pdf")} download>
            Tagged PDF
          </a>
          <a className={link} href={layoutApi.exportUrl("epub")} download>
            EPUB
          </a>
          <a className={link} href={layoutApi.exportUrl("epub", true)} download>
            EPUB fixed
          </a>
        </div>
      </section>
    </div>
  );
}
