import { StrictMode } from "react";
import ReactDOM from "react-dom/client";
import { BrowserRouter } from "react-router";

import { App } from "@/App";
import { SessionProvider } from "@/auth/session-provider";
import { ThemeProvider } from "@/components/theme-provider";
import { Toaster } from "@/components/ui/sonner";
import { TooltipProvider } from "@/components/ui/tooltip";
import "@/index.css";

ReactDOM.createRoot(document.getElementById("root")!).render(
  <StrictMode>
    <ThemeProvider>
      <TooltipProvider delayDuration={200}>
        <SessionProvider>
          <BrowserRouter>
            <App />
          </BrowserRouter>
          <Toaster />
        </SessionProvider>
      </TooltipProvider>
    </ThemeProvider>
  </StrictMode>,
);
