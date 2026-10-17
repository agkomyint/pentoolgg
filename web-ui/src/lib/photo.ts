// Photo studio client. Every preview is developed by the Rust pipeline and every
// edit is one guarded Rust transaction (/api/photo/*); nothing is computed here.
import { getJson, postBytes, postJson } from "@/lib/api";

/* eslint-disable @typescript-eslint/no-explicit-any */
export type Develop = Record<string, any>;
export type PhotoSummary = {
  id: string;
  width: number;
  height: number;
  rating: number;
  pick: "none" | "pick" | "reject" | string;
  label: string;
};
export type Variant = { id: string; name?: string; develop?: Develop };
export type PhotoDetail = {
  revision: string;
  kind?: "raw" | "derived" | "rendered";
  width?: number;
  height?: number;
  photo: { variants: Variant[]; snapshots?: { id: string; name?: string }[]; keywords?: string[] };
};
export type PreviewReport = {
  width: number;
  height: number;
  developed_width: number;
  developed_height: number;
  highlight_clipped_pixels: number;
  shadow_clipped_pixels: number;
  delivery: {
    space: string;
    pixels: number;
    out_of_gamut_pixels: number;
    histogram: { r: number[]; g: number[]; b: number[]; bins: number };
  };
};
export type Shot = { url: string; report: PreviewReport };

export const SLIDERS: [string, [string, string, number, number, number][]][] = [
  ["Tone", [["tone.exposure", "Exposure", -5, 5, 0.05], ["tone.contrast", "Contrast", -100, 100, 1], ["tone.highlights", "Highlights", -100, 100, 1], ["tone.shadows", "Shadows", -100, 100, 1], ["tone.whites", "Whites", -100, 100, 1], ["tone.blacks", "Blacks", -100, 100, 1]]],
  ["Presence", [["presence.texture", "Texture", -100, 100, 1], ["presence.clarity", "Clarity", -100, 100, 1], ["presence.dehaze", "Dehaze", -100, 100, 1], ["presence.vibrance", "Vibrance", -100, 100, 1], ["presence.saturation", "Saturation", -100, 100, 1]]],
  ["Detail", [["detail.sharpening.amount", "Sharpening", 0, 150, 1], ["detail.noise.luminance", "Luminance NR", 0, 100, 1], ["detail.noise.color", "Color NR", 0, 100, 1]]],
  ["Geometry", [["geometry.rotate", "Rotate", -45, 45, 0.1], ["geometry.vertical", "Vertical", -100, 100, 1], ["geometry.horizontal", "Horizontal", -100, 100, 1], ["lens.distortion", "Distortion", -100, 100, 1]]],
  ["Effects", [["effects.vignette.amount", "Vignette", -100, 100, 1], ["effects.grain.amount", "Grain", 0, 100, 1]]],
];
export const LABELS = ["none", "red", "yellow", "green", "blue", "purple"];
export const GROUPS = ["white_balance", "tone", "presence", "curves", "hsl", "grading", "monochrome", "detail", "lens", "geometry", "crop", "effects", "calibration", "local"];

export function getPath(object: unknown, path: string): any {
  return path.split(".").reduce<any>((value, key) => value?.[key], object);
}

export const photoApi = {
  catalog: (query: string) =>
    getJson<{ photos: PhotoSummary[]; revision: string; matches: number; has_more: boolean }>(
      `/api/photo/catalog?query=${encodeURIComponent(query)}&limit=500`,
    ),
  detail: (photo: string) => getJson<PhotoDetail>(`/api/photo/detail?photo=${encodeURIComponent(photo)}`),
  import: (file: File, id: string, revision: string) => {
    const query = new URLSearchParams({ id, name: file.name });
    return postBytes<{ revision: string; result: { photo: string; asset: string; upgraded: boolean } }>(
      `/api/photo/import?${query}`,
      file,
      { "content-type": "application/octet-stream", "x-pentool-revision": revision },
    );
  },
  place: (body: { photo: string; variant: string; page: string; layer: string; x: number; y: number; width: number; height: number; fit: string; position: [number, number]; revision: string }) =>
    postJson<{ revision: string; result: { id: string; photo: string; page: string; layer: string } }>("/api/photo/place", body),
  async preview(args: { photo: string; variant: string; edge: number; space: string; overlay: string; uncropped: boolean }): Promise<Shot> {
    const data = await postJson<{ png: string; report: PreviewReport }>("/api/photo/preview", args);
    return { url: `data:image/png;base64,${data.png}`, report: data.report };
  },
  // Every edit carries the revision last read, so a concurrent CLI or agent
  // change is refused instead of overwritten.
  edit: (edit: Record<string, unknown>, revision: string | null) =>
    postJson<{ revision: string }>("/api/photo/edit", { edit, revision }),
  undo: () => postJson<unknown>("/api/undo", {}),
};
