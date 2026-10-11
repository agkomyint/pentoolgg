import { cva, type VariantProps } from "class-variance-authority";
import { cn } from "@/lib/utils";

const badgeVariants = cva("inline-flex items-center rounded-full border px-2 py-0.5 text-[11px] font-medium", {
  variants: {
    variant: {
      default: "bg-muted text-muted-foreground",
      success: "border-success/40 text-success",
      warning: "border-warning/50 text-warning",
      destructive: "border-destructive/50 text-destructive",
      outline: "",
    },
  },
  defaultVariants: { variant: "default" },
});

export function Badge({
  className,
  variant,
  ...props
}: React.HTMLAttributes<HTMLSpanElement> & VariantProps<typeof badgeVariants>) {
  return <span className={cn(badgeVariants({ variant }), className)} {...props} />;
}
