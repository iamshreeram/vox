import React, { useContext } from "react";
import { SettingLabelContext } from "./settingsGroupContext";

interface InputProps extends React.InputHTMLAttributes<HTMLInputElement> {
  variant?: "default" | "compact";
}

export const Input: React.FC<InputProps> = ({
  className = "",
  variant = "default",
  disabled,
  ...props
}) => {
  // Named by the enclosing setting row title when no aria-label is given (R2-7).
  const labelledBy = useContext(SettingLabelContext);
  const baseClasses =
    "px-2 py-1 text-sm font-semibold bg-mid-gray/10 border border-mid-gray/80 rounded-md text-start transition-all duration-150";

  const interactiveClasses = disabled
    ? "opacity-60 cursor-not-allowed bg-mid-gray/10 border-mid-gray/40"
    : "hover:bg-logo-primary/10 hover:border-logo-primary focus-visible:bg-logo-primary/20 focus-visible:border-logo-primary";

  const variantClasses = {
    default: "px-3 py-2",
    compact: "px-2 py-1",
  } as const;

  return (
    <input
      className={`${baseClasses} ${variantClasses[variant]} ${interactiveClasses} ${className}`}
      disabled={disabled}
      aria-labelledby={props["aria-label"] ? undefined : labelledBy}
      {...props}
    />
  );
};
