// @ts-check
import { defineConfig } from 'astro/config';

import react from '@astrojs/react';
import tailwindcss from '@tailwindcss/vite';

// https://astro.build/config
export default defineConfig({
  integrations: [react()],

  // The tutorial is a sequence, so /tutorial/ means its first chapter. Every
  // chapter is then a page of its own, named by the same slug the contents in
  // `src/lib/tutorial.ts` uses.
  redirects: {
    '/tutorial': '/tutorial/getting-started/',
  },

  vite: {
    plugins: [tailwindcss()]
  }
});