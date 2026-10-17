import { useCallback, useEffect, useState } from "react";
import { Button } from "@/components/ui/button";
import { Badge } from "@/components/ui/badge";
import { getJson, postJson } from "@/lib/api";
import { useStudio } from "@/store";

// Design systems (v0.19). Every view calls /api/design/*, which runs the same functions as
// `pentool design ...`; Studio adds no model of its own. Reads are free; template fill is
// guarded by the document revision and writes only a new file, never the template.

type Paged = { matches: number; returned: number; offset: number; limit: number; has_more: boolean };
type TokenRow = {
  token: string;
  type: string;
  value: unknown;
  state: "local" | "clean" | "override";
  dependents: number;
  source?: { library?: string; version?: string };
  resolved?: { status: string; value: unknown; chain: string[] };
};
type LibraryRow = { library: string; version: string; locked_version?: string | null; state: string; detail: string };
type TemplateRow = { file: string; name?: string; version?: string; parameters: number; content_hash: string };
type TemplateParam = { name: string; type: string; required: boolean; targets: number };
type Finding = { code: string; severity: string; page?: string; id: string; kind: string; path: string; suggestion?: unknown };
type AuditReport = Paged & {
  findings: Finding[];
  severities: { error: number; warn: number; info: number };
  plan?: { operations: unknown[]; page: string | null; skipped: unknown[] };
};
type FillReport = {
  dry_run: boolean;
  output: string;
  would_change: boolean;
  changes: unknown[];
  parameters: Record<string, { applied: unknown }>;
};

const tabs = ["tokens", "libraries", "templates", "audit"] as const;
type Tab = (typeof tabs)[number];

function show(value: unknown): string {
  return typeof value === "string" ? value : JSON.stringify(value);
}

function useDesignQuery<T>(path: string) {
  const { revision } = useStudio();
  const [data, setData] = useState<T | null>(null);
  const [error, setError] = useState<string | null>(null);
  const refresh = useCallback(async () => {
    try {
      setData(await getJson<T>(path));
      setError(null);
    } catch (e) {
      setData(null);
      setError(e instanceof Error ? e.message : "Request failed.");
    }
  }, [path]);
  // Re-read whenever the shared document changes.
  useEffect(() => {
    void refresh();
  }, [refresh, revision]);
  return { data, error, refresh };
}

function Problem({ error }: { error: string | null }) {
  return error ? <p className="text-xs text-red-600">{error}</p> : null;
}

function Pager({ page, onOffset }: { page: Paged; onOffset: (n: number) => void }) {
  if (page.matches <= page.limit) return null;
  return (
    <div className="flex items-center gap-2 text-xs">
      <Button size="sm" variant="outline" disabled={page.offset === 0} onClick={() => onOffset(Math.max(0, page.offset - page.limit))}>
        Prev
      </Button>
      <span>
        {page.offset + 1}-{page.offset + page.returned} of {page.matches}
      </span>
      <Button size="sm" variant="outline" disabled={!page.has_more} onClick={() => onOffset(page.offset + page.limit)}>
        Next
      </Button>
    </div>
  );
}

function TokensView() {
  const [type, setType] = useState("");
  const [offset, setOffset] = useState(0);
  const q = new URLSearchParams({ resolve: "true", limit: "50", offset: String(offset) });
  if (type) q.set("type", type);
  const { data, error } = useDesignQuery<Paged & { tokens: TokenRow[] }>(`/api/design/tokens?${q}`);
  return (
    <div className="space-y-2">
      <label className="block text-xs">
        Type
        <select
          className="ml-2 rounded border bg-background p-0.5"
          value={type}
          onChange={(e) => {
            setType(e.target.value);
            setOffset(0);
          }}
        >
          <option value="">all</option>
          {["color", "spacing", "radius", "font", "number"].map((t) => (
            <option key={t}>{t}</option>
          ))}
        </select>
      </label>
      <Problem error={error} />
      {data && data.matches === 0 && (
        <p className="text-xs text-muted-foreground">
          No tokens. Install a library with <code>pentool design library install</code>.
        </p>
      )}
      {data?.tokens.map((t) => (
        <div key={t.token} className="flex items-center gap-2 rounded border p-1.5 text-xs">
          {t.type === "color" && typeof t.resolved?.value === "string" && (
            <span aria-hidden className="inline-block h-4 w-4 shrink-0 rounded border" style={{ background: t.resolved.value }} />
          )}
          <div className="min-w-0 flex-1">
            <div className="truncate font-medium">{t.token}</div>
            <div className="truncate text-muted-foreground">
              {show(t.value)}
              {t.resolved && t.resolved.chain.length > 1 ? ` → ${show(t.resolved.value)}` : ""}
            </div>
          </div>
          {t.resolved && t.resolved.status !== "resolved" && <Badge variant="warning">{t.resolved.status}</Badge>}
          <Badge variant="outline">{t.state}</Badge>
          <span title="Objects and tokens that use this token">{t.dependents} uses</span>
        </div>
      ))}
      {data && <Pager page={data} onOffset={setOffset} />}
    </div>
  );
}

