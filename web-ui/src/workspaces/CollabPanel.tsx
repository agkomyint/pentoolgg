import { useCallback, useEffect, useRef, useState } from "react";
import { Button } from "@/components/ui/button";
import { Badge } from "@/components/ui/badge";
import { Empty, Row, Section } from "@/components/ui/kit";
import { useStudio } from "@/store";
import {
  collabGet,
  collabPost,
  displayName,
  hasToken,
  setToken,
  streamEvents,
  useCollab,
  type Actor,
  type ConflictReport,
  type PresenceEntry,
} from "@/lib/collab";

// Collaboration workspace (v0.20). Every control is gated by the role the server reports, and the
// server enforces it again on each request. The panel degrades to "not enabled" when the server
// was not started with collaboration; comments still work there because they are plain review data.

const errorText = (e: unknown) => (e instanceof Error ? e.message : String(e));

function ConnectionSection() {
  const { status, session, error, probe } = useCollab();
  const [value, setValue] = useState("");
  const connect = async () => {
    setToken(value);
    setValue("");
    await probe();
  };
  return (
    <Section id="collab-connection" title="Connection">
      {status === "off" && (
        <Empty>
          Collaboration is not enabled on this server. Start it with <code>pentool serve --collab</code> to share this
          document. Comments below still work for a single editor.
        </Empty>
      )}
      {status === "denied" && <p className="text-xs text-amber-600">{error ?? "A token is required."}</p>}
      {status === "on" && session && (
        <div className="space-y-1 text-xs">
          <Row label="You">
            <span>
              {displayName(session.actor)} <Badge variant="outline">{session.actor.kind}</Badge>
            </span>
          </Row>
          <Row label="Role">
            <Badge>{session.role}</Badge>
          </Row>
          <Row label="Log head">
            <span className="tabular-nums">#{session.head.seq}</span>
          </Row>
          {session.role === "viewer" && <p className="text-muted-foreground">Read only: you can watch but not comment or edit.</p>}
          {session.role === "commenter" && <p className="text-muted-foreground">You can comment but not edit.</p>}
        </div>
      )}
      <div className="mt-2 flex gap-1">
        <input
          type="password"
          autoComplete="off"
          className="min-w-0 flex-1 rounded border bg-background px-2 py-1 text-xs"
          placeholder={hasToken() ? "Token set (this tab only)" : "Access token"}
          value={value}
          onChange={(e) => setValue(e.target.value)}
        />
        <Button size="sm" disabled={!value.trim()} onClick={() => void connect()}>
          Connect
        </Button>
        {hasToken() && (
          <Button
            size="sm"
            variant="ghost"
            onClick={() => {
              setToken("");
              void probe();
            }}
          >
            Forget
          </Button>
        )}
      </div>
    </Section>
  );
}

/** Publish this editor's page and selection (at most every 100 ms, heartbeat every 5 s) and follow the table. */
function usePresence(enabled: boolean, mayPublish: boolean): PresenceEntry[] {
  const [entries, setEntries] = useState<PresenceEntry[]>([]);
  const pageId = useStudio((s) => s.pageId);
  const selectedId = useStudio((s) => s.selectedId);
  const latest = useRef({ pageId, selectedId });
  latest.current = { pageId, selectedId };
  const lastSent = useRef(0);
  const timer = useRef<number | undefined>(undefined);

  useEffect(() => {
    if (!enabled) {
      setEntries([]);
      return;
    }
    let stop = () => {};
    let closed = false;
    let retry: number | undefined;
    const connect = () => {
      stop = streamEvents(
        "/api/collab/presence/events",
        (name, data) => {
          if (name !== "presence") return;
          try {
            setEntries((JSON.parse(data) as { entries: PresenceEntry[] }).entries);
          } catch {
            // ignore a malformed frame
          }
        },
        () => {
          if (!closed) retry = window.setTimeout(connect, 3000);
        },
      );
    };
    connect();
    return () => {
      closed = true;
      stop();
      window.clearTimeout(retry);
    };
  }, [enabled]);

  useEffect(() => {
    if (!enabled || !mayPublish) return;
    const send = (body: unknown) => {
      lastSent.current = Date.now();
      void collabPost("/api/collab/presence", body).catch(() => {});
    };
    const state = () => ({
      page: latest.current.pageId,
      selection: latest.current.selectedId ? [latest.current.selectedId] : [],
    });
    const heartbeat = window.setInterval(() => send({}), 5000);
    const leave = () => {
      try {
        void fetch("/api/collab/presence", {
          method: "POST",
          keepalive: true,
          headers: { "content-type": "application/json" },
          body: JSON.stringify({ leave: true }),
        });
      } catch {
        // best effort
      }
    };
    send(state());
    window.addEventListener("pagehide", leave);
    return () => {
      window.clearInterval(heartbeat);
      window.removeEventListener("pagehide", leave);
      leave();
    };
  }, [enabled, mayPublish]);

  useEffect(() => {
    if (!enabled || !mayPublish) return;
    const wait = Math.max(0, 100 - (Date.now() - lastSent.current));
    window.clearTimeout(timer.current);
    timer.current = window.setTimeout(() => {
      lastSent.current = Date.now();
      void collabPost("/api/collab/presence", {
        page: pageId,
        selection: selectedId ? [selectedId] : [],
      }).catch(() => {});
    }, wait);
    return () => window.clearTimeout(timer.current);
  }, [enabled, mayPublish, pageId, selectedId]);

  return entries;
}

