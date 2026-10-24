import { NavLink } from "react-router";
import { useSessionScope } from "@/auth/session-context";
import { buttonVariants } from "@/components/ui/button";
import { cn } from "@/lib/utils";

export function PrintNavigation() {
  const { identity } = useSessionScope();
  const links = [
    { to: "/print", label: "Executions" },
    ...(identity?.role === "admin"
      ? [
          { to: "/print/printers", label: "Printers and pairing" },
          { to: "/print/sources", label: "Print sources" },
        ]
      : []),
  ];
  return (
    <nav aria-label="Print navigation" className="flex flex-wrap gap-2">
      {links.map((link) => (
        <NavLink
          key={link.to}
          to={link.to}
          end
          className={({ isActive }) =>
            cn(
              buttonVariants({
                variant: isActive ? "secondary" : "ghost",
                size: "sm",
              }),
            )
          }
        >
          {link.label}
        </NavLink>
      ))}
    </nav>
  );
}