function LibrariesView() {
  const { data, error, refresh } = useDesignQuery<{ ok: boolean; libraries: LibraryRow[] }>("/api/design/library/status");
  return (
    <div className="space-y-2">
      <div className="flex items-center gap-2">
        <span className="text-xs">
          Document, <code>pentool.lock</code> and installed packages
        </span>
        <Button size="sm" variant="ghost" className="ml-auto" onClick={() => void refresh()}>
          Refresh
        </Button>
      </div>
      <Problem error={error} />
      {data?.libraries.length === 0 && (
        <p className="text-xs text-muted-foreground">
          No libraries vendored. Install with <code>pentool design library install PACKAGE DOC</code>; plan updates with <code>--dry-run</code> in the CLI.
        </p>
      )}
      {data?.libraries.map((l) => (
        <div key={l.library} className="rounded border p-1.5 text-xs">
          <div className="flex items-center gap-2">
            <strong>{l.library}</strong>
            <span>{l.version}</span>
            <Badge variant={l.state === "ok" ? "outline" : "warning"}>{l.state}</Badge>
          </div>
          {l.detail && <p className="text-muted-foreground">{l.detail}</p>}
          {l.locked_version && l.locked_version !== l.version && <p>Locked at {l.locked_version}</p>}
        </div>
      ))}
    </div>
  );
}

