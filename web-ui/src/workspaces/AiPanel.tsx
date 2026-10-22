import { useCallback, useEffect, useState } from "react";
import { Button } from "@/components/ui/button";
import { Badge } from "@/components/ui/badge";
import { getJson, postJson } from "@/lib/api";
import {
  ADAPTERS,
  connectBody,
  emptyForm,
  formFromCatalog,
  formProblem,
  type AiStatus,
  type DoctorProvider,
  type DoctorReport,
  type ProviderForm,
} from "@/lib/aiProvider";
import { AiGenerate } from "./AiGenerate";
import { useStudio } from "@/store";

// Provider setup for generative AI. Everything here goes through `/api/ai/*`, which reuses the
// CLI's `ai connect` and `ai doctor`. The browser never holds a key: a provider points at an
// environment variable or key file on the machine running `pentool serve`. Nothing contacts a
// model until you press "Test connection", and that call only lists models (non-billable).

const input = "mt-1 h-7 w-full rounded border bg-background px-1.5 text-xs";

function message(e: unknown, fallback: string) {
  return e instanceof Error ? e.message : fallback;
}

function Result({ report }: { report: DoctorProvider }) {
  const tested = report.check;
  return (
    <div className="mt-2 space-y-1 rounded border bg-muted/30 p-2 text-xs" role="status">
      <div className="flex flex-wrap items-center gap-1">
        <Badge variant={report.ready ? "success" : "destructive"}>{report.ready ? "Ready" : "Needs attention"}</Badge>
        {tested && (
          <Badge variant={tested.ok ? "success" : "destructive"}>
            {tested.ok ? `Connected, ${tested.image_models ?? 0} image models` : "Connection failed"}
          </Badge>
        )}
        {!tested && report.ready && <Badge variant="outline">Not tested online yet</Badge>}
      </div>
      {report.problems.map((p) => (
        <p key={p.code} className="text-red-600">
          <code>{p.code}</code>
          {p.fix ? `: ${p.fix}` : ""}
        </p>
      ))}
      {tested && !tested.ok && tested.message && <p className="break-words text-muted-foreground">{tested.message}</p>}
    </div>
  );
}

