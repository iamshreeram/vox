import React from "react";

interface ButtonProps extends React.ButtonHTMLAttributes<HTMLButtonElement> {
  variant?:
    | "primary"
    | "primary-soft"
    | "secondary"
    | "warning"
    | "danger"
    | "danger-ghost"
    | "ghost";
  size?: "sm" | "md" | "lg";
}

export const Button: React.FC<ButtonProps> = ({
  children,
  className = "",
  variant = "primary",
  size = "md",
  ...props
}) => {
  const baseClasses =
    "font-medium rounded-lg border transition-colors disabled:cursor-not-allowed cursor-pointer";

  const variantClasses = {
    primary:
      "btn-primary text-on-accent bg-linear-to-r/srgb from-accent-a to-accent-b border-transparent hover:brightness-110 disabled:opacity-50",
    "primary-soft":
      "text-text bg-logo-primary/20 border-transparent hover:bg-logo-primary/30 disabled:opacity-50",
    secondary:
      "text-text bg-mid-gray/10 border-mid-gray/20 hover:bg-background-ui/30 hover:border-logo-primary disabled:text-muted",
    // Secondary's neutral resting look, but hover/focus use the semantic
    // --color-warning token (theme.css) instead of the pink accent — for
    // buttons sitting on warning surfaces like SecureInputWarning.
    // Text is the full text token (4.5:1 on the mid-gray fill). Disabled dims to the muted token,
    // not opacity, so the disabled label keeps 4.5:1 (a11yTokens.test.ts).
    warning:
      "text-text bg-mid-gray/10 border-mid-gray/20 hover:bg-warning/15 hover:border-warning disabled:text-muted",
    danger:
      "text-white bg-danger border-mid-gray/20 hover:bg-danger hover:border-danger disabled:opacity-50",
    "danger-ghost":
      "text-danger border-transparent hover:text-danger hover:bg-danger-soft focus-visible:bg-danger-soft disabled:opacity-50",
    ghost:
      "text-current border-transparent hover:bg-mid-gray/10 hover:border-logo-primary focus-visible:bg-mid-gray/20 disabled:opacity-50",
  };

  const sizeClasses = {
    sm: "px-2 py-1 text-xs",
    md: "px-4 py-[5px] text-sm",
    lg: "px-4 py-2 text-base",
  };

  return (
    <button
      className={`${baseClasses} ${variantClasses[variant]} ${sizeClasses[size]} ${className}`}
      {...props}
    >
      {children}
    </button>
  );
};
