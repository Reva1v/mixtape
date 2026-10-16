import { defineConfig } from "vite";
import { svelte } from "@sveltejs/vite-plugin-svelte";
import tailwindcss from "@tailwindcss/vite";
import Icons from "unplugin-icons/vite";
import { fileURLToPath } from "node:url";

export default defineConfig({
  plugins: [tailwindcss(), svelte(), Icons({ compiler: "svelte" })],
  resolve: { alias: { $lib: fileURLToPath(new URL("./src/lib", import.meta.url)) } },
  clearScreen: false,
  server: { port: 5173, strictPort: true },
  build: { target: "es2023", outDir: "dist", emptyOutDir: true },
});
