export const ARTWORK_SOURCE_DRAG_TYPE = "application/x-pentool-art-source";

export type ArtworkSourceDrag =
  | { kind: "page"; page: string }
  | { kind: "node"; page: string; node: string };

export function parseArtworkSourceDrag(value: string): ArtworkSourceDrag | null {
  try {
    const source = JSON.parse(value) as Record<string, unknown>;
    if (typeof source.page !== "string" || !source.page) return null;
    if (source.kind === "page") return { kind: "page", page: source.page };
    if (source.kind === "node" && typeof source.node === "string" && source.node) {
      return { kind: "node", page: source.page, node: source.node };
    }
  } catch {
    // Invalid drag payloads are ignored; the regular source picker remains available.
  }
  return null;
}
