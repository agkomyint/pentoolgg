// One entry per product area. `since` ties each workspace to the roadmap so the
// shell shows what exists now and what each future milestone adds.
import type { LucideIcon } from "lucide-react";
import { Box, Brush, Camera, Image, Layers, MessagesSquare, Newspaper, PenTool, Palette, Printer, Puzzle, Sparkles, Users } from "lucide-react";

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
  { id: "photo", label: "Photo", icon: Camera, status: "available", since: "0.11", summary: "Catalog, culling, RAW development, masks, crop and copy/sync.", planned: [] },
  { id: "print", label: "Print", icon: Printer, status: "partial", since: "0.12", summary: "Color roles, preflight, plates and soft proof.", planned: [] },
  { id: "paint", label: "Paint", icon: Brush, status: "partial", since: "0.10", summary: "Raster brush, clone, heal, selections and quick mask. Plan passes with protected regions, preview, commit and scoring (0.13).", planned: [] },
  { id: "layout", label: "Layout", icon: Newspaper, status: "partial", since: "0.14", summary: "Composed stories, overset and layout preflight, tagged PDF and EPUB export. Spreads, style panels and threading tools are still CLI-only.", planned: [] },
  { id: "3d", label: "3D", icon: Box, status: "partial", since: "0.15", summary: "Import, frame, light and render static 3D objects.", planned: ["Viewport orbit, pan and zoom", "Material controls", "Lighting controls"] },
  { id: "extensions", label: "Extensions", icon: Puzzle, status: "available", since: "0.18", summary: "Sandboxed WebAssembly extensions that read the document and propose undoable edits.", planned: [] },
  { id: "ai", label: "AI", icon: Sparkles, status: "partial", since: "0.25", summary: "Connect and validate an image-model provider. Generating and editing images follow.", planned: ["Generate panel with candidates", "Accept, replace and discard", "Remove background and variations"] },
  { id: "stats", label: "Stats", icon: Users, status: "available", since: "0.20", summary: "Read-only contribution stats for people and agents, derived from the operation log.", planned: [] },
  { id: "collab", label: "Collab", icon: MessagesSquare, status: "available", since: "0.20", summary: "Collaborators, comments and review states, and named merge conflicts. Roles are enforced by the server.", planned: [] },
  { id: "systems", label: "Systems", icon: Palette, status: "available", since: "0.19", summary: "Design tokens, token libraries, templates and the token audit. Installing and updating libraries stays in the CLI.", planned: [] },
];
