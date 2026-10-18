// @ts-check

import eslint from "@eslint/js";
import { defineConfig } from "eslint/config";
import reactHooks from "eslint-plugin-react-hooks";
import globals from "globals";
import tseslint from "typescript-eslint";

export default defineConfig(
  // ts-rs output is generated from the Rust types; don't lint it.
  { ignores: ["dist", "node_modules", "src/generated"] },
  eslint.configs.recommended,
  tseslint.configs.recommended,
  reactHooks.configs.flat["recommended-latest"],
  {
    languageOptions: {
      globals: { ...globals.browser },
    },
  },
);
