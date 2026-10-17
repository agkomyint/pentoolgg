// Collaboration client (v0.20). The server enforces every role; this file only
// carries the Bearer token and reads what the server reports.
import { create } from "zustand";

export type Role = "viewer" | "commenter" | "editor" | "owner";
export type Actor = { kind: "human" | "agent"; id: string; name?: string; directed_by?: string };
export type Session = {
  protocol: number;
  role: Role;
  actor: Actor;
  may: { read: boolean; comment: boolean; edit: boolean; manage: boolean };
  peers: unknown;
  claims: unknown;
  head: { seq: number; hash: string };
  limits: Record<string, number>;
  presence: { updates_per_second: number; entries: number; ttl_ms: number; selection: number };
};
export type PresenceEntry = {
  actor: Actor;
  page?: string;
  selection: string[];
  cursor?: { x: number; y: number };
  unknown_page?: boolean;
  unknown_ids?: string[];
  age_ms: number;
};
export type Conflict = {
  id: string;
  code: string;
  page: string;
  object: string;
  property?: string;
  winning_op?: string;
  losing_op: string;
  winning_actor?: string;
  losing_actor: string;
  winning_value?: unknown;
  losing_value?: unknown;
  status: string;
};
export type ConflictReport = {
  chain: string;
  ops: number;
  conflicts_total: number;
  conflicts: Conflict[];
  rejected: { op: string; code: string }[];
};

const TOKEN_KEY = "pentool-collab-token";

function loadToken(): string {
  try {
    return sessionStorage.getItem(TOKEN_KEY) ?? "";
  } catch {
    return "";
  }
}

let token = loadToken();
let patched = false;

/** Send the token with every same-origin API call so the whole Studio works on a token-protected server. */
function installFetchAuth() {
  if (patched) return;
  patched = true;
  const original = window.fetch.bind(window);
  window.fetch = (input, init) => {
    if (!token) return original(input, init);
    const url = typeof input === "string" ? input : input instanceof URL ? input.href : input.url;
    const local = url.startsWith("/") || url.startsWith(window.location.origin);
    if (!local || !url.includes("/api/")) return original(input, init);
    const headers = new Headers(init?.headers ?? (input instanceof Request ? input.headers : undefined));
    if (!headers.has("authorization")) headers.set("authorization", `Bearer ${token}`);
    return original(input, { ...init, headers });
  };
}

export function setToken(value: string) {
  token = value.trim();
  try {
    if (token) sessionStorage.setItem(TOKEN_KEY, token);
    else sessionStorage.removeItem(TOKEN_KEY);
  } catch {
    // storage can be unavailable; the token then lives in memory only
  }
  installFetchAuth();
}
export const hasToken = () => token !== "";
if (token) installFetchAuth();

async function failure(response: Response): Promise<Error> {
  let message = `${response.status} ${response.statusText}`;
  try {
    const body = await response.json();
    const detail = body?.error ?? body;
    if (typeof detail === "string") message = detail;
    else if (detail?.message) message = `${detail.message}${detail.action ? ` (${detail.action})` : ""}`;
  } catch {
    // non-JSON body
  }
  return new Error(message);
}

export async function collabGet<T>(path: string): Promise<T> {
  const response = await fetch(path);
  if (!response.ok) throw await failure(response);
  return response.json();
}

export async function collabPost<T>(path: string, body: unknown): Promise<T> {
  const response = await fetch(path, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify(body),
  });
  if (!response.ok) throw await failure(response);
  return response.json();
}

/** Read a server-sent event stream with fetch so the Authorization header can be sent. */
export function streamEvents(path: string, onEvent: (name: string, data: string) => void, onEnd: () => void): () => void {
  const controller = new AbortController();
  void (async () => {
    try {
      const response = await fetch(path, { signal: controller.signal, headers: { accept: "text/event-stream" } });
      if (!response.ok || !response.body) return;
      const reader = response.body.getReader();
      const decoder = new TextDecoder();
      let buffer = "";
      for (;;) {
        const { done, value } = await reader.read();
        if (done) break;
        buffer += decoder.decode(value, { stream: true }).replace(/\r\n/g, "\n");
        let split = buffer.indexOf("\n\n");
        while (split >= 0) {
          const block = buffer.slice(0, split);
          buffer = buffer.slice(split + 2);
          let name = "message";
          const data: string[] = [];
          for (const line of block.split("\n")) {
            if (line.startsWith("event:")) name = line.slice(6).trim();
            else if (line.startsWith("data:")) data.push(line.slice(5).replace(/^ /, ""));
          }
          if (data.length) onEvent(name, data.join("\n"));
          split = buffer.indexOf("\n\n");
        }
      }
    } catch {
      // aborted or dropped; the caller reconnects
    } finally {
      if (!controller.signal.aborted) onEnd();
    }
  })();
  return () => controller.abort();
}

type CollabState = {
  /** "unknown" until the first probe; "off" when the server does not run collaboration. */
  status: "unknown" | "off" | "denied" | "on";
  session: Session | null;
  error: string | null;
  probe: () => Promise<void>;
};

export const useCollab = create<CollabState>((set) => ({
  status: "unknown",
  session: null,
  error: null,
  async probe() {
    try {
      const response = await fetch("/api/collab/session");
      if (response.status === 401 || response.status === 403) {
        set({ status: "denied", session: null, error: (await failure(response)).message });
        return;
      }
      if (!response.ok) {
        set({ status: "off", session: null, error: null });
        return;
      }
      set({ status: "on", session: (await response.json()) as Session, error: null });
    } catch (e) {
      set({ status: "off", session: null, error: e instanceof Error ? e.message : null });
    }
  },
}));

export const displayName = (actor: Actor) => actor.name ?? actor.id;