function CollaboratorsSection({ entries, self }: { entries: PresenceEntry[]; self?: Actor }) {
  return (
    <Section id="collab-people" title={`Collaborators (${entries.length})`}>
      {entries.length === 0 && <Empty>Nobody else is connected.</Empty>}
      <ul className="space-y-1 text-xs">
        {entries.map((entry) => (
          <li key={entry.actor.id} className="flex items-center gap-2">
            <span className={entry.age_ms > 10000 ? "opacity-50" : ""}>
              {displayName(entry.actor)}
              {self && entry.actor.id === self.id ? " (you)" : ""}
            </span>
            <Badge variant="outline">{entry.actor.kind}</Badge>
            {entry.actor.directed_by && <span className="text-muted-foreground">for {entry.actor.directed_by}</span>}
            <span className="ml-auto truncate text-muted-foreground">
              {entry.page ? `${entry.page}${entry.unknown_page ? " (missing)" : ""}` : ""}
              {entry.selection.length ? ` · ${entry.selection.length} selected` : ""}
            </span>
          </li>
        ))}
      </ul>
    </Section>
  );
}

type Comment = { id: string; author: Actor; body: string; mentions?: string[] };
type Thread = {
  id: string;
  target?: { page?: string; object?: string };
  status: "open" | "resolved";
  review_state: string;
  version: number;
  dangling?: boolean;
  comments?: Comment[];
  comment_count?: number;
};
type ThreadList = { matches: number; returned: number; has_more: boolean; dangling: number; threads: Thread[] };

const REVIEW_STATES = ["requested", "approved", "changes-requested"] as const;

