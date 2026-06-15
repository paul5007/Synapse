import { type HTMLAttributes, type ReactNode } from "react";
import { cn } from "@/lib/utils";
import type { FleetStatus } from "@/lib/dashboard-state";

const statusClasses: Record<FleetStatus, string> = {
  working: "border-info-border bg-info-bg text-info-fg",
  idle: "border-border bg-surface-1 text-muted",
  ready_for_review: "border-success-border bg-success-bg text-success-fg",
  needs_input: "border-warning-border bg-warning-bg text-warning-fg",
  awaiting_approval: "border-warning-border bg-warning-bg text-warning-fg",
  stuck: "border-danger-border bg-danger-bg text-danger-fg",
  failed: "border-danger-border bg-danger-bg text-danger-fg",
  done: "border-success-border bg-success-bg text-success-fg"
};

const statusShape: Record<FleetStatus, string> = {
  working: "●",
  idle: "○",
  ready_for_review: "◧",
  needs_input: "◆",
  awaiting_approval: "⚑",
  stuck: "▲",
  failed: "×",
  done: "✓"
};

const statusLabel: Record<FleetStatus, string> = {
  working: "Working",
  idle: "Idle",
  ready_for_review: "Review",
  needs_input: "Needs input",
  awaiting_approval: "Approval",
  stuck: "Stuck",
  failed: "Failed",
  done: "Done"
};

export function Badge({ className, ...props }: HTMLAttributes<HTMLSpanElement>) {
  return (
    <span
      className={cn(
        "inline-flex min-h-6 items-center rounded-pill border px-2 text-xs font-medium",
        className
      )}
      {...props}
    />
  );
}

export function StatusBadge({ status, className }: { status: FleetStatus; className?: string }) {
  return (
    <Badge
      className={cn(statusClasses[status], className)}
      aria-label={`Status: ${statusLabel[status]}`}
    >
      <span aria-hidden="true" className="mr-1 font-mono">
        {statusShape[status]}
      </span>
      {statusLabel[status]}
    </Badge>
  );
}

export function FlagBadge({ children, tone = "info" }: { children: ReactNode; tone?: "info" | "warn" | "danger" }) {
  const toneClass =
    tone === "danger"
      ? "border-danger-border bg-danger-bg text-danger-fg"
      : tone === "warn"
        ? "border-warning-border bg-warning-bg text-warning-fg"
        : "border-info-border bg-info-bg text-info-fg";
  return <Badge className={toneClass}>{children}</Badge>;
}
