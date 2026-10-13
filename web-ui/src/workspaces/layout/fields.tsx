import { useEffect, useState } from "react";
import { cn } from "@/lib/utils";

/** Text input that commits on Enter or blur and re-syncs when the document changes. */
export function TextField({
  value,
  onCommit,
  placeholder,
  ariaLabel,
  className,
}: {
  value: string;
  onCommit: (text: string) => void;
  placeholder?: string;
  ariaLabel?: string;
  className?: string;
}) {
  const [text, setText] = useState(value);
  useEffect(() => setText(value), [value]);
  const commit = () => (text === value ? undefined : onCommit(text));
  return (
    <input
      aria-label={ariaLabel}
      placeholder={placeholder}
      className={cn("h-6 w-full min-w-0 flex-1 px-1.5 text-[12px]", className)}
      value={text}
      onChange={(e) => setText(e.target.value)}
      onBlur={commit}
      onKeyDown={(e) => e.key === "Enter" && (e.target as HTMLInputElement).blur()}
    />
  );
}
