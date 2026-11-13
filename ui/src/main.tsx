import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { App } from "./App";
import { SettingsWindow } from "./SettingsWindow";
import { isSettingsWindow } from "./settings";
import "./index.css";

// Both windows load this page; each renders its own UI.
const Root = isSettingsWindow() ? SettingsWindow : App;

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    <Root />
  </StrictMode>,
);
