import { defineConfig } from 'vite';
import { svelte } from '@sveltejs/vite-plugin-svelte';
import { readFileSync } from 'fs';
import { fileURLToPath } from 'url';

const file = fileURLToPath(new URL('package.json', import.meta.url));
const json = readFileSync(file, 'utf8');
const pkg = JSON.parse(json);

// https://vitejs.dev/config/
export default defineConfig(({ command, mode }) => {
    return {
        plugins: [svelte()],
        define: {
            __APP_VERSION__: JSON.stringify(pkg.version),
        },
        publicDir: 'public',
        server: {
            headers: {
                // Allow blob: URLs for camera preview frames (QR scanner).
                'Content-Security-Policy':
                    "default-src 'self'; script-src 'self' 'unsafe-inline' 'wasm-unsafe-eval'; style-src 'self' 'unsafe-inline'; font-src 'self'; connect-src 'self' ipc://localhost ws://localhost:* wss://localhost:*; img-src 'self' data: blob:; media-src 'self' blob: mediastream:;",
            },
        },
    };
});
