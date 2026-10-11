import { CheckCircle2, Circle } from "lucide-react";
import type { Workspace } from "@/lib/workspaces";
import { Badge } from "@/components/ui/badge";

/** Honest placeholder: lists what the roadmap promises and what is not built. */
export function Planned({ workspace }: { workspace: Workspace }) {
  return (
    <div className="space-y-3">
      <div className="flex items-center gap-2">
        <h3 className="text-sm font-semibold">{workspace.label}</h3>
        <Badge variant={workspace.status === "planned" ? "outline" : "warning"}>
          {workspace.status === "planned" ? `planned for ${workspace.since}` : `partial since ${workspace.since}`}
        </Badge>
      </div>
      <p className="text-muted-foreground">{workspace.summary}</p>
      {workspace.status === "planned" ? (
        <p className="text-xs text-muted-foreground">
          The format and CLI for this area are not implemented yet, so this workspace has no controls. The CLI and the
          agent API remain the supported way to work until it ships.
        </p>
      ) : (
        <p className="text-xs text-muted-foreground">
          This area works through the CLI and the classic editor at <a className="underline" href="/">/</a>. Studio
          controls are still to be built:
        </p>
      )}
      {workspace.planned.length > 0 && (
        <ul className="space-y-1.5">
          {workspace.planned.map((item) => (
            <li key={item} className="flex items-center gap-2 text-xs">
              {workspace.status === "planned" ? <Circle size={12} /> : <CheckCircle2 size={12} className="opacity-30" />}
              {item}
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}
