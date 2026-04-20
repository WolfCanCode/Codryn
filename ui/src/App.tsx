import {
  Routes,
  Route,
  Navigate,
  NavLink,
  Link,
  useLocation,
} from "react-router-dom";
import { cn } from "@/lib/utils";
import logo from "@/assets/logo-2.png";
import ProjectsPage from "@/pages/ProjectsPage";
import GraphPage from "@/pages/GraphPage";
import ConfigPage from "@/pages/ConfigPage";
import AnalyticsPage from "@/pages/AnalyticsPage";

const navItems = [
  { to: "/projects", label: "Projects" },
  { to: "/analytics", label: "Analytics" },
  { to: "/config", label: "Config" },
];

export default function App() {
  const location = useLocation();
  const graphRoute = location.pathname.startsWith("/graph");
  return (
    <div className="flex h-screen overflow-hidden flex-col bg-[#fafbfc]">
      <header className="shrink-0 border-b bg-white">
        <div className="mx-auto flex h-14 max-w-[1440px] items-center gap-8 px-6">
          <Link
            to="/projects"
            className="shrink-0 rounded-md outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2"
          >
            <img
              src={logo}
              alt="codryn"
              className="h-11 object-cover sm:h-12 py-3"
            />
          </Link>
          <nav className="flex gap-1">
            {navItems.map((n) => (
              <NavLink
                key={n.to}
                to={n.to}
                className={cn(
                  "rounded-md px-3 py-1.5 text-sm font-medium transition-colors hover:bg-accent",
                  location.pathname.startsWith(n.to)
                    ? "bg-accent text-accent-foreground"
                    : "text-muted-foreground",
                )}
              >
                {n.label}
              </NavLink>
            ))}
          </nav>
        </div>
      </header>
      <main
        className={cn(
          "mx-auto flex w-full max-w-[1440px] min-h-0 flex-1 flex-col",
          graphRoute ? "overflow-hidden bg-zinc-100" : "overflow-y-auto px-6 py-6 bg-[#fafbfc]",
        )}
      >
        <Routes>
          <Route path="/" element={<Navigate to="/projects" replace />} />
          <Route path="/projects" element={<ProjectsPage />} />
          <Route path="/graph" element={<GraphPage />} />
          <Route path="/config" element={<ConfigPage />} />
          <Route path="/analytics" element={<AnalyticsPage />} />
        </Routes>
      </main>
      <footer className="shrink-0 border-t bg-white">
        <div className="mx-auto max-w-[1440px] px-6 py-3 text-center text-xs text-muted-foreground">
          Made by Tommy Le
        </div>
      </footer>
    </div>
  );
}
