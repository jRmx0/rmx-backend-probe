/**
 * Production build: `bun run build` → dist/.
 *
 * Same as `bun build ./public/index.html --minify ...`, but through the
 * Bun.build() API, because the CLI does not load the Tailwind plugin from
 * bunfig.toml (only the dev server does) and would ship unprocessed CSS.
 *
 * Asset URLs get the /probe/ prefix, where probe-server serves the files.
 */

import tailwind from "bun-plugin-tailwind";
import { rm } from "node:fs/promises";

const outdir = "dist";
await rm(outdir, { recursive: true, force: true });

const result = await Bun.build({
  entrypoints: ["./public/index.html"],
  outdir,
  target: "browser",
  minify: true,
  sourcemap: "linked",
  publicPath: "/probe/",
  env: "BUN_PUBLIC_*",
  define: { "process.env.NODE_ENV": JSON.stringify("production") },
  plugins: [tailwind],
});

if (!result.success) {
  for (const log of result.logs) console.error(log);
  process.exit(1);
}

for (const out of result.outputs) {
  const kb = (out.size / 1024).toFixed(1).padStart(8);
  console.log(`${kb} KB  ${out.path.replace(process.cwd(), ".")}`);
}
