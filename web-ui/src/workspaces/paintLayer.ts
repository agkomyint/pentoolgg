import { api, postJson, type Doc, type SceneNode } from "@/lib/api";
import { pageOf } from "@/lib/model";
import { useStudio } from "@/store";
import { usePaint, type RasterTool } from "./rasterStore";

export type RasterNode = SceneNode & { x?: number; y?: number; width: number; height: number };

export function collectRasters(doc: Doc | null, pageId: string | undefined): RasterNode[] {
  const out: RasterNode[] = [];
  if (!doc || doc.version < 6) return out;
  const walk = (nodes: SceneNode[] | undefined) => {
    for (const node of nodes ?? []) {
      if (node.kind === "raster") out.push(node as RasterNode);
      walk(node.children);
    }
  };
  for (const layer of pageOf(doc, pageId)?.layers ?? []) walk(layer.nodes);
  return out;
}

/** Add a page-sized paint layer, upgrading the document to v6 first when needed. */
export async function addPaintLayer(): Promise<string | null> {
  const { doc, pageId, replaceDoc, notify } = useStudio.getState();
  if (!doc) return null;
  try {
    let source: Doc = doc;
    if (source.version < 6) source = (await api.migrate(source, 6)).document;
    const id = `paint-${Date.now().toString(36)}`;
    const body = await postJson<{ document: Doc }>("/api/raster", { document: source, page: pageOf(source, pageId)?.id, id, action: "add", args: {} });
    replaceDoc(body.document);
    usePaint.getState().set({ layerId: id, status: "Paint layer added." });
    notify(doc.version < 6 ? "Upgraded the project to format v6 and added a paint layer. Save to keep it." : "Added a paint layer. Save to keep it.");
    return id;
  } catch (e) {
    usePaint.getState().set({ status: e instanceof Error ? e.message : "Could not add a paint layer." });
    return null;
  }
}

/** Switch the main canvas to a paint tool and show its options. */
export function startPainting(tool: RasterTool): void {
  usePaint.getState().set({ tool });
  const studio = useStudio.getState();
  studio.setPaintMode(true);
  if (studio.workspaceId !== "paint") studio.setWorkspace("paint");
}
