import React, { useMemo } from "react";
import { getCurrentWebviewWindow } from "@tauri-apps/api/webviewWindow";
import { HudWindow } from "./components/HudWindow";
import { ToastWindow } from "./components/ToastWindow";

export const App: React.FC = () => {
  const windowLabel = useMemo(() => {
    try {
      return getCurrentWebviewWindow().label;
    } catch {
      const params = new URLSearchParams(window.location.search);
      return params.get("window") || "hud";
    }
  }, []);

  if (windowLabel === "toast") {
    return <ToastWindow />;
  }

  return <HudWindow />;
};

export default App;
