import React, { useContext, useEffect, useId, useRef, useState } from "react";
import { Tooltip } from "./Tooltip";
import {
  SettingLabelContext,
  SettingsGroupContext,
  groupedRowClass,
  shouldShowTooltip,
} from "./settingsGroupContext";

interface SettingContainerProps {
  title: string;
  description: string;
  children: React.ReactNode;
  /** Optional badge shown beside the title (horizontal layout only). Absent means no change. */
  badge?: React.ReactNode;
  descriptionMode?: "inline" | "tooltip";
  grouped?: boolean;
  layout?: "horizontal" | "stacked";
  disabled?: boolean;
  tooltipPosition?: "top" | "bottom";
}

export const SettingContainer: React.FC<SettingContainerProps> = ({
  title,
  description,
  children,
  descriptionMode = "tooltip",
  grouped = false,
  layout = "horizontal",
  disabled = false,
  tooltipPosition = "top",
  badge,
}) => {
  const [showTooltip, setShowTooltip] = useState(false);
  const tooltipRef = useRef<HTMLDivElement>(null);
  const { density, visible } = useContext(SettingsGroupContext);
  const titleId = useId();

  // A collapsed group must not leave its tooltip open (reopening keeps it closed).
  useEffect(() => {
    if (!visible) setShowTooltip(false);
  }, [visible]);

  // Handle click outside to close tooltip
  useEffect(() => {
    const handleClickOutside = (event: MouseEvent) => {
      if (
        tooltipRef.current &&
        !tooltipRef.current.contains(event.target as Node)
      ) {
        setShowTooltip(false);
      }
    };

    if (showTooltip) {
      document.addEventListener("mousedown", handleClickOutside);
      return () =>
        document.removeEventListener("mousedown", handleClickOutside);
    }
  }, [showTooltip]);

  const toggleTooltip = () => {
    setShowTooltip(!showTooltip);
  };

  const containerClasses = grouped
    ? density === "compact"
      ? "px-3 py-1"
      : "px-4 p-2"
    : "px-4 p-2 rounded-lg border border-mid-gray/20";

  if (layout === "stacked") {
    if (descriptionMode === "tooltip") {
      return (
        <div className={containerClasses} data-setting-title={title}>
          <div className="flex items-center gap-2 mb-2">
            <h3
              id={titleId}
              className={`text-sm font-medium ${disabled ? "text-muted" : ""}`}
            >
              {title}
            </h3>
            <div
              ref={tooltipRef}
              className="relative"
              onMouseEnter={() => setShowTooltip(true)}
              onMouseLeave={() => setShowTooltip(false)}
              onClick={toggleTooltip}
            >
              {/* A real button gives a 24px hit target (WCAG 2.5.8); the 16px icon is unchanged. Enter and
                  Space arrive as native clicks, which the wrapper's onClick turns into the tooltip toggle. */}
              <button
                type="button"
                aria-label="More information"
                className="inline-flex items-center justify-center min-w-[24px] min-h-[24px] text-muted hover:text-logo-primary cursor-help transition-colors duration-200 select-none"
              >
                <svg
                  className="w-4 h-4 pointer-events-none"
                  fill="none"
                  stroke="currentColor"
                  viewBox="0 0 24 24"
                  aria-hidden="true"
                >
                  <path
                    strokeLinecap="round"
                    strokeLinejoin="round"
                    strokeWidth={2}
                    d="M13 16h-1v-4h-1m1-4h.01M21 12a9 9 0 11-18 0 9 9 0 0118 0z"
                  />
                </svg>
              </button>
              {shouldShowTooltip(showTooltip, visible) && (
                <Tooltip targetRef={tooltipRef} position="top">
                  <p className="text-sm text-center leading-relaxed">
                    {description}
                  </p>
                </Tooltip>
              )}
            </div>
          </div>
          <div className="w-full">
            <SettingLabelContext.Provider value={titleId}>
              {children}
            </SettingLabelContext.Provider>
          </div>
        </div>
      );
    }

    return (
      <div className={containerClasses} data-setting-title={title}>
        <div className="mb-2">
          <h3
            id={titleId}
            className={`text-sm font-medium ${disabled ? "text-muted" : ""}`}
          >
            {title}
          </h3>
          <p className={`text-sm ${disabled ? "text-muted" : ""}`}>
            {description}
          </p>
        </div>
        <div className="w-full">
          <SettingLabelContext.Provider value={titleId}>
            {children}
          </SettingLabelContext.Provider>
        </div>
      </div>
    );
  }

  // Horizontal layout (default)
  const horizontalContainerClasses = grouped
    ? `flex items-center justify-between ${groupedRowClass(density)}`
    : "flex items-center justify-between min-h-12 px-4 p-2 rounded-lg border border-mid-gray/20";
  const heading = (
    <h3
      id={titleId}
      className={`text-sm font-medium ${disabled ? "text-muted" : ""}`}
    >
      {title}
    </h3>
  );

  if (descriptionMode === "tooltip") {
    return (
      <div className={horizontalContainerClasses} data-setting-title={title}>
        <div className="max-w-2/3">
          <div className="flex items-center gap-2">
            {heading}
            {badge}
            <div
              ref={tooltipRef}
              className="relative"
              onMouseEnter={() => setShowTooltip(true)}
              onMouseLeave={() => setShowTooltip(false)}
              onClick={toggleTooltip}
            >
              {/* A real button gives a 24px hit target (WCAG 2.5.8); the 16px icon is unchanged. Enter and
                  Space arrive as native clicks, which the wrapper's onClick turns into the tooltip toggle. */}
              <button
                type="button"
                aria-label="More information"
                className="inline-flex items-center justify-center min-w-[24px] min-h-[24px] text-muted hover:text-logo-primary cursor-help transition-colors duration-200 select-none"
              >
                <svg
                  className="w-4 h-4 pointer-events-none"
                  fill="none"
                  stroke="currentColor"
                  viewBox="0 0 24 24"
                  aria-hidden="true"
                >
                  <path
                    strokeLinecap="round"
                    strokeLinejoin="round"
                    strokeWidth={2}
                    d="M13 16h-1v-4h-1m1-4h.01M21 12a9 9 0 11-18 0 9 9 0 0118 0z"
                  />
                </svg>
              </button>
              {shouldShowTooltip(showTooltip, visible) && (
                <Tooltip targetRef={tooltipRef} position={tooltipPosition}>
                  <p className="text-sm text-center leading-relaxed">
                    {description}
                  </p>
                </Tooltip>
              )}
            </div>
          </div>
        </div>
        <div className="relative">
          <SettingLabelContext.Provider value={titleId}>
            {children}
          </SettingLabelContext.Provider>
        </div>
      </div>
    );
  }

  return (
    <div className={horizontalContainerClasses} data-setting-title={title}>
      <div className="max-w-2/3">
        {badge === undefined ? (
          heading
        ) : (
          <div className="flex items-center gap-2">
            {heading}
            {badge}
          </div>
        )}
        <p className={`text-sm ${disabled ? "text-muted" : ""}`}>
          {description}
        </p>
      </div>
      <div className="relative">
        <SettingLabelContext.Provider value={titleId}>
          {children}
        </SettingLabelContext.Provider>
      </div>
    </div>
  );
};