function CommentsSection({ actor, canComment }: { actor: Actor; canComment: boolean }) {
  const pageId = useStudio((s) => s.pageId);
  const selectedId = useStudio((s) => s.selectedId);
  const [list, setList] = useState<ThreadList | null>(null);
  const [open, setOpen] = useState<string | null>(null);
  const [filter, setFilter] = useState<"open" | "resolved" | "all">("open");
  const [draft, setDraft] = useState("");
  const [error, setError] = useState<string | null>(null);

  const refresh = useCallback(async () => {
    try {
      const q = new URLSearchParams({ comments: "true", limit: "100" });
      if (filter !== "all") q.set("status", filter);
      setList(await collabGet<ThreadList>(`/api/comments?${q}`));
      setError(null);
    } catch (e) {
      setError(errorText(e));
    }
  }, [filter]);
  useEffect(() => {
    void refresh();
    const id = window.setInterval(() => void refresh(), 10000);
    return () => window.clearInterval(id);
  }, [refresh]);

  const send = async (body: Record<string, unknown>) => {
    try {
      await collabPost("/api/comments", { actor, ...body });
      setError(null);
      await refresh();
      return true;
    } catch (e) {
      setError(errorText(e));
      return false;
    }
  };
  const add = async () => {
    const target: Record<string, string> = {};
    if (pageId) target.page = pageId;
    if (selectedId) target.object = selectedId;
    if (await send({ action: "add", target, body: draft })) setDraft("");
  };

  return (
    <Section
      id="collab-comments"
      title={`Comments${list ? ` (${list.matches})` : ""}`}
      actions={
        <Button variant="ghost" size="sm" onClick={() => void refresh()}>
          Refresh
        </Button>
      }
    >
      <div className="mb-2 flex gap-1">
        {(["open", "resolved", "all"] as const).map((f) => (
          <Button key={f} size="sm" variant={filter === f ? "default" : "outline"} onClick={() => setFilter(f)}>
            {f}
          </Button>
        ))}
      </div>
      {error && <p className="mb-1 text-xs text-red-600">{error}</p>}
      {list && list.dangling > 0 && (
        <p className="mb-1 text-xs text-amber-600">{list.dangling} thread(s) point at objects that no longer exist.</p>
      )}
      {list && list.threads.length === 0 && <Empty>No {filter === "all" ? "" : filter} threads.</Empty>}
      <ul className="space-y-2 text-xs">
        {list?.threads.map((thread) => (
          <ThreadRow
            key={thread.id}
            thread={thread}
            expanded={open === thread.id}
            onToggle={() => setOpen(open === thread.id ? null : thread.id)}
            canComment={canComment}
            send={send}
          />
        ))}
      </ul>
      {canComment ? (
        <div className="mt-2 space-y-1">
          <textarea
            className="w-full rounded border bg-background px-2 py-1 text-xs"
            rows={2}
            maxLength={16000}
            placeholder={selectedId ? `Comment on ${selectedId}` : "Comment on the document"}
            value={draft}
            onChange={(e) => setDraft(e.target.value)}
          />
          <Button size="sm" disabled={!draft.trim()} onClick={() => void add()}>
            New thread
          </Button>
        </div>
      ) : (
        <p className="mt-2 text-xs text-muted-foreground">Your role cannot comment.</p>
      )}
    </Section>
  );
}

function ThreadRow({
  thread,
  expanded,
  onToggle,
  canComment,
  send,
}: {
  thread: Thread;
  expanded: boolean;
  onToggle: () => void;
  canComment: boolean;
  send: (body: Record<string, unknown>) => Promise<boolean>;
}) {
  const [reply, setReply] = useState("");
  const select = useStudio((s) => s.select);
  const first = thread.comments?.[0];
  const where = [thread.target?.page, thread.target?.object].filter(Boolean).join(" / ") || "document";
  return (
    <li className="rounded border p-2">
      <button className="flex w-full items-center gap-2 text-left" onClick={onToggle}>
        <Badge variant={thread.status === "open" ? "default" : "outline"}>{thread.status}</Badge>
        {thread.review_state !== "none" && <Badge variant="outline">{thread.review_state}</Badge>}
        <span className="truncate">{where}</span>
        {thread.dangling && <span className="text-amber-600">missing</span>}
        <span className="ml-auto text-muted-foreground">{thread.id}</span>
      </button>
      {!expanded && first && <p className="mt-1 truncate text-muted-foreground">{first.body}</p>}
      {expanded && (
        <div className="mt-2 space-y-1">
          {thread.target?.object && !thread.dangling && (
            <Button size="sm" variant="ghost" onClick={() => select(thread.target!.object!)}>
              Select object
            </Button>
          )}
          {thread.comments?.map((comment) => (
            <div key={comment.id} className="rounded bg-muted/40 p-1">
              <span className="font-medium">{displayName(comment.author)}</span>{" "}
              <span className="text-muted-foreground">({comment.author.kind})</span>
              <p className="whitespace-pre-wrap">{comment.body}</p>
            </div>
          ))}
          {canComment && (
            <>
              <textarea
                className="w-full rounded border bg-background px-2 py-1"
                rows={2}
                placeholder="Reply"
                value={reply}
                onChange={(e) => setReply(e.target.value)}
              />
              <div className="flex flex-wrap gap-1">
                <Button
                  size="sm"
                  disabled={!reply.trim()}
                  onClick={async () => {
                    if (await send({ action: "reply", thread: thread.id, body: reply, if_version: thread.version })) setReply("");
                  }}
                >
                  Reply
                </Button>
                <Button
                  size="sm"
                  variant="outline"
                  onClick={() => void send({ action: thread.status === "open" ? "resolve" : "reopen", thread: thread.id, if_version: thread.version })}
                >
                  {thread.status === "open" ? "Resolve" : "Reopen"}
                </Button>
                {REVIEW_STATES.map((state) => (
                  <Button
                    key={state}
                    size="sm"
                    variant={thread.review_state === state ? "default" : "outline"}
                    onClick={() => void send({ action: "review", thread: thread.id, state, if_version: thread.version })}
                  >
                    {state}
                  </Button>
                ))}
              </div>
            </>
          )}
        </div>
      )}
    </li>
  );
}

