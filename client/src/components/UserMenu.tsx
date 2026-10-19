import { useState } from "react";
import { useAuthStore } from "@/stores/authStore";
import { logout } from "@/transports/https";

/** Who is logged in, plus logout (this browser) and logout everywhere. */
export function UserMenu() {
  const username = useAuthStore((s) => s.username);
  const [busy, setBusy] = useState(false);

  function run(everywhere: boolean) {
    setBusy(true);
    void logout(everywhere).finally(() => setBusy(false));
  }

  const button = "rounded px-2 py-1 text-xs text-zinc-300 hover:bg-zinc-800 disabled:opacity-50";

  return (
    <div className="flex items-center gap-1 text-sm">
      <span className="px-1 text-zinc-400">{username}</span>
      <button type="button" className={button} disabled={busy} onClick={() => run(false)}>
        Log out
      </button>
      <button
        type="button"
        className={button}
        disabled={busy}
        onClick={() => run(true)}
        title="End every session of this user, on all devices"
      >
        Log out everywhere
      </button>
    </div>
  );
}
