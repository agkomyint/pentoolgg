// Read-only helpers over the version 9 `layout` object. Edits never happen
// here: they go through `useLayout().run` (Rust planners) or `useStudio().edit`
// for plain text fields that the server validates on the next compose.
import type { Doc, Page, SceneNode } from "@/lib/api";
import { children, layerNodes } from "@/lib/model";

export type Run = { text?: string; [key: string]: unknown };
export type Paragraph = { style?: string; overrides?: Record<string, unknown>; runs: Run[] };
export type Story = { id: string; language?: string; paragraphs: Paragraph[] };
export type Spread = { id: string; pages: string[]; facing?: boolean };
export type Parent = { id: string; based_on?: string | null; pages?: string[] };
export type StyleMap = Record<string, { based_on?: string | null; group?: string; [key: string]: unknown }>;

type LayoutObject = {
  spreads?: Spread[];
  parents?: Parent[];
  stories?: Story[];
  styles?: { paragraph?: StyleMap; character?: StyleMap; [kind: string]: StyleMap | undefined };
};

export const layoutOf = (doc: Doc | null): LayoutObject => (doc?.layout ?? {}) as LayoutObject;
export const isLayoutDoc = (doc: Doc | null): boolean => !!doc && doc.version >= 9;

export function frameNodes(page: Page | undefined): SceneNode[] {
  const out: SceneNode[] = [];
  const visit = (nodes: SceneNode[]) => {
    for (const node of nodes) {
      if (node.kind === "text-frame") out.push(node);
      visit(children(node));
    }
  };
  for (const layer of page?.layers ?? []) visit(layerNodes(layer));
  return out;
}

/** True when a paragraph can be edited as one text box without losing runs. */
export const isPlainParagraph = (p: Paragraph): boolean =>
  p.runs.length === 1 && typeof p.runs[0]?.text === "string";

export const paragraphText = (p: Paragraph): string =>
  p.runs.map((r) => (typeof r.text === "string" ? r.text : "")).join("");

export const hasOverrides = (p: Paragraph): boolean => !!p.overrides && Object.keys(p.overrides).length > 0;

export const pageParent = (page: Page | undefined): string | undefined =>
  ((page as { layout?: { parent?: string } } | undefined)?.layout ?? {}).parent;

export const nextId = (prefix: string, taken: Set<string>): string => {
  for (let i = 1; ; i += 1) {
    const id = `${prefix}-${i}`;
    if (!taken.has(id)) return id;
  }
};
