import { ImagePlus } from "lucide-react";
import { Button } from "@/components/ui/button";
import { useStudio } from "@/store";
import { placementBlocker, usePhoto } from "./photoStore";

/** Puts the selected photos, with their edits, on the canvas. */
export function AddToCanvas({ compact = false }: { compact?: boolean }) {
  const s = usePhoto();
  const studio = useStudio();
  // Read after subscribing so the reason follows page, layer and selection changes.
  const count = s.selected.length || (s.current ? 1 : 0);
  const blocker = placementBlocker() ?? (count ? null : "Select a photo first.");
  const label = count > 1 ? `Add ${count} to canvas` : "Add to canvas";
  return (
    <Button
      className={compact ? "h-7 gap-1 px-2 text-xs" : "min-h-10 gap-1.5"}
      disabled={s.busy || studio.busy || !!blocker}
      aria-busy={s.busy}
      title={blocker ?? "Place on the active layer and open Design. Later Photo edits update it."}
      onClick={() => void s.placeOnCanvas()}
    >
      <ImagePlus className="size-4" aria-hidden />
      {label}
    </Button>
  );
}