const show = (value: unknown) => (value === undefined ? "—" : typeof value === "string" ? value : JSON.stringify(value));

function ConflictsSection() {
  const [report, setReport] = useState<ConflictReport | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [open, setOpen] = useState<string | null>(null);
  const refresh = useCallback(async () => {
    try {
      setReport(await collabGet<ConflictReport>("/api/collab/conflicts"));
      setError(null);
    } catch (e) {
      setReport(null);
      setError(errorText(e));
    }
  }, []);
  useEffect(() => {
    void refresh();
  }, [refresh]);
  return (
    <Section
      id="collab-conflicts"
      title={`Conflicts${report ? ` (${report.conflicts_total})` : ""}`}
      actions={
        <Button variant="ghost" size="sm" onClick={() => void refresh()}>
          Refresh
        </Button>
      }
    >
      {error && <p className="text-xs text-muted-foreground">No conflict report: {error}</p>}
      {report && report.conflicts.length === 0 && <Empty>No conflicts. Concurrent edits merged cleanly.</Empty>}
      <ul className="space-y-1 text-xs">
        {report?.conflicts.map((c) => (
          <li key={c.id} className="rounded border p-2">
            <button className="flex w-full items-center gap-2 text-left" onClick={() => setOpen(open === c.id ? null : c.id)}>
              <Badge variant="outline">{c.code}</Badge>
              <span className="truncate">
                {c.object}
                {c.property ? `.${c.property}` : ""}
              </span>
              <span className="ml-auto text-muted-foreground">{c.status}</span>
            </button>
            {open === c.id && (
              <div className="mt-2 grid grid-cols-2 gap-2">
                <div>
                  <p className="font-medium">Kept</p>
                  <p className="text-muted-foreground">{c.winning_actor ?? "—"}</p>
                  <pre className="whitespace-pre-wrap break-all">{show(c.winning_value)}</pre>
                </div>
                <div>
                  <p className="font-medium">Overridden</p>
                  <p className="text-muted-foreground">{c.losing_actor}</p>
                  <pre className="whitespace-pre-wrap break-all">{show(c.losing_value)}</pre>
                </div>
                <p className="col-span-2 text-muted-foreground">
                  Page {c.page}. The kept side is deterministic on every peer. To restore the overridden value, make a new
                  edit; it is recorded as a normal operation.
                </p>
              </div>
            )}
          </li>
        ))}
      </ul>
      {report && report.conflicts_total > report.conflicts.length && (
        <p className="mt-1 text-xs text-muted-foreground">
          Showing {report.conflicts.length} of {report.conflicts_total}.
        </p>
      )}
      {report && report.rejected.length > 0 && (
        <p className="mt-1 text-xs text-amber-600">{report.rejected.length} operation(s) were rejected on replay.</p>
      )}
    </Section>
  );
}

const LOCAL_ACTOR: Actor = { kind: "human", id: "local", name: "Local editor" };

export function CollabPanel() {
  const { status, session, probe } = useCollab();
  useEffect(() => {
    void probe();
  }, [probe]);
  const on = status === "on";
  const entries = usePresence(on, on && !!session?.may.comment);
  const actor = session?.actor ?? LOCAL_ACTOR;
  return (
    <div className="space-y-1 p-2">
      <ConnectionSection />
      {on && <CollaboratorsSection entries={entries} self={session?.actor} />}
      <CommentsSection actor={actor} canComment={on ? !!session?.may.comment : status !== "denied"} />
      {on && <ConflictsSection />}
    </div>
  );
}
