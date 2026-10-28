import React, { useId } from "react";
import { Check, Loader2, Pencil, Trash2 } from "lucide-react";
import Badge from "../../ui/Badge";
import { Button } from "../../ui/Button";
import { rowActions, type RowStatus } from "./modelRowState";

// One presentational row for every model-like thing (DESIGN.md Rev 5, item 4).
// Pure: no i18n and no store access. Every user-visible string arrives through
// `labels` (or `badges`/`children`) from the caller. The action matrix lives in
// modelRowState.ts; this file only renders it.

/** Visible strings for one row. Callers build these with t(). */
export interface ModelRowLabels {
  select: string;
  active: string;
  download: string;
  cancel: string;
  delete: string;
  retry: string;
  edit: string;
  verifying: string;
  extracting: string;
  switching: string;
  userAdded: string;
  /** Accessible name of the download progress bar. */
  progress: string;
  /** Accessible name of the cancel button while downloading (defaults to `cancel`). */
  cancelDownload?: string;
  /** Visible progress text, given the rounded percentage. */
  downloading?: (percent: number) => string;
  /** Visible speed text, given the speed already formatted to one decimal. */
  speed?: (mbPerSecond: string) => string;
  /** Delete button tooltip, given the model name. */
  deleteModel?: (name: string) => string;
}

export interface ModelRowBadge {
  label: string;
  variant?: "primary" | "secondary" | "success";
  icon?: React.ReactNode;
}

export type ModelRowVariant = "default" | "featured";

export interface ModelRowProps {
  id: string;
  name: string;
  description?: string;
  badges: (string | ModelRowBadge)[];
  status: RowStatus;
  progress?: number;
  speedMbPerSecond?: number;
  errorMessage?: string;
  deletable?: boolean;
  userAdded?: boolean;
  licence?: string;
  variant?: ModelRowVariant;
  testId?: string;
  showChildren?: "active" | "always";
  children?: React.ReactNode;
  labels: ModelRowLabels;
  onSelect?: (id: string) => void;
  onDownload?: (id: string) => void;
  onCancel?: (id: string) => void;
  onDelete?: (id: string) => void;
  onRetry?: (id: string) => void;
  onEdit?: (id: string) => void;
  className?: string;
  disabled?: boolean;
  /**
   * "card" is the model-card look (STT list, onboarding); "engine" is the compact
   * TTS engine look. Default "card".
   */
  frame?: "card" | "engine";
  /** Engine frame: marks the chosen engine (data-selected) and draws its border. */
  selected?: boolean;
  /**
   * The row itself is the primary action: clicking it downloads (when not installed)
   * or selects. No separate select/download buttons are drawn.
   */
  rowSelect?: boolean;
  /** Draw the active/userAdded/switching badges. Default true. */
  statusBadges?: boolean;
  /** Draw the progress bar, busy label, error message and retry. Default true. */
  statusDetails?: boolean;
  /** Extra text under the description (for example an unavailable reason). */
  details?: React.ReactNode;
  /** Right-hand side content inside the header (for example speed/accuracy bars). */
  aside?: React.ReactNode;
  /** Right-hand control in the header (for example an engine select button). */
  action?: React.ReactNode;
  /** Card footer: capability tags, shown before the meta text. */
  tags?: React.ReactNode;
  /** Card footer: right-aligned meta (for example size), wrapped by the row. */
  meta?: React.ReactNode;
  /** Extra data-* attributes for the row root (for example data-engine). */
  dataAttributes?: Record<string, string>;
  /** Root element. Default "div". */
  containerTag?: "div" | "section";
}

const CARD_STYLES = {
  root: "flex flex-col rounded-xl px-4 py-3 gap-2 text-left transition-all duration-200",
  header: "flex justify-between items-center w-full",
  left: "flex flex-col items-start flex-1 min-w-0",
  nameRow: "flex items-center gap-3 flex-wrap",
  title: "text-base font-semibold text-text transition-colors",
  description: "text-text/60 text-sm leading-relaxed",
};

const ENGINE_STYLES = {
  root: "flex flex-col gap-3 rounded-lg border p-3",
  header: "flex items-start justify-between gap-3",
  left: "flex flex-col gap-1",
  nameRow: "flex flex-wrap items-center gap-2",
  title: "text-sm font-medium",
  description: "text-xs text-mid-gray",
};

/** Clamp to 0..100; missing or NaN counts as 0. */
const clampPercent = (value: number | undefined): number => {
  if (value === undefined || Number.isNaN(value)) return 0;
  return Math.min(100, Math.max(0, value));
};

