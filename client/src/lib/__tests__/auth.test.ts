import { describe, expect, test } from "bun:test";
import { loginErrorMessage } from "@/lib/auth";

describe("loginErrorMessage", () => {
  test("the same message for a wrong password and an unknown user", () => {
    expect(loginErrorMessage(401, { code: "unauthorized", message: "wrong username or password" }, null)).toBe(
      "Wrong username or password.",
    );
  });

  test("rate limited: shows how long to wait", () => {
    expect(loginErrorMessage(429, null, "4")).toBe("Too many failed attempts. Try again in 4 s.");
    expect(loginErrorMessage(429, null, "900")).toBe("Too many failed attempts. Try again in 15 min.");
    expect(loginErrorMessage(429, null, null)).toBe("Too many failed attempts. Try again later.");
  });

  test("other errors", () => {
    expect(loginErrorMessage(403, null, null)).toContain("ALLOWED_ORIGINS");
    expect(loginErrorMessage(500, { code: "internal", message: "database error" }, null)).toBe("database error");
    expect(loginErrorMessage(502, null, null)).toBe("Login failed (HTTP 502).");
  });
});
