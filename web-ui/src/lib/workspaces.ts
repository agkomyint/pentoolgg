// One entry per product area. `since` ties each workspace to the roadmap so the
// shell shows what exists now and what each future milestone adds.
import type { LucideIcon } from "lucide-react";
import { Brush, Camera, Image, Layers, Newspaper, PenTool, Printer } from "lucide-react";

export type Status = "available" | "partial" | "planned";
export type Workspace = {
  id: string;
  label: string;
  icon: LucideIcon;
  status: Status;
  since: string;
  summary: string;
  /** Capabilities the roadmap promises, listed while the workspace is not built. */
  planned: string[];
};

export const workspaces: Workspace[] = [
  { id: "design", label: "Design", icon: PenTool, status: "available", since: "0.6", summary: "Vector scene editing, text, components and export.", planned: [] },
  { id: "image", label: "Images", icon: Image, status: "partial", since: "0.7", summary: "Raster images and non-destructive adjustments.", planned: ["Image placement and fit", "Crop and mask", "Operations stack"] },
  { id: "composite", label: "Composite", icon: Layers, status: "partial", since: "0.8", summary: "Blend modes, effects and compositing.", planned: ["Blend mode picker", "Effects stack", "Dependency report"] },
  { id: "photo", label: "Photo", icon: Camera, status: "partial", since: "0.11", summary: "Catalog, RAW development and photo edits.", planned: ["Catalog browser", "Develop panel", "Batch export"] },
  { id: "print", label: "Print", icon: Printer, status: "partial", since: "0.12", summary: "Color roles, preflight, plates and soft proof.", planned: [] },
  { id: "paint", label: "Paint", icon: Brush, status: "planned", since: "0.13", summary: "Agent-directed realistic painting.", planned: ["Stroke plan viewer", "Palette and color mixing", "Region comparison", "Revision scoring"] },
  { id: "layout", label: "Layout", icon: Newspaper, status: "planned", since: "0.16", summary: "Page layout, styles, books and IDML.", planned: ["Spreads and parents", "Threaded text frames", "Paragraph and character styles", "Books and IDML interchange"] },
];
