import { defineConfig } from 'astro/config';
import { unified } from '@astrojs/markdown-remark';
import { remarkDocLinks } from './src/lib/remark-doc-links.mjs';
import olive from './olive.tmLanguage.json' with { type: 'json' };

export default defineConfig({
  site: 'https://olive.ecnivs.com',
  base: '/',
  trailingSlash: 'ignore',
  markdown: {
    processor: unified({ remarkPlugins: [remarkDocLinks] }),
    shikiConfig: {
      themes: {
        light: 'github-light',
        dark: 'github-dark',
      },
      langs: [olive],
    },
  },
});
