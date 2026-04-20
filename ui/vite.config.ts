import { defineConfig } from 'vite'
import react from '@vitejs/plugin-react'
import tailwindcss from '@tailwindcss/vite'
import path from 'path'

export default defineConfig({
  plugins: [react(), tailwindcss()],
  resolve: {
    alias: {
      '@': path.resolve(__dirname, './src'),
    },
  },
  server: {
    proxy: {
      '/rpc': 'http://127.0.0.1:9749',
      '/api': 'http://127.0.0.1:9749',
    },
  },
  build: {
    outDir: 'dist',
  },
})
