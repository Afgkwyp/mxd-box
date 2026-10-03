import path from 'node:path'
import tailwindcss from '@tailwindcss/vite'
import react from '@vitejs/plugin-react'
import { defineConfig } from 'vite'

export default defineConfig({
  plugins: [react(), tailwindcss()],
  resolve: {
    alias: {
      '@': path.resolve(import.meta.dirname, './src'),
    },
  },
  server: {
    port: 5173,
    strictPort: true,
    // 不盯 Rust 那边：cargo 编译时 target 里的 exe 被锁，vite 去 watch 会 EBUSY 直接崩掉
    watch: {
      ignored: ['**/src-tauri/**'],
    },
  },
})
