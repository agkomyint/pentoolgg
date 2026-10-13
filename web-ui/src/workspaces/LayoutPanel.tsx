import { useEffect } from "react";
import { useStudio, LATEST_FORMAT } from "@/store";
import { Button } from "@/components/ui/button";
import { Empty } from "@/components/ui/kit";
import { CheckSection } from "./layout/CheckSection";
import { FrameSection } from "./layout/FrameSection";
import { PagesSection } from "./layout/PagesSection";
import { StoriesSection } from "./layout/StoriesSection";
import { StylesSection } from "./layout/StylesSection";
import { ViewSection } from "./layout/ViewSection";
import { useLayout } from "./layout/layoutStore";

/** Layout workspace dock: pages, stories, frame options, styles, checks. */
export function LayoutPanel() {
  const { doc, upgradeDoc, busy } = useStudio();
  const refresh = useLayout((s) => s.refresh);

  // Recompose (in Rust) shortly after the document changes.
  useEffect(() => {
    const timer = window.setTimeout(() => void refresh(), 350);
    return () => window.clearTimeout(timer);
  }, [doc, refresh]);

  if (!doc) return null;
  if (doc.version < 9) {
    return (
      <div className="space-y-2 p-2">
        <Empty>
          This document is format v{doc.version}. Page layout needs format v9 (spreads, parents, stories, text
          frames).
        </Empty>
        <Button disabled={busy || LATEST_FORMAT < 9} onClick={() => void upgradeDoc(9)}>
          Upgrade to format v9
        </Button>
      </div>
    );
  }
  return (
    <>
      <PagesSection />
      <StoriesSection />
      <FrameSection />
      <StylesSection />
      <ViewSection />
      <CheckSection />
    </>
  );
}
