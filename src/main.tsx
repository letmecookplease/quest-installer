import { createRoot } from "react-dom/client";
import "@fontsource-variable/fira-code";
import { App } from "./App";
import "./styles.css";

createRoot(document.getElementById("root")!).render(<App />);