function TemplatesView() {
  const { shared, dirty, revision, notify } = useStudio();
  const list = useDesignQuery<Paged & { templates: TemplateRow[] }>("/api/design/templates?limit=100");
  const check = useDesignQuery<{ name: string; version: string; parameters: TemplateParam[] }>("/api/design/template/check");
  const [values, setValues] = useState<Record<string, string>>({});
  const [output, setOutput] = useState("filled.pen");
  const [report, setReport] = useState<FillReport | null>(null);
  const [busy, setBusy] = useState(false);
  const blocked = !shared ? "Open a shared document with `pentool serve FILE`." : dirty ? "Save or discard local edits first." : null;

  const coerce = () => {
    const out: Record<string, unknown> = {};
    for (const p of check.data?.parameters ?? []) {
      const raw = values[p.name];
      if (raw === undefined || raw === "") continue;
      out[p.name] = p.type === "number" ? Number(raw) : p.type === "bool" ? raw === "true" : raw;
    }
    return out;
  };
  const fill = async (dryRun: boolean) => {
    setBusy(true);
    try {
      const out = await postJson<FillReport>("/api/design/template/fill", {
        values: coerce(),
        output,
        revision,
        dry_run: dryRun,
      });
      setReport(out);
      notify(dryRun ? "Preview only, nothing written." : `Wrote ${out.output}. The template was not changed.`);
      if (!dryRun) void list.refresh();
    } catch (e) {
      notify(e instanceof Error ? e.message : "Fill failed.", true);
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="space-y-2">
      <Problem error={list.error} />
      <p className="text-xs text-muted-foreground">Templates in the served folder:</p>
      {list.data?.templates.length === 0 && (
        <p className="text-xs text-muted-foreground">
          None. A template is a .pen with <code>design.template</code>.
        </p>
      )}
      {list.data?.templates.map((t) => (
        <div key={t.file} className="rounded border p-1.5 text-xs">
          <strong>{t.name ?? t.file}</strong> {t.version}{" "}
          <span className="text-muted-foreground">
            {t.file} · {t.parameters} parameters
          </span>
        </div>
      ))}
      <div className="space-y-1.5 rounded border p-2">
        <strong className="text-xs">Fill the open document{check.data ? `: ${check.data.name} ${check.data.version}` : ""}</strong>
        <Problem error={check.error} />
        {check.data?.parameters.map((p) => (
          <label key={p.name} className="block text-xs">
            {p.name}{" "}
            <span className="text-muted-foreground">
              ({p.type}
              {p.required ? ", required" : ""}, {p.targets} targets)
            </span>
            <input className="mt-0.5 w-full rounded border bg-background p-1" value={values[p.name] ?? ""} onChange={(e) => setValues({ ...values, [p.name]: e.target.value })} />
          </label>
        ))}
        <label className="block text-xs">
          Output file
          <input className="mt-0.5 w-full rounded border bg-background p-1" value={output} onChange={(e) => setOutput(e.target.value)} />
        </label>
        {blocked && <p className="text-xs text-amber-600">{blocked}</p>}
        <div className="flex gap-1">
          <Button size="sm" variant="outline" disabled={busy || !!blocked || !check.data} onClick={() => void fill(true)}>
            Preview
          </Button>
          <Button size="sm" disabled={busy || !!blocked || !check.data} onClick={() => void fill(false)}>
            Fill
          </Button>
        </div>
        {report && (
          <div className="text-xs">
            {report.dry_run ? "Preview" : "Written"}: {report.output} · {report.changes.length} change(s)
            <pre className="max-h-32 overflow-auto font-mono">{JSON.stringify(report.parameters, null, 2)}</pre>
          </div>
        )}
      </div>
    </div>
  );
}

function AuditView() {
  const { shared, dirty, revision, pageId, load, notify } = useStudio();
  const [offset, setOffset] = useState(0);
  const [nearest, setNearest] = useState(false);
  const q = new URLSearchParams({ limit: "50", offset: String(offset), fix_plan: "true" });
  if (nearest) q.set("fix_nearest", "true");
  const { data, error, refresh } = useDesignQuery<AuditReport>(`/api/design/audit?${q}`);
  const [busy, setBusy] = useState(false);
  const ops = data?.plan?.operations ?? [];
  const blocked = !shared ? "Open a shared document with `pentool serve FILE`." : dirty ? "Save or discard local edits first." : null;
  const apply = async () => {
    setBusy(true);
    try {
      // The fix plan is ordinary batch operations: same revision guard and transaction as any edit.
      await postJson("/api/scene", { operations: ops, page: data?.plan?.page ?? pageId, revision });
      notify(`Applied ${ops.length} fix(es). Undo reverts them.`);
      await load();
      await refresh();
    } catch (e) {
      notify(e instanceof Error ? e.message : "Could not apply the fix plan.", true);
    } finally {
      setBusy(false);
    }
  };
  return (
    <div className="space-y-2">
      <Problem error={error} />
      {data && (
        <div className="flex flex-wrap items-center gap-2 text-xs">
          <Badge variant={data.severities.error ? "warning" : "outline"}>{data.severities.error} errors</Badge>
          <Badge variant="outline">{data.severities.warn} warnings</Badge>
          <Badge variant="outline">{data.severities.info} info</Badge>
          <label className="ml-auto flex items-center gap-1">
            <input type="checkbox" checked={nearest} onChange={(e) => setNearest(e.target.checked)} /> include near matches
          </label>
        </div>
      )}
      {data && data.matches === 0 && <p className="text-xs text-muted-foreground">No findings.</p>}
      {data?.findings.map((f, i) => (
        <div key={`${f.id}-${f.path}-${i}`} className="rounded border p-1.5 text-xs">
          <div className="flex items-center gap-2">
            <Badge variant={f.severity === "error" ? "warning" : "outline"}>{f.severity}</Badge>
            <strong>{f.code}</strong>
            <span className="truncate text-muted-foreground">
              {f.id} · {f.path}
            </span>
          </div>
          {f.suggestion !== undefined && <div className="text-muted-foreground">Suggest {show(f.suggestion)}</div>}
        </div>
      ))}
      {data && <Pager page={data} onOffset={setOffset} />}
      {ops.length > 0 && (
        <div className="space-y-1 rounded border p-2 text-xs">
          Fix plan: {ops.length} operation(s) bind matching values to tokens.
          {blocked && <p className="text-amber-600">{blocked}</p>}
          <div>
            <Button size="sm" disabled={busy || !!blocked} onClick={() => void apply()}>
              Apply fix plan
            </Button>
          </div>
        </div>
      )}
    </div>
  );
}

export function DesignSystemsPanel() {
  const [tab, setTab] = useState<Tab>("tokens");
  return (
    <div className="space-y-3 p-2">
      <div className="flex items-center gap-2">
        <h3 className="text-sm font-semibold">Design systems</h3>
        <Badge variant="outline">same as the CLI</Badge>
      </div>
      <div role="tablist" className="flex gap-1">
        {tabs.map((t) => (
          <Button key={t} role="tab" aria-selected={tab === t} size="sm" variant={tab === t ? "default" : "outline"} onClick={() => setTab(t)}>
            {t[0]!.toUpperCase() + t.slice(1)}
          </Button>
        ))}
      </div>
      {tab === "tokens" && <TokensView />}
      {tab === "libraries" && <LibrariesView />}
      {tab === "templates" && <TemplatesView />}
      {tab === "audit" && <AuditView />}
    </div>
  );
}
