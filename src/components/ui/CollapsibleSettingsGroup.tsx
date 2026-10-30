import React, { useEffect, useId, useState } from "react";
import { ChevronRight } from "lucide-react";
import {
  GROUP_OPEN_EVENT,
  consumePendingOpen,
  parseGroupOpenEvent,
  readOpen,
  writeOpen,
} from "./collapsibleState";
import { SettingsGroupContext } from "./settingsGroupContext";

interface CollapsibleSettingsGroupProps {
  id: string;
  /** Already translated. */
  title: string;
  /** Already translated. */
  description?: string;
  defaultOpen?: boolean;
  children: React.ReactNode;
}

export const CollapsibleSettingsGroup: React.FC<
  CollapsibleSettingsGroupProps
> = ({ id, title, description, defaultOpen = false, children }) => {
  const [open, setOpen] = useState(() => readOpen(id, defaultOpen));
  const bodyId = `settings-group-body-${useId().replace(/:/g, "")}`;

  useEffect(() => {
    if (consumePendingOpen(id)) setOpen(true);
    const onOpenRequest = (event: Event) => {
      if (parseGroupOpenEvent(event) !== id) return;
      // Already mounted: handling the event consumes the pending request too.
      consumePendingOpen(id);
      setOpen(true);
    };
    window.addEventListener(GROUP_OPEN_EVENT, onOpenRequest);
    return () => window.removeEventListener(GROUP_OPEN_EVENT, onOpenRequest);
  }, [id]);

  const toggle = () => {
    const next = !open;
    setOpen(next);
    writeOpen(id, next);
  };

  return (
    <div className="settings-card" data-group-id={id}>
      <button
        type="button"
        aria-expanded={open}
        aria-controls={bodyId}
        onClick={toggle}
        className="w-full flex items-center gap-2 px-4 py-2 text-start cursor-pointer"
      >
        <ChevronRight
          aria-hidden="true"
          className={`w-4 h-4 shrink-0 text-muted transition-transform duration-200 ${open ? "rotate-90" : ""}`}
        />
        <span className="flex-1 min-w-0">
          <span className="settings-card-title block text-xs font-medium text-muted uppercase tracking-wide">
            {title}
          </span>
          {description && open && (
            <span className="block text-xs text-muted mt-1">{description}</span>
          )}
        </span>
      </button>
      <div
        id={bodyId}
        hidden={!open}
        className="divide-y divide-mid-gray/20 border-t border-mid-gray/20"
      >
        <SettingsGroupContext.Provider
          value={{ density: "compact", visible: open }}
        >
          {children}
        </SettingsGroupContext.Provider>
      </div>
    </div>
  );
};