export function AiPanel() {
  const { notify } = useStudio();
  const [status, setStatus] = useState<AiStatus | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [form, setForm] = useState<ProviderForm>(emptyForm);
  const [formError, setFormError] = useState<string | null>(null);
  const [busy, setBusy] = useState<string | null>(null);
  const [reports, setReports] = useState<Record<string, DoctorProvider>>({});

  const refresh = useCallback(async () => {
    try {
      setStatus(await getJson<AiStatus>("/api/ai/status"));
      setError(null);
    } catch (e) {
      setError(message(e, "Could not read the AI settings."));
    }
  }, []);
  useEffect(() => {
    void refresh();
  }, [refresh]);

  const catalog = status?.enabled ? status.catalog : [];
  const configured = status?.enabled ? status.configured : [];
  const known = catalog.some((entry) => entry.name === form.name.trim());
  const set = (patch: Partial<ProviderForm>) => {
    setForm((f) => ({ ...f, ...patch }));
    setFormError(null);
  };

  const validate = async (name: string, check: boolean) => {
    setBusy(`${check ? "test" : "validate"}:${name}`);
    try {
      const report = await postJson<DoctorReport>("/api/ai/doctor", { provider: name, check });
      const entry = report.providers.find((p) => p.name === name);
      if (entry) setReports((r) => ({ ...r, [name]: entry }));
    } catch (e) {
      notify(message(e, "Validation failed."), true);
    } finally {
      setBusy(null);
    }
  };

  const connect = async () => {
    const problem = formProblem(form, catalog);
    if (problem) {
      setFormError(problem);
      return;
    }
    setBusy("connect");
    try {
      const out = await postJson<{ status: string }>("/api/ai/connect", connectBody(form));
      const name = form.name.trim();
      notify(out.status === "unchanged" ? `${name}: already saved.` : `${name}: ${out.status}.`);
      setForm(emptyForm());
      await refresh();
      await validate(name, false);
    } catch (e) {
      setFormError(message(e, "Could not save the provider."));
    } finally {
      setBusy(null);
    }
  };

  const remove = async (name: string) => {
    if (!window.confirm(`Remove provider "${name}"? Its key is not touched.`)) return;
    setBusy(`remove:${name}`);
    try {
      await postJson("/api/ai/remove", { name });
      setReports(({ [name]: _gone, ...rest }) => rest);
      notify(`${name}: removed.`);
      await refresh();
    } catch (e) {
      notify(message(e, "Could not remove the provider."), true);
    } finally {
      setBusy(null);
    }
  };

  return (
    <div className="space-y-3 p-2">
      <div className="flex items-center gap-2">
        <h3 className="text-sm font-semibold">AI providers</h3>
        <Badge variant="outline">optional</Badge>
        <Button variant="ghost" size="sm" className="ml-auto" onClick={() => void refresh()}>
          Refresh
        </Button>
      </div>
      <p className="text-xs text-muted-foreground">
        Connect an image model to generate and edit images. Pentool never stores or shows your key: put it in an
        environment variable (or a key file) on this machine and enter only its name. Testing lists models; generation requires a separate confirmation.
      </p>
      {error && <p className="text-xs text-red-600">{error}</p>}
      {status && !status.enabled && <p className="text-xs text-amber-600">AI is turned off for this server (PENTOOL_AI=off).</p>}

      {status?.enabled && <AiGenerate status={status} />}
      {status?.enabled && (
        <>
          <section aria-label="Connected providers" className="space-y-2">
            {configured.length === 0 && <p className="text-xs text-muted-foreground">No provider connected yet.</p>}
            {configured.map((p) => (
              <div key={p.name} className="rounded border p-2 text-xs">
                <div className="flex items-center gap-1">
                  <strong>{p.name}</strong>
                  <Badge variant="outline">{p.adapter}</Badge>
                  <Badge variant={p.credential.present ? "success" : "warning"}>
                    {p.credential.present ? "key found" : "key not found"}
                  </Badge>
                </div>
                <p className="mt-1 break-all text-muted-foreground">{p.endpoint}</p>
                <p className="text-muted-foreground">
                  Key from {p.credential.source === "env" ? `variable ${p.credential.name}` : `file ${p.credential.path}`}
                  {p.default_model ? ` · model ${p.default_model}` : ""}
                </p>
                <div className="mt-2 flex flex-wrap gap-1">
                  <Button size="sm" variant="outline" disabled={busy !== null} onClick={() => void validate(p.name, false)}>
                    {busy === `validate:${p.name}` ? "Checking…" : "Validate"}
                  </Button>
                  <Button
                    size="sm"
                    disabled={busy !== null}
                    onClick={() => void validate(p.name, true)}
                    title="Lists the provider's models. Not billed."
                  >
                    {busy === `test:${p.name}` ? "Testing…" : "Test connection"}
                  </Button>
                  <Button size="sm" variant="ghost" className="ml-auto" disabled={busy !== null} onClick={() => void remove(p.name)}>
                    Remove
                  </Button>
                </div>
                {reports[p.name] && <Result report={reports[p.name]!} />}
              </div>
            ))}
          </section>

          <section aria-label="Connect a provider" className="space-y-2 border-t pt-3">
            <h4 className="text-xs font-semibold">Connect a provider</h4>
            <div className="flex flex-wrap gap-1">
              {catalog.map((entry) => (
                <Button
                  key={entry.name}
                  size="sm"
                  variant="outline"
                  onClick={() => {
                    setForm(formFromCatalog(entry));
                    setFormError(null);
                  }}
                >
                  {entry.name}
                </Button>
              ))}
              <Button
                size="sm"
                variant="ghost"
                onClick={() => {
                  setForm(emptyForm());
                  setFormError(null);
                }}
              >
                Custom
              </Button>
            </div>
            <label className="block text-xs">
              Name
              <input className={input} value={form.name} onChange={(e) => set({ name: e.target.value })} placeholder="studio" spellCheck={false} />
            </label>
            <label className="block text-xs">
              Adapter
              <select className={input} value={form.adapter} onChange={(e) => set({ adapter: e.target.value })} disabled={known}>
                {ADAPTERS.map((a) => (
                  <option key={a} value={a}>
                    {a}
                  </option>
                ))}
              </select>
            </label>
            <label className="block text-xs">
              Endpoint {known && <span className="text-muted-foreground">(optional, catalog default)</span>}
              <input className={input} value={form.endpoint} onChange={(e) => set({ endpoint: e.target.value })} placeholder="https://host/v1" spellCheck={false} />
            </label>
            <div className="text-xs">
              <span>Where is the key?</span>
              <div className="mt-1 flex gap-3">
                <label className="flex items-center gap-1">
                  <input type="radio" name="ai-cred" checked={form.credentialKind === "env"} onChange={() => set({ credentialKind: "env", credential: "" })} />
                  Environment variable
                </label>
                <label className="flex items-center gap-1">
                  <input type="radio" name="ai-cred" checked={form.credentialKind === "file"} onChange={() => set({ credentialKind: "file", credential: "" })} />
                  Key file
                </label>
              </div>
              <input
                className={input}
                value={form.credential}
                onChange={(e) => set({ credential: e.target.value })}
                placeholder={form.credentialKind === "env" ? "GEMINI_API_KEY" : "C:/Users/you/.keys/provider.txt"}
                spellCheck={false}
                autoComplete="off"
                aria-label={form.credentialKind === "env" ? "Environment variable name" : "Key file path"}
              />
              <p className="mt-1 text-muted-foreground">Enter the name or path only. Never paste the key itself here.</p>
            </div>
            <label className="block text-xs">
              Default model <span className="text-muted-foreground">(optional)</span>
              <input className={input} value={form.defaultModel} onChange={(e) => set({ defaultModel: e.target.value })} placeholder="gemini-2.5-flash-image" spellCheck={false} />
            </label>
            {formError && (
              <p className="text-xs text-red-600" role="alert">
                {formError}
              </p>
            )}
            <Button disabled={busy !== null} onClick={() => void connect()}>
              {busy === "connect" ? "Saving…" : "Save and validate"}
            </Button>
          </section>
        </>
      )}
    </div>
  );
}
