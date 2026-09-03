import { Navigate, Route, Routes } from "react-router";

import { RequireSession } from "@/auth/require-session";
import { AppLayout } from "@/layout/app-layout";
import { AccountsPage } from "@/pages/accounts-page";
import { BindingsPage } from "@/pages/bindings-page";
import { DevicesPage } from "@/pages/devices-page";
import { EnrollmentPage } from "@/pages/enrollment-page";
import { LoginPage } from "@/pages/login-page";
import { PreparationPage } from "@/pages/preparation-page";
import { SeatsPage } from "@/pages/seats-page";
import { TargetsPage } from "@/pages/targets-page";

export function App() {
  return (
    <Routes>
      <Route path="/login" element={<LoginPage />} />
      <Route element={<RequireSession />}>
        <Route element={<AppLayout />}>
          <Route index element={<Navigate to="/seats" replace />} />
          <Route path="/preparation" element={<PreparationPage />} />
          <Route path="/seats" element={<SeatsPage />} />
          <Route path="/accounts" element={<AccountsPage />} />
          <Route path="/bindings" element={<BindingsPage />} />
          <Route path="/devices" element={<DevicesPage />} />
          <Route path="/enrollment" element={<EnrollmentPage />} />
          <Route path="/targets" element={<TargetsPage />} />
        </Route>
      </Route>
      <Route path="*" element={<Navigate to="/" replace />} />
    </Routes>
  );
}
