import { defineConfig } from 'vite';
import react from '@vitejs/plugin-react';

// Vite runs this config in Node and provides __dirname / process; declare them
// ambiently so tsc passes without @types/node (this project doesn't depend on it).
declare const __dirname: string;
declare const process: { env: Record<string, string | undefined> };

// DUCKLE_WEB=1 builds the server/browser edition (#75 phase 2): no Tauri, so
// `@tauri-apps/api/core` is aliased to a shim that routes invoke() over HTTP to
// the duckle-runner web API. Output goes to dist-web so the desktop dist stays
// untouched.
const web = process.env.DUCKLE_WEB === '1';

// https://vitejs.dev/config/
export default defineConfig({
    plugins: [react()],

    // Tauri injects env vars; tell Vite to expose them.
    envPrefix: ['VITE_', 'TAURI_ENV_*'],

    // Tauri owns the console output for prettier dev UX.
    clearScreen: false,

    resolve: web
        ? {
              alias: {
                  '@tauri-apps/api/core': __dirname + '/src/web-shim/tauri-core.ts',
              },
          }
        : undefined,

    server: {
        port: 5173,
        strictPort: true,
        watch: {
            ignored: ['**/apps/desktop/**', '**/target/**'],
        },
    },

    build: {
        target: 'es2022',
        outDir: web ? 'dist-web' : 'dist',
        // Smaller bundles; Tauri ships the webview which already supports modern JS.
        minify: 'esbuild',
        sourcemap: false,
    },
});
