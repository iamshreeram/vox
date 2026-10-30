import React from "react";

interface BadgeProps {
  children: React.ReactNode;
  variant?: "primary" | "success" | "secondary";
  className?: string;
}

const Badge: React.FC<BadgeProps> = ({
  children,
  variant = "primary",
  className = "",
}) => {
  const variantClasses = {
    // The solid accent fill with its on-accent text (>= 4.5:1 in every palette), not the logo colour
    // with body text, which measured 1.6:1 on the dark theme.
    primary: "badge-primary bg-accent-ui text-on-accent",
    success: "bg-success-soft text-success",
    secondary: "bg-mid-gray/20 text-text/70",
  };

  return (
    <span
      className={`inline-flex items-center px-3 py-1 rounded-full text-xs font-medium ${variantClasses[variant]} ${className}`}
    >
      {children}
    </span>
  );
};

export default Badge;
