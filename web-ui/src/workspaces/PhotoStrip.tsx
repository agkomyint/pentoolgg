import { useEffect } from "react";
import { Camera } from "lucide-react";
import { Button } from "@/components/ui/button";
import { cn } from "@/lib/utils";
import { useStudio } from "@/store";
import { PHOTO_DRAG_TYPE, draggedPhotos, usePhoto } from "./photoStore";
import { Thumb } from "./PhotoStage";

/**
 * The project's photos as small thumbnails beside the layers. Drag one (or the
 * selection) onto the canvas or onto a page in the page strip to place it with
 * its edits; double-click places it in the middle of the current page.
 */
export function PhotoStrip() {
  const { shared, setWorkspace } = useStudio();
  const s = usePhoto();
  useEffect(() => {
    if (shared && !s.photos.length && !s.busy) void s.run(() => s.loadCatalog());
    // Load once per shared document; the Photo workspace keeps it current after that.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [shared]);

  if (!shared) return <p className="p-1 text-xs text-muted-foreground">Serve a .pen project file to use its photos here.</p>;
  if (!s.photos.length) return (
    <div className="space-y-2 p-1 text-xs text-muted-foreground">
      <p>{s.busy ? "Loading photos…" : "No photos in this project yet."}</p>
      <Button variant="outline" size="sm" className="gap-1.5" onClick={() => setWorkspace("photo")}><Camera className="size-3.5" aria-hidden />Import in Photo</Button>
    </div>
  );
  return (
    <div className="space-y-2">
      <p className="text-[11px] text-muted-foreground">Drag onto the canvas or a page. Ctrl-click to pick several.</p>
      <ul className="grid grid-cols-[repeat(auto-fill,minmax(72px,1fr))] gap-1.5" aria-label="Project photos">
        {s.photos.map((photo) => (
          <li key={photo.id}>
            <button
              type="button"
              draggable
              aria-pressed={s.selected.includes(photo.id)}
              title={`${photo.id} · ${photo.width}×${photo.height}. Drag onto the canvas or a page; double-click to place it here.`}
              className={cn("block w-full cursor-grab rounded border p-0.5 active:cursor-grabbing", s.selected.includes(photo.id) ? "border-primary ring-1 ring-primary" : "border-transparent hover:border-border")}
              onClick={(e) => s.select(photo.id, e.shiftKey ? "range" : e.ctrlKey || e.metaKey ? "toggle" : undefined)}
              onDoubleClick={() => void s.place([photo.id])}
              onDragStart={(e) => {
                const ids = draggedPhotos(photo.id);
                e.dataTransfer.effectAllowed = "copy";
                e.dataTransfer.setData(PHOTO_DRAG_TYPE, JSON.stringify(ids));
                e.dataTransfer.setData("text/plain", ids.join(", "));
                const img = e.currentTarget.querySelector("img");
                if (img) e.dataTransfer.setDragImage(img, img.width / 2, img.height / 2);
              }}
            >
              <Thumb id={photo.id} className="pointer-events-none" />
              <span className="mt-0.5 block truncate text-[10px] text-muted-foreground">{photo.id}</span>
            </button>
          </li>
        ))}
      </ul>
      {s.status && <p role="status" className="break-words text-[11px] text-muted-foreground">{s.status}</p>}
    </div>
  );
}
