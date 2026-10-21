// Provider setup for generative AI. The server (`/api/ai/*`) is the authority and reuses
// the CLI's checks; these helpers only catch obvious mistakes before a round trip. The
// browser never handles an API key: a provider names an environment variable or a key file.

export type Credential = { source: "env"; name: string; present: boolean } | { source: "file"; path: string; present: boolean };
export type Provider = {
  name: string;
  adapter: string;
  endpoint: string;
  credential: Credential;
  default_model?: string | null;
  capabilities: string[];
};
export type CatalogEntry = { name: string; adapter: string; endpoint: string; env: string[]; default_model: string; capabilities: string[] };
export type AiStatus = { enabled: false } | { enabled: true; configured: Provider[]; catalog: CatalogEntry[]; defaults: Record<string, string> };
export type Problem = { code: string; fix?: string; message?: string };
export type DoctorProvider = Provider & {
  ready: boolean;
  problems: Problem[];
  check?: { ok: boolean; image_models?: number; code?: string; message?: string; fix?: string };
};
export type DoctorReport = { ok: boolean; providers: DoctorProvider[]; checked_network?: boolean };

export const ADAPTERS = ["gemini", "openai-compatible"] as const;
export type CredentialKind = "env" | "file";

export type ProviderForm = {
  name: string;
  adapter: string;
  endpoint: string;
  credentialKind: CredentialKind;
  credential: string;
  defaultModel: string;
};

export const emptyForm = (): ProviderForm => ({ name: "", adapter: "openai-compatible", endpoint: "", credentialKind: "env", credential: "", defaultModel: "" });

/** Starts the form from a catalog entry, so a known provider needs only a key variable. */
export function formFromCatalog(entry: CatalogEntry): ProviderForm {
  return { name: entry.name, adapter: entry.adapter, endpoint: entry.endpoint, credentialKind: "env", credential: entry.env[0] ?? "", defaultModel: entry.default_model };
}

const LOOPBACK = new Set(["localhost", "127.0.0.1", "[::1]"]);

export function endpointProblem(endpoint: string): string | null {
  let url: URL;
  try {
    url = new URL(endpoint);
  } catch {
    return "Enter a full URL such as https://host/v1.";
  }
  if (url.username || url.password) return "The endpoint cannot contain credentials.";
  if (url.hash) return "The endpoint cannot contain a #fragment.";
  if (url.protocol === "https:") return null;
  if (url.protocol === "http:" && LOOPBACK.has(url.hostname)) return null;
  return "Use https. Plain http is only allowed for localhost.";
}

/** Returns the first problem with the form, or null when it is worth sending. */
export function formProblem(form: ProviderForm, catalog: CatalogEntry[]): string | null {
  const name = form.name.trim();
  if (!name) return "Give the provider a name.";
  if (/[\s/]/.test(name)) return "The name cannot contain spaces or slashes.";
  const known = catalog.some((entry) => entry.name === name);
  if (!form.adapter) return "Choose an adapter.";
  if (!known || form.endpoint.trim()) {
    if (!form.endpoint.trim()) return "An endpoint is required for a provider outside the catalog.";
    const bad = endpointProblem(form.endpoint.trim());
    if (bad) return bad;
  }
  const credential = form.credential.trim();
  if (!credential) return form.credentialKind === "env" ? "Name the environment variable that holds the key." : "Give the path of the key file.";
  if (form.credentialKind === "env" && !/^[A-Za-z_][A-Za-z0-9_]*$/.test(credential)) {
    return "That looks like a key, not a variable name. Paste the key into your environment and enter only its name.";
  }
  const model = form.defaultModel.trim().replace(/^[^/]+\//, "");
  if (model && !/^[A-Za-z0-9._:-]{1,96}$/.test(model)) return "Model ids use letters, digits, '.', '-', '_' and ':'.";
  return null;
}

/** The request body for `/api/ai/connect`; empty fields are left out so catalog defaults apply. */
export function connectBody(form: ProviderForm) {
  const clean = (value: string) => value.trim() || undefined;
  return {
    name: form.name.trim(),
    adapter: clean(form.adapter),
    endpoint: clean(form.endpoint),
    credential_env: form.credentialKind === "env" ? clean(form.credential) : undefined,
    credential_file: form.credentialKind === "file" ? clean(form.credential) : undefined,
    default_model: clean(form.defaultModel),
  };
}
