import { api, postJson, type Doc, type SceneNode } from "@/lib/api";
import { findNode, pageOf } from "@/lib/model";
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

/** The one image node currently selected on the page, if any. */
export function selectedImage(): SceneNode | null {
  const { doc, pageId, selectedIds } = useStudio.getState();
  if (!doc || selectedIds.length !== 1) return null;
  const node = findNode(pageOf(doc, pageId), selectedIds[0]!)?.node;
  return node && node.kind === "image" ? node : null;
}

/**
 * Turn the selected photo into pixels the selection and paint tools can edit, in place
 * (one undoable step), so a person can select and move parts of an image as in a photo editor.
 */
export async function bakeSelectedImage(): Promise<string | null> {
  const image = selectedImage();
  const { doc, pageId, replaceDoc, notify } = useStudio.getState();
  if (!image || !doc) return null;
  try {
    let source: Doc = doc;
    if (source.version < 6) source = (await api.migrate(source, 6)).document;
    const id = `${image.id}-pixels`;
    const body = await postJson<{ document: Doc }>("/api/raster", { document: source, page: pageOf(source, pageId)?.id, id: image.id, action: "rasterize", args: { new_id: id, replace: true } });
    replaceDoc(body.document, "Edit photo pixels");
    useStudio.setState({ selectedIds: [id], selectedId: id });
    usePaint.getState().set({ layerId: id, selection: null, mask: null, status: "The photo is now editable pixels: select, move or paint on it. Undo restores the image." });
    return id;
  } catch (e) {
    const message = e instanceof Error ? e.message : "Could not turn the photo into pixels.";
    usePaint.getState().set({ status: message });
    notify(message, true);
    return null;
  }
}

/** Switch the main canvas to a paint tool and show its options. */
export function startPainting(tool: RasterTool): void {
  usePaint.getState().set({ tool });
  if (selectedImage()) void bakeSelectedImage();
  const studio = useStudio.getState();
  studio.setPaintMode(true);
  if (studio.workspaceId !== "paint") studio.setWorkspace("paint");
}
