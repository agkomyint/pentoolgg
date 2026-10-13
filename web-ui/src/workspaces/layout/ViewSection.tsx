import { Check, Kbd, Section } from "@/components/ui/kit";
import { useLayout } from "./layoutStore";

/** Overlay toggles and the optional InDesign-style key set. */
export function ViewSection() {
  const { overlays, setOverlay, indesignKeys, setIndesignKeys } = useLayout();
  return (
    <Section id="layout-view" title="View" defaultOpen={false}>
      <Check checked={overlays.frames} onChange={(v) => setOverlay("frames", v)}>
        Frame edges
      </Check>
      <Check checked={overlays.threads} onChange={(v) => setOverlay("threads", v)}>
        Thread links
      </Check>
      <Check checked={overlays.overset} onChange={(v) => setOverlay("overset", v)}>
        Overset markers
      </Check>
      <Check checked={overlays.margins} onChange={(v) => setOverlay("margins", v)}>
        Margins and columns
      </Check>
      <Check checked={overlays.baseline} onChange={(v) => setOverlay("baseline", v)}>
        Baseline grid and guides
      </Check>
      <Check checked={overlays.hidden} onChange={(v) => setOverlay("hidden", v)}>
        Hidden characters (story editor)
      </Check>
      <Check checked={indesignKeys} onChange={setIndesignKeys}>
        InDesign-style keys
      </Check>
      {indesignKeys && (
        <p className="text-[11px] text-muted-foreground">
          <Kbd>V</Kbd> select, <Kbd>T</Kbd> text, <Kbd>Page Up</Kbd>/<Kbd>Page Down</Kbd> previous/next page,{" "}
          <Kbd>Ctrl</Kbd>+<Kbd>Shift</Kbd>+<Kbd>P</Kbd> new page.
        </p>
      )}
    </Section>
  );
}
