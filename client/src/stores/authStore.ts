import { create } from "zustand";

/**
 * Login state. The session itself lives in an HttpOnly cookie that this code
 * can't read (on purpose: neither can an injected script). The client only
 * learns whether it is logged in from the server's answers: 200 from
 * /api/auth/me, or 401 from any request.
 */
export type AuthState = "checking" | "logged-out" | "logged-in";

type AuthStore = {
  state: AuthState;
  username?: string;
  /** Shown above the login form, e.g. why the user was logged out. */
  notice?: string;

  setLoggedIn: (username: string) => void;
  setLoggedOut: (notice?: string) => void;
};

export const useAuthStore = create<AuthStore>((set) => ({
  state: "checking",

  setLoggedIn: (username) => set({ state: "logged-in", username, notice: undefined }),
  setLoggedOut: (notice) => set({ state: "logged-out", username: undefined, notice }),
}));
