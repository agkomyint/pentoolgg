import { useCallback, useEffect, useState } from "react";
import { Button } from "@/components/ui/button";
import { Badge } from "@/components/ui/badge";
import { getJson } from "@/lib/api";

// Contribution stats (v0.20). Read only: the figures are derived by replaying the verified
// operation log on the server, never from the summary embedded in the document.

type By = "actor" | "kind" | "page";
type Row = {
  key: string;
  name?: string;
  kind?: string;
  share: string;
  operations: number;
  objects_created: number;
  objects_modified: number;
  properties_changed: number;
  net_authored: number;
};
type Totals = { share: string; operations: number };
type StatsReport = {
  by: By;
  matches: number;
  returned: number;
  has_more: boolean;
  total_operations: number;
  totals: { human: Totals; agent: Totals };
  rows: Row[];
  embedded_summary: { status: "missing" | "verified" | "mismatch" };
};

export function StatsPanel() {
  const [by, setBy] = useState<By>("actor");
  const [report, setReport] = useState<StatsReport | null>(null);
  const [error, setError] = useState<string | null>(null);
  const refresh = useCallback(async () => {
    try {
      setReport(await getJson<StatsReport>(`/api/collab/stats?by=${by}&limit=200`));
      setError(null);
    } catch (e) {
      setReport(null);
      setError(e instanceof Error ? e.message : "Could not load contribution stats.");
    }
  }, [by]);
  useEffect(() => {
    void refresh();
  }, [refresh]);
  return (
    <div className="space-y-3 p-2">
      <div className="flex items-center gap-2">
        <h3 className="text-sm font-semibold">Contributions</h3>
        <Badge variant="outline">read only</Badge>
        <Button variant="ghost" size="sm" className="ml-auto" onClick={() => void refresh()}>
          Refresh
        </Button>
      </div>
      <div className="flex gap-1">
        {(["actor", "kind", "page"] as const).map((option) => (
          <Button key={option} size="sm" variant={by === option ? "default" : "outline"} onClick={() => setBy(option)}>
            {option}
          </Button>
        ))}
      </div>
      {error && <p className="text-xs text-red-600">{error}</p>}
      {report && (
        <>
          <p className="text-xs text-muted-foreground">
            {report.total_operations} operations · humans {report.totals.human.share} · agents {report.totals.agent.share}
          </p>
          {report.embedded_summary.status === "mismatch" && (
            <p className="text-xs text-amber-600">
              The summary stored in the document disagrees with the log; these figures come from the log.
            </p>
          )}
          <table className="w-full text-xs">
            <thead>
              <tr className="text-left text-muted-foreground">
                <th>{by}</th>
                <th>share</th>
                <th>ops</th>
                <th>new</th>
                <th>mod</th>
                <th>props</th>
                <th>net</th>
              </tr>
            </thead>
            <tbody>
              {report.rows.map((row) => (
                <tr key={row.key}>
                  <td className="max-w-[8rem] truncate" title={row.key}>
                    {row.name ?? row.key}
                    {row.kind && by === "actor" ? ` (${row.kind})` : ""}
                  </td>
                  <td>{row.share}</td>
                  <td>{row.operations}</td>
                  <td>{row.objects_created}</td>
                  <td>{row.objects_modified}</td>
                  <td>{row.properties_changed}</td>
                  <td>{row.net_authored}</td>
                </tr>
              ))}
            </tbody>
          </table>
          {report.has_more && (
            <p className="text-xs text-muted-foreground">
              Showing {report.returned} of {report.matches}.
            </p>
          )}
          {report.rows.length === 0 && <p className="text-xs text-muted-foreground">No recorded operations yet.</p>}
        </>
      )}
    </div>
  );
}
