import { useState, type FormEvent } from "react";
import { useAuthStore } from "@/stores/authStore";
import { login } from "@/transports/https";

/**
 * The page's own login. There is no sign-up: users are created on the server
 * (`probe-server create-user`). The password goes to the server once, over
 * HTTPS; afterwards the browser holds only the session cookie.
 */
export function LoginForm() {
  const [username, setUsername] = useState("");
  const [password, setPassword] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string>();
  const notice = useAuthStore((s) => s.notice);

  async function submit(e: FormEvent) {
    e.preventDefault();
    setBusy(true);
    setError(undefined);
    try {
      await login({ username, password });
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
      setPassword("");
    } finally {
      setBusy(false);
    }
  }

  const input =
    "w-full rounded-md border border-zinc-700 bg-zinc-900 px-3 py-2 text-sm outline-none focus:border-teal-500 disabled:opacity-60";

  return (
    <div className="flex min-h-full items-center justify-center p-4">
      <form onSubmit={submit} className="flex w-full max-w-xs flex-col gap-3 rounded-md border border-zinc-800 p-5">
        <div>
          <h1 className="text-lg font-semibold">Backend probe</h1>
          <p className="text-sm text-zinc-400">Blade of Grass · log in to continue</p>
        </div>
        {notice && !error && <p className="text-sm text-zinc-300">{notice}</p>}
        <input
          className={input}
          value={username}
          onChange={(e) => setUsername(e.target.value)}
          placeholder="Username"
          aria-label="Username"
          autoComplete="username"
          autoCapitalize="none"
          autoCorrect="off"
          spellCheck={false}
          autoFocus
          required
          disabled={busy}
        />
        <input
          className={input}
          type="password"
          value={password}
          onChange={(e) => setPassword(e.target.value)}
          placeholder="Password"
          aria-label="Password"
          autoComplete="current-password"
          required
          disabled={busy}
        />
        {error && (
          <p role="alert" className="text-sm text-red-400">
            {error}
          </p>
        )}
        <button
          type="submit"
          disabled={busy || !username || !password}
          className="rounded-md bg-teal-600 px-3 py-2 text-sm font-medium text-white hover:bg-teal-500 disabled:opacity-50"
        >
          {busy ? "Logging in…" : "Log in"}
        </button>
      </form>
    </div>
  );
}
