/** The floating window's page (src/design/floating/index.html). SPIKE, development only. */
import "@fontsource-variable/inter/opsz.css";
import "@fontsource-variable/jetbrains-mono";
import "./floating.css";

import { createRoot } from "react-dom/client";

import { Floating } from "./Floating";

createRoot(document.getElementById("root")!).render(<Floating />);