const renderBadge = (badge: string | ModelRowBadge, index: number) => {
  if (typeof badge === "string") {
    return (
      <Badge key={`badge-${index}`} variant="secondary">
        {badge}
      </Badge>
    );
  }
  return (
    <Badge key={`badge-${index}`} variant={badge.variant ?? "secondary"}>
      {badge.icon}
      {badge.label}
    </Badge>
  );
};

const ModelRow: React.FC<ModelRowProps> = ({
  id,
  name,
  description,
  badges,
  status,
  progress,
  speedMbPerSecond,
  errorMessage,
  deletable = true,
  userAdded = false,
  licence,
  variant = "default",
  testId = "model-row",
  showChildren = "active",
  children,
  labels,
  onSelect,
  onDownload,
  onCancel,
  onDelete,
  onRetry,
  onEdit,
  className = "",
  disabled = false,
  frame = "card",
  selected,
  rowSelect = false,
  statusBadges = true,
  statusDetails = true,
  details,
  aside,
  action,
  tags,
  meta,
  dataAttributes,
  containerTag = "div",
}) => {
  const titleId = useId();
  const styles = frame === "engine" ? ENGINE_STYLES : CARD_STYLES;
  const actions = rowActions(status, { deletable });
  const isActive = status === "active";
  // In rowSelect mode the whole row is the button: download when not installed, else select.
  const clickable = rowSelect && (actions.select || actions.download);
  const showBody = showChildren === "always" || isActive;
  const percent = clampPercent(progress);

  const activate = () => {
    if (!clickable || disabled) return;
    if (status === "notInstalled" && onDownload) {
      onDownload(id);
    } else {
      onSelect?.(id);
    }
  };

  const getFrameClasses = (): string => {
    if (frame === "engine") {
      return selected
        ? "border-logo-primary bg-logo-primary/5"
        : "border-mid-gray/20";
    }
    if (isActive) return "border-2 border-logo-primary/50 bg-logo-primary/10";
    if (variant === "featured")
      return "border-2 border-logo-primary/25 bg-logo-primary/5";
    return "border-2 border-mid-gray/20";
  };

  const getInteractiveClasses = (): string => {
    if (!clickable) return "";
    if (disabled) return "opacity-50 cursor-not-allowed";
    return "cursor-pointer hover:border-logo-primary/50 hover:bg-logo-primary/5 hover:shadow-lg hover:scale-[1.01] active:scale-[0.99] group";
  };

  const stopThen = (run: () => void) => (event: React.MouseEvent) => {
    event.preventDefault();
    event.stopPropagation();
    run();
  };

  const titleClass = `${styles.title} ${clickable && frame === "card" ? "group-hover:text-logo-primary" : ""}`;

  const statusBadgeNodes = statusBadges
    ? [
        isActive ? (
          <Badge key="status-active" variant="primary">
            <Check className="w-3 h-3 mr-1" />
            {labels.active}
          </Badge>
        ) : null,
        userAdded ? (
          <Badge key="status-user" variant="secondary">
            {labels.userAdded}
          </Badge>
        ) : null,
        status === "switching" ? (
          <Badge key="status-switching" variant="secondary">
            <Loader2 className="w-3 h-3 mr-1 animate-spin" />
            {labels.switching}
          </Badge>
        ) : null,
      ]
    : null;

  const selectButton =
    actions.select && onSelect && !rowSelect ? (
      <Button
        data-testid="model-row-select"
        variant="primary-soft"
        size="sm"
        onClick={stopThen(() => onSelect(id))}
      >
        {labels.select}
      </Button>
    ) : null;

  const downloadButton =
    actions.download && onDownload && !rowSelect ? (
      <Button
        data-testid="model-row-download"
        variant="primary-soft"
        size="sm"
        onClick={stopThen(() => onDownload(id))}
      >
        {labels.download}
      </Button>
    ) : null;

  const deleteButton =
    actions.delete && onDelete ? (
      <Button
        data-testid="model-row-delete"
        variant="ghost"
        size="sm"
        onClick={stopThen(() => onDelete(id))}
        title={labels.deleteModel ? labels.deleteModel(name) : labels.delete}
        className="flex items-center gap-1.5 text-logo-primary/85 hover:text-logo-primary hover:bg-logo-primary/10"
      >
        <Trash2 className="w-3.5 h-3.5" />
        <span>{labels.delete}</span>
      </Button>
    ) : null;

  const editButton =
    userAdded && onEdit ? (
      <Button
        data-testid="model-row-edit"
        variant="ghost"
        size="sm"
        onClick={stopThen(() => onEdit(id))}
        className="flex items-center gap-1.5"
      >
        <Pencil className="w-3.5 h-3.5" />
        <span>{labels.edit}</span>
      </Button>
    ) : null;

  const ownActions = (
    <>
      {editButton}
      {deleteButton}
    </>
  );

  const progressBlock =
    statusDetails && status === "downloading" ? (
      <div className="w-full mt-3">
        <div
          role="progressbar"
          aria-label={labels.progress}
          aria-valuemin={0}
          aria-valuemax={100}
          aria-valuenow={percent}
          className="w-full h-1.5 bg-mid-gray/20 rounded-full overflow-hidden"
        >
          <div
            className="h-full bg-logo-primary rounded-full transition-all duration-300"
            style={{ width: `${percent}%` }}
          />
        </div>
        <div className="flex items-center justify-between text-xs mt-1">
          <span className="text-text/50">
            {labels.downloading
              ? labels.downloading(Math.round(percent))
              : `${Math.round(percent)}%`}
          </span>
          <div className="flex items-center gap-2">
            {speedMbPerSecond !== undefined && speedMbPerSecond > 0 && (
              <span className="tabular-nums text-text/50">
                {labels.speed
                  ? labels.speed(speedMbPerSecond.toFixed(1))
                  : speedMbPerSecond.toFixed(1)}
              </span>
            )}
            {actions.cancel && onCancel && (
              <Button
                data-testid="model-row-cancel"
                variant="danger-ghost"
                size="sm"
                onClick={stopThen(() => onCancel(id))}
                aria-label={labels.cancelDownload ?? labels.cancel}
              >
                {labels.cancel}
              </Button>
            )}
          </div>
        </div>
      </div>
    ) : null;

  const busyLabel =
    status === "verifying"
      ? labels.verifying
      : status === "extracting"
        ? labels.extracting
        : null;

  const busyBlock =
    statusDetails && busyLabel !== null ? (
      <div className="w-full mt-3">
        <div className="w-full h-1.5 bg-mid-gray/20 rounded-full overflow-hidden">
          <div className="h-full bg-logo-primary rounded-full animate-pulse w-full" />
        </div>
        <p className="text-xs text-text/50 mt-1">{busyLabel}</p>
      </div>
    ) : null;

  const errorBlock =
    statusDetails && status === "error" ? (
      <div className="flex flex-col gap-2 mt-2">
        {errorMessage && (
          <p role="alert" className="text-xs text-red-400">
            {errorMessage}
          </p>
        )}
        {actions.retry && onRetry && (
          <div>
            <Button
              data-testid="model-row-retry"
              variant="secondary"
              size="sm"
              onClick={stopThen(() => onRetry(id))}
            >
              {labels.retry}
            </Button>
          </div>
        )}
      </div>
    ) : null;

  const rootClasses = [
    styles.root,
    getFrameClasses(),
    getInteractiveClasses(),
    className,
  ]
    .filter(Boolean)
    .join(" ");

  const Root = containerTag;

  return (
    <Root
      {...dataAttributes}
      data-testid={testId}
      data-status={status}
      data-active={isActive ? "true" : "false"}
      data-variant={variant}
      data-selected={selected === undefined ? undefined : String(selected)}
      aria-current={isActive ? "true" : undefined}
      aria-labelledby={titleId}
      role={clickable ? "button" : undefined}
      tabIndex={clickable ? 0 : undefined}
      onClick={clickable ? activate : undefined}
      onKeyDown={
        clickable
          ? (event: React.KeyboardEvent) => {
              if (event.key === "Enter") activate();
            }
          : undefined
      }
      className={rootClasses}
    >
      <div className={styles.header}>
        <div className={styles.left}>
          <div className={styles.nameRow}>
            <h3 id={titleId} className={titleClass}>
              {name}
            </h3>
            {badges.map(renderBadge)}
            {statusBadgeNodes}
          </div>
          {description !== undefined && (
            <p className={styles.description}>{description}</p>
          )}
          {licence && <p className="text-xs text-text/50">{licence}</p>}
          {details}
        </div>
        {aside}
        {action}
        {selectButton}
        {downloadButton}
      </div>

      {frame === "card" && (
        <>
          <hr className="w-full border-mid-gray/20" />
          <div className="flex items-center gap-3 w-full -mb-0.5 mt-0.5 h-5">
            {tags}
            {meta && (
              <span className="flex items-center gap-1.5 ms-auto text-xs text-text/50">
                {meta}
              </span>
            )}
            {ownActions}
          </div>
        </>
      )}

      {progressBlock}
      {busyBlock}
      {errorBlock}
      {frame === "engine" && ownActions}
      {showBody && children}
    </Root>
  );
};

export default ModelRow;
