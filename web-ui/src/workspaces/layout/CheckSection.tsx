import { useState } from "react";
import { Download } from "lucide-react";
import { layoutOps, type LayoutPreflightReport } from "@/lib/api";
import { useStudio } from "@/store";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Check, NumberField, Row, Section } from "@/components/ui/kit";

/** Layout preflight (composer-measured) and tagged PDF / EPUB export. */
export function CheckSection() {
  const { doc, select, notify } = useStudio();
  const [bleed, setBleed] = useState(0);
  const [maxOverrides, setMaxOverrides] = useState(20);
  const [noAlt, setNoAlt] = useState(false);
  const [report, setReport] = useState<LayoutPreflightReport | null>(null);
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
  if (!doc) return null;

  return (
    <>
      <Section id="layout-preflight" title="Preflight" defaultOpen={false}>
        <Row label="Bleed (pt)">
          <NumberField ariaLabel="Bleed" min={0} max={1000} value={bleed} onCommit={setBleed} />
        </Row>
        <Row label="Overrides %">
          <NumberField ariaLabel="Maximum overrides percent" min={0} max={100} value={maxOverrides} onCommit={setMaxOverrides} />
        </Row>
        <Check checked={noAlt} onChange={setNoAlt}>
          Skip alt-text check
        </Check>
        <Button
          disabled={busy !== null}
          onClick={() =>
            void guard("Preflight", async () =>
              setReport(await layoutOps.preflight(doc, { bleed, max_overrides: maxOverrides, no_alt: noAlt })),
            )
          }
        >
          {busy === "Preflight" ? "Checking…" : "Run preflight"}
        </Button>
        {report && (
          <div className="space-y-1.5">
            <div className="flex items-center gap-2">
              <Badge variant={report.ok ? "success" : "destructive"}>{report.ok ? "Ready" : "Blocked"}</Badge>
              <span className="text-[11px] text-muted-foreground">
                {report.errors} error(s), {report.warnings} warning(s)
                {report.truncated ? `, ${report.issues_total} total` : ""}
              </span>
            </div>
            <ul className="max-h-64 space-y-1 overflow-auto">
              {report.issues.map((issue, index) => {
                const target = issue.frame ?? issue.node;
                return (
                  <li key={`${issue.code}-${target ?? index}-${index}`}>
                    <button
                      className="w-full rounded border bg-input p-1.5 text-left hover:border-primary disabled:cursor-default"
                      disabled={!target}
                      onClick={() => target && select(target)}
                    >
                      <span className="flex items-center gap-1.5">
                        <Badge variant={issue.severity === "error" ? "destructive" : "warning"}>{issue.severity}</Badge>
                        <code className="text-[10.5px]">{issue.code}</code>
                      </span>
                      <span className="mt-0.5 block text-[11.5px]">{issue.message}</span>
                      <span className="block text-[11px] text-muted-foreground">Fix: {issue.action}</span>
                    </button>
                  </li>
                );
              })}
            </ul>
          </div>
        )}
      </Section>
      <Section id="layout-publish" title="Publish" defaultOpen={false}>
        <p className="text-[11px] text-muted-foreground">Exports the open document, unsaved edits included.</p>
        <div className="flex flex-wrap gap-1.5">
          {(
            [
              ["Tagged PDF", "pdf", false],
              ["EPUB", "epub", false],
              ["EPUB fixed", "epub", true],
            ] as const
          ).map(([label, format, fixed]) => (
            <Button
              key={label}
              variant="outline"
              disabled={busy !== null}
              onClick={() => void guard(label, () => layoutOps.download(doc, format, fixed))}
            >
              <Download size={12} /> {label}
            </Button>
          ))}
        </div>
      </Section>
    </>
  );
}
