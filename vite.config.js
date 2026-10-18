import { defineConfig } from "vite";

export default defineConfig({
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    watch: { ignored: ["**/src-tauri/**"] },
  },
  build: {
    target: "chrome105",
    outDir: "dist",
    rollupOptions: { input: { drawer: "index.html", shelf: "shelf.html" } },
  },
});
