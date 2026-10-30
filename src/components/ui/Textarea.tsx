import React, { useContext } from "react";
import { SettingLabelContext } from "./settingsGroupContext";

interface TextareaProps
  extends React.TextareaHTMLAttributes<HTMLTextAreaElement> {
  variant?: "default" | "compact";
}

export const Textarea: React.FC<TextareaProps> = ({
  className = "",
  variant = "default",
  ...props
}) => {
  // Named by the enclosing setting row title when no aria-label is given (R2-7).
  const labelledBy = useContext(SettingLabelContext);
  const baseClasses =
    "px-2 py-1 text-sm font-semibold bg-mid-gray/10 border border-mid-gray/80 rounded-md text-start transition-[background-color,border-color] duration-150 hover:bg-logo-primary/10 hover:border-logo-primary focus-visible:bg-logo-primary/10 focus-visible:border-logo-primary resize-y";

  const variantClasses = {
    default: "px-3 py-2 min-h-[100px]",
    compact: "px-2 py-1 min-h-[80px]",
  };

  return (
    <textarea
      className={`${baseClasses} ${variantClasses[variant]} ${className}`}
      aria-labelledby={props["aria-label"] ? undefined : labelledBy}
      {...props}
    />
  );
};
