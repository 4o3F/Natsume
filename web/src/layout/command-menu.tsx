import { useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { useNavigate } from "react-router";
import { LogOut, Monitor } from "lucide-react";

import { unwrap } from "@/api/errors";
import type { SeatResponse } from "@/api/generated/types";
import { useSessionScope } from "@/auth/session-context";
import { useLogout } from "@/auth/use-session";
import { useTheme } from "@/components/theme-provider";
import {
  CommandDialog,
  CommandEmpty,
  CommandGroup,
  CommandInput,
  CommandItem,
  CommandList,
  CommandSeparator,
  CommandShortcut,
} from "@/components/ui/command";
import { visibleNavigation } from "./navigation";
import { themeOptions } from "./theme-options";

export function CommandMenu({
  open,
  onOpenChange,
  isAdmin,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  isAdmin: boolean;
}) {
  const { api } = useSessionScope();
  const navigate = useNavigate();
  const logout = useLogout();
  const { setTheme } = useTheme();
  const [search, setSearch] = useState("");
  const seats = useQuery({
    queryKey: ["seats"],
    queryFn: async ({ signal }) =>
      unwrap<SeatResponse[]>(await api.listSeats({ signal })),
    enabled: open,
  });

  function run(action: () => void) {
    onOpenChange(false);
    setSearch("");
    action();
  }

  return (
    <CommandDialog
      open={open}
      onOpenChange={onOpenChange}
      title="Command menu"
      description="Jump to a page or seat, or run an action"
    >
      <CommandInput
        placeholder="Search pages, seats, actions…"
        value={search}
        onValueChange={setSearch}
      />
      <CommandList>
        <CommandEmpty>No matches.</CommandEmpty>
        {visibleNavigation(isAdmin).map((group) => (
          <CommandGroup key={group.label} heading={group.label}>
            {group.items.map((item) => (
              <CommandItem
                key={item.to}
                value={`${item.label} ${item.description}`}
                onSelect={() => run(() => navigate(item.to))}
              >
                <item.icon aria-hidden="true" />
                <span>{item.label}</span>
                <span className="ml-2 truncate text-xs text-muted-foreground">
                  {item.description}
                </span>
              </CommandItem>
            ))}
          </CommandGroup>
        ))}
        {search.trim() && seats.data && seats.data.length > 0 && (
          <>
            <CommandSeparator />
            <CommandGroup heading="Seats">
              {seats.data.slice(0, 200).map((seat) => (
                <CommandItem
                  key={seat.seat_id}
                  value={`seat ${seat.seat_code}`}
                  onSelect={() =>
                    run(() =>
                      navigate(
                        `/devices?q=${encodeURIComponent(seat.seat_code)}`,
                      ),
                    )
                  }
                >
                  <Monitor aria-hidden="true" />
                  <span className="font-mono">{seat.seat_code}</span>
                  <CommandShortcut>Open in Devices</CommandShortcut>
                </CommandItem>
              ))}
            </CommandGroup>
          </>
        )}
        <CommandSeparator />
        <CommandGroup heading="Preferences">
          {themeOptions.map((option) => (
            <CommandItem
              key={option.value}
              value={`theme ${option.label}`}
              onSelect={() => run(() => setTheme(option.value))}
            >
              <option.icon aria-hidden="true" />
              {option.label} theme
            </CommandItem>
          ))}
          <CommandItem
            value="logout sign out"
            onSelect={() => run(() => logout.mutate())}
          >
            <LogOut aria-hidden="true" />
            Log out
          </CommandItem>
        </CommandGroup>
      </CommandList>
    </CommandDialog>
  );
}
