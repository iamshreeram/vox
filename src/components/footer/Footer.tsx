import React, { useState, useEffect } from "react";
import { getVersion } from "@tauri-apps/api/app";

import ModelSelector from "../model-selector";
import UpdateChecker from "../update-checker";
import { versionLabel } from "./footerLabels";

const Footer: React.FC = () => {
  const [version, setVersion] = useState("");

  useEffect(() => {
    const fetchVersion = async () => {
      try {
        const appVersion = await getVersion();
        setVersion(appVersion);
      } catch (error) {
        console.error("Failed to get app version:", error);
        setVersion("0.1.2");
      }
    };

    fetchVersion();
  }, []);

  const label = versionLabel(version);

  return (
    <div className="w-full border-t border-mid-gray/20 pt-3">
      <div className="flex justify-between items-center text-xs px-4 pb-3 text-muted">
        <div className="flex min-w-0 items-center gap-4">
          <ModelSelector />
        </div>

        {/* Update Status */}
        <div className="flex shrink-0 items-center gap-1">
          <UpdateChecker />
          {label !== null && (
            <>
              <span>•</span>
              <span title={label}>{label}</span>
            </>
          )}
        </div>
      </div>
    </div>
  );
};

export default Footer;
