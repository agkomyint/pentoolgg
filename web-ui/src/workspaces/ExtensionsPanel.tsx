import { useCallback, useEffect, useMemo, useState } from "react";
import { Button } from "@/components/ui/button";
import { Badge } from "@/components/ui/badge";
import { getJson, postJson } from "@/lib/api";
import { useStudio } from "@/store";

// Sandboxed extensions (v0.18). Studio only lists and runs what `pentool extension add
// --approve` installed; the run is the same host and transaction as the CLI, so a preview
// and a commit report the same thing and a failed run changes nothing.

type ExtensionCommand = { id: string; title: string };
export type InstalledExtension = {
  name: string;
  version?: string;
  description?: string;
  capabilities?: string[];
  limits?: { cpu_ms: number; memory_mb: number };
  commands?: ExtensionCommand[];
  status: "ok" | "broken";
  error?: string;
};
type RunReport = {
  changed?: boolean;
  dry_run?: boolean;
  calls?: number;
  logs?: string[];
  result?: unknown;
  change?: { revision_after?: string };
};

export function useExtensions() {
  const [items, setItems] = useState<InstalledExtension[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const refresh = useCallback(async () => {
    try {
      const listed = await getJson<{ extensions: InstalledExtension[] }>("/api/extensions");
      setItems(listed.extensions);
      setError(null);
    } catch (e) {
      setError(e instanceof Error ? e.message : "Could not list extensions.");
    }
  }, []);
  useEffect(() => {
    void refresh();
  }, [refresh]);
  return { items, error, refresh };
}

/** Runs one extension command against the shared document, then reloads it. */
export function useExtensionRunner() {
  const { shared, dirty, revision, load, notify } = useStudio();
  const [busy, setBusy] = useState(false);
  const [report, setReport] = useState<(RunReport & { name: string }) | null>(null);
  const blocked = !shared ? "Open a shared document with `pentool serve FILE`." : dirty ? "Save or discard local edits first." : null;
  const run = useCallback(
    async (name: string, command: string | undefined, args: unknown, dryRun: boolean) => {
      if (blocked) {
        notify(blocked, true);
        return;
      }
      setBusy(true);
      try {
        const out = await postJson<RunReport>("/api/extensions/run", {
          name,
          command,
          args,
          dry_run: dryRun,
          revision,
        });
        setReport({ ...out, name });
        if (!dryRun && out.changed) await load();
        notify(dryRun ? `${name}: preview only, nothing written.` : out.changed ? `${name}: applied. Undo reverts it.` : `${name}: no changes.`);
      } catch (e) {
        notify(e instanceof Error ? e.message : "Extension failed.", true);
      } finally {
        setBusy(false);
      }
    },
    [blocked, revision, load, notify],
  );
  return { run, busy, report, blocked };
}

export function ExtensionsPanel() {
  const { items, error, refresh } = useExtensions();
  const { run, busy, report, blocked } = useExtensionRunner();
  const [args, setArgs] = useState("{}");
  let parsed: unknown = {};
  let argsError: string | null = null;
  try {
    parsed = JSON.parse(args || "{}");
  } catch {
    argsError = "Arguments must be valid JSON.";
  }
  return (
    <div className="space-y-3 p-2">
      <div className="flex items-center gap-2">
        <h3 className="text-sm font-semibold">Extensions</h3>
        <Badge variant="outline">sandboxed</Badge>
        <Button variant="ghost" size="sm" className="ml-auto" onClick={() => void refresh()}>
          Refresh
        </Button>
      </div>
      <p className="text-xs text-muted-foreground">
        Extensions run as WebAssembly with no files, network or clock. They read the document and propose edits that
        commit as one undoable step. Install with <code>pentool extension add FILE.penext --approve</code>.
      </p>
      {blocked && <p className="text-xs text-amber-600">{blocked}</p>}
      {error && <p className="text-xs text-red-600">{error}</p>}
      {items?.length === 0 && <p className="text-xs text-muted-foreground">No extensions installed in this project.</p>}
      <label className="block text-xs">
        Arguments (JSON)
        <textarea
          className="mt-1 h-16 w-full rounded border bg-background p-1 font-mono text-xs"
          value={args}
          onChange={(e) => setArgs(e.target.value)}
          spellCheck={false}
        />
      </label>
      {argsError && <p className="text-xs text-red-600">{argsError}</p>}
      {items?.map((ext) => (
        <div key={ext.name} className="space-y-1.5 rounded border p-2">
          <div className="flex items-center gap-2">
            <strong className="text-xs">{ext.name}</strong>
            {ext.version && <span className="text-xs text-muted-foreground">{ext.version}</span>}
            {ext.status === "broken" && <Badge variant="warning">broken</Badge>}
          </div>
          {ext.error && <p className="text-xs text-red-600">{ext.error}</p>}
          {ext.description && <p className="text-xs text-muted-foreground">{ext.description}</p>}
          {ext.capabilities && (
            <p className="text-xs">
              Allowed to: {ext.capabilities.length ? ext.capabilities.join(", ") : "nothing"}
              {ext.limits ? ` · ${ext.limits.cpu_ms} ms CPU, ${ext.limits.memory_mb} MB` : ""}
            </p>
          )}
          {ext.status === "ok" &&
            (ext.commands?.length ? ext.commands : [{ id: "", title: "Run" }]).map((cmd) => (
              <div key={cmd.id} className="flex items-center gap-1">
                <span className="flex-1 text-xs">{cmd.title}</span>
                <Button size="sm" variant="outline" disabled={busy || !!blocked || !!argsError} onClick={() => void run(ext.name, cmd.id || undefined, parsed, true)}>
                  Preview
                </Button>
                <Button size="sm" disabled={busy || !!blocked || !!argsError} onClick={() => void run(ext.name, cmd.id || undefined, parsed, false)}>
                  Run
                </Button>
              </div>
            ))}
        </div>
      ))}
      {report && (
        <div className="space-y-1 rounded border p-2 text-xs">
          <strong>
            {report.name}: {report.dry_run ? "preview" : report.changed ? "applied" : "no changes"}
          </strong>
          {report.logs?.map((line, i) => (
            <div key={i} className="font-mono">
              {line}
            </div>
          ))}
          {report.result !== undefined && report.result !== null && (
            <pre className="max-h-40 overflow-auto font-mono">{JSON.stringify(report.result, null, 2)}</pre>
          )}
        </div>
      )}
    </div>
  );
}

/** Ctrl/Cmd+K: filter every installed extension command and run it. */
export function ExtensionPalette() {
  const [open, setOpen] = useState(false);
  const [query, setQuery] = useState("");
  const { items, refresh } = useExtensions();
  const { run, busy, blocked } = useExtensionRunner();
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === "k") {
        e.preventDefault();
        setOpen((v) => !v);
        void refresh();
      } else if (e.key === "Escape") {
        setOpen(false);
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [refresh]);
  const commands = useMemo(
    () =>
      (items ?? [])
        .filter((x) => x.status === "ok")
        .flatMap((x) => (x.commands?.length ? x.commands : [{ id: "", title: "Run" }]).map((c) => ({ ext: x.name, ...c })))
        .filter((c) => `${c.ext} ${c.title}`.toLowerCase().includes(query.toLowerCase())),
    [items, query],
  );
  if (!open) return null;
  return (
    <div className="fixed inset-0 z-50 flex items-start justify-center bg-black/30 pt-24" onClick={() => setOpen(false)}>
      <div role="dialog" aria-label="Extension commands" className="w-96 max-w-[90vw] rounded border bg-card p-2 shadow-lg" onClick={(e) => e.stopPropagation()}>
        <input
          autoFocus
          className="mb-2 w-full rounded border bg-background p-1 text-sm"
          placeholder="Run an extension command…"
          value={query}
          onChange={(e) => setQuery(e.target.value)}
        />
        {blocked && <p className="mb-1 text-xs text-amber-600">{blocked}</p>}
        <ul className="max-h-64 overflow-auto">
          {commands.map((c) => (
            <li key={`${c.ext}/${c.id}`}>
              <button
                className="w-full rounded px-2 py-1 text-left text-sm hover:bg-accent disabled:opacity-50"
                disabled={busy || !!blocked}
                onClick={() => {
                  setOpen(false);
                  void run(c.ext, c.id || undefined, {}, false);
                }}
              >
                {c.title} <span className="text-xs text-muted-foreground">{c.ext}</span>
              </button>
            </li>
          ))}
          {commands.length === 0 && <li className="px-2 py-1 text-xs text-muted-foreground">No matching extension commands.</li>}
        </ul>
      </div>
    </div>
  );
}
