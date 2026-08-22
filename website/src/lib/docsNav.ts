import type { CollectionEntry } from 'astro:content';

export interface DocsNavLink {
  slug: string;
  label: string;
}

const H1_RE = /^#\s+(.+)$/m;

function titleFor(entry: CollectionEntry<'docs'>): string {
  const match = entry.body?.match(H1_RE);
  return match ? match[1].trim() : entry.id;
}

/**
 * Flat, alphabetical nav built straight from the docs collection -- no
 * categories, no parsing docs/index.md's prose. A doc added to the olive
 * repo shows up here on the next build with zero website changes; nothing
 * here depends on how index.md happens to be formatted.
 */
export function buildDocsNav(entries: CollectionEntry<'docs'>[]): DocsNavLink[] {
  const links = entries
    .filter((e) => e.id !== 'index')
    .map((e) => ({ slug: e.id, label: titleFor(e) }));

  links.sort((a, b) => {
    if (a.slug === 'introduction') return -1;
    if (b.slug === 'introduction') return 1;
    return a.label.localeCompare(b.label);
  });

  return links;
}
