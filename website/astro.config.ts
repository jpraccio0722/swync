import { defineConfig } from 'astro/config';

import mdx from '@astrojs/mdx';
import { unified } from '@astrojs/markdown-remark';
import react from '@astrojs/react';
import tailwindcss from '@tailwindcss/vite';

import { rehypeSwync } from './src/lib/highlight.ts';

// https://astro.build/config
export default defineConfig({
  integrations: [react(), mdx()],

  markdown: {
    // The snippets are swync, a language Shiki has no grammar for — and a
    // grammar written for it here would be a second answer to what a swync
    // token is, drifting from the app's within a release. So Shiki is off and
    // `rehypeSwync` colours the ```swync fences with the app's own tokenizer
    // instead; see `src/lib/highlight.ts`. Classes rather than Shiki's inline
    // styles, which is also what lets the palette have a light half.
    syntaxHighlight: false,
    // `processor` rather than the `rehypePlugins` shortcut beside it: that one
    // is deprecated in Astro 7, and warns on every build.
    processor: unified({ rehypePlugins: [rehypeSwync] }),
  },

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