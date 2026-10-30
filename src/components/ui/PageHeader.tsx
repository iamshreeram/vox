import React from "react";

interface PageHeaderProps {
  /** The page title. It is the page's only h1; sections on the page use h2. */
  title: string;
  /** Optional one-line explanation under the title. */
  description?: string;
}

/**
 * The title block shown at the top of every settings page (UI enterprise S4a, U9). One shared
 * component, so every page has the same title size, weight and spacing.
 */
export const PageHeader: React.FC<PageHeaderProps> = ({
  title,
  description,
}) => (
  <div className="max-w-3xl w-full mx-auto px-1">
    <h1 className="text-xl font-semibold">{title}</h1>
    {description && <p className="mt-1 text-sm text-text/60">{description}</p>}
  </div>
);
