// Shared building blocks for the Studio dock. Every panel is made of Section
// (collapsible group), Row (label + control) and the form controls below, so
// panels look and behave the same in every workspace.
import { useEffect, useState } from "react";
import { ChevronDown, ChevronRight } from "lucide-react";
import { cn } from "@/lib/utils";

function remembered(key: string, fallback: boolean): boolean {
  try {
    const stored = localStorage.getItem(`pentool-panel:${key}`);
    return stored === null ? fallback : stored === "1";
  } catch {
    return fallback;
  }
}

/** Collapsible dock panel; its open state is remembered per id. */
export function Section({
  id,
  title,
  actions,
  defaultOpen = true,
  children,
  className,
}: {
  id: string;
  title: string;
  actions?: React.ReactNode;
  defaultOpen?: boolean;
  children: React.ReactNode;
  className?: string;
}) {
  const [open, setOpen] = useState(() => remembered(id, defaultOpen));
  useEffect(() => {
    try {
      localStorage.setItem(`pentool-panel:${id}`, open ? "1" : "0");
    } catch {
      // storage can be unavailable
    }
  }, [id, open]);
  return (
    <section className="border-b">
      <header className="flex h-7 items-center gap-1 px-2">
        <button
          className="flex min-w-0 flex-1 items-center gap-1 text-left text-[11px] font-semibold uppercase tracking-wide text-muted-foreground hover:text-foreground"
          aria-expanded={open}
          onClick={() => setOpen(!open)}
        >
          {open ? <ChevronDown size={12} /> : <ChevronRight size={12} />}
          <span className="truncate">{title}</span>
        </button>
        {open && actions}
      </header>
      {open && <div className={cn("space-y-2 px-2 pb-2.5", className)}>{children}</div>}
    </section>
  );
}

export function Row({ label, children, className }: { label: string; children: React.ReactNode; className?: string }) {
  return (
    <label className={cn("flex items-center gap-2 text-[12px]", className)}>
      <span className="w-20 shrink-0 text-muted-foreground">{label}</span>
      <span className="flex min-w-0 flex-1 items-center gap-1">{children}</span>
    </label>
  );
}

export function NumberField({
  value,
  onCommit,
  min,
  max,
  step = 1,
  className,
  ariaLabel,
  disabled = false,
}: {
  value: number;
  onCommit: (n: number) => void;
  min?: number;
  max?: number;
  step?: number;
  className?: string;
  ariaLabel?: string;
  disabled?: boolean;
}) {
  const shown = String(Math.round(value * 100) / 100);
  const [text, setText] = useState(shown);
  useEffect(() => setText(shown), [shown]);
  const commit = () => {
    let n = Number(text);
    if (!Number.isFinite(n)) return setText(shown);
    if (min !== undefined) n = Math.max(min, n);
    if (max !== undefined) n = Math.min(max, n);
    if (n !== value) onCommit(n);
    else setText(shown);
  };
  return (
    <input
      aria-label={ariaLabel}
      disabled={disabled}
      autoComplete="off"
      className={cn("h-6 w-full min-w-0 flex-1 px-1.5 text-[12px] tabular-nums", className)}
      type="text"
      inputMode="decimal"
      pattern="-?[0-9]*\.?[0-9]*"
      data-step={step}
      spellCheck={false}
      value={text}
      onChange={(e) => setText(e.target.value)}
      onBlur={commit}
      onKeyDown={(e) => e.key === "Enter" && (e.target as HTMLInputElement).blur()}
    />
  );
}

export function Segmented<T extends string>({
  value,
  options,
  onChange,
  ariaLabel,
}: {
  value: T;
  options: { value: T; label: React.ReactNode; title?: string }[];
  onChange: (v: T) => void;
  ariaLabel: string;
}) {
  return (
    <div role="radiogroup" aria-label={ariaLabel} className="inline-flex overflow-hidden rounded border">
      {options.map((o) => (
        <button
          key={o.value}
          role="radio"
          aria-checked={o.value === value}
          title={o.title}
          onClick={() => onChange(o.value)}
          className={cn(
            "h-6 border-r px-2 text-[11px] last:border-r-0",
            o.value === value ? "bg-primary text-primary-foreground" : "bg-muted text-muted-foreground hover:text-foreground",
          )}
        >
          {o.label}
        </button>
      ))}
    </div>
  );
}

export function Check({
  checked,
  disabled = false,
  onChange,
  children,
}: {
  checked: boolean;
  disabled?: boolean;
  onChange: (v: boolean) => void;
  children: React.ReactNode;
}) {
  return (
    <label className="flex min-h-10 cursor-pointer items-center gap-2 text-xs has-[:disabled]:cursor-not-allowed has-[:disabled]:opacity-60">
      <input type="checkbox" className="h-4 w-4 accent-[var(--primary)] focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2" checked={checked} disabled={disabled} onChange={(e) => onChange(e.target.checked)} />
      {children}
    </label>
  );
}

export function Kbd({ children }: { children: React.ReactNode }) {
  return <kbd className="rounded border bg-muted px-1 text-[10px] text-muted-foreground">{children}</kbd>;
}

export function Empty({ children }: { children: React.ReactNode }) {
  return <p className="rounded border border-dashed px-2 py-3 text-center text-[11px] text-muted-foreground">{children}</p>;
}
