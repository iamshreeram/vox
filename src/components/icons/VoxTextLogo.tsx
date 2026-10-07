import React from "react";

/* eslint-disable i18next/no-literal-string -- this whole file is a static
   brand wordmark glyph (SVG <text>), never user-facing translatable copy. */

/**
 * "Vox" wordmark. Two-tone like the legacy Vox mark (a soft stroke/shadow
 * layer behind a solid fill layer) but drawn as real text instead of traced
 * letterforms, and themed entirely through the `.logo-primary` /
 * `.logo-stroke` CSS classes (see theme.css) so it follows the gold
 * light/dark palette automatically instead of hardcoding a color.
 */
const VoxTextLogo = ({
  width,
  height,
  className,
}: {
  width?: number;
  height?: number;
  className?: string;
}) => {
  return (
    <svg
      width={width}
      height={height}
      className={className}
      viewBox="0 0 320 110"
      fill="none"
      xmlns="http://www.w3.org/2000/svg"
    >
      <text
        x="50%"
        y="58%"
        dominantBaseline="middle"
        textAnchor="middle"
        className="logo-stroke"
        fontFamily="'Avenir Next', 'Segoe UI', system-ui, sans-serif"
        fontWeight="800"
        fontSize="96"
        letterSpacing="4"
        transform="translate(4, 4)"
      >
        Vox
      </text>
      <text
        x="50%"
        y="58%"
        dominantBaseline="middle"
        textAnchor="middle"
        className="logo-primary"
        fontFamily="'Avenir Next', 'Segoe UI', system-ui, sans-serif"
        fontWeight="800"
        fontSize="96"
        letterSpacing="4"
      >
        Vox
      </text>
    </svg>
  );
};

export default VoxTextLogo;
