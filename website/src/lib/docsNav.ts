export interface DocsNavLink {
  slug: string;
  label: string;
}

export interface DocsNavGroup {
  heading: string;
  links: DocsNavLink[];
}

const LINK_RE = /-\s*\[\*\*(.+?)\*\*\]\(([a-zA-Z0-9_-]+)\.md\)/g;

/**
 * Parses docs/index.md's own H2 groupings and linked pages, so the site's
 * sidebar is derived from the real index rather than a second, hand-kept copy.
 */
export function parseDocsNav(raw: string): DocsNavGroup[] {
  const groups: DocsNavGroup[] = [];
  const sections = raw.split(/^## /m).slice(1);

  for (const section of sections) {
    const [headingLine, ...rest] = section.split('\n');
    const heading = headingLine.replace(/\?$/, '').trim();
    const body = rest.join('\n');
    const links: DocsNavLink[] = [];
    let match: RegExpExecArray | null;
    LINK_RE.lastIndex = 0;
    while ((match = LINK_RE.exec(body)) !== null) {
      links.push({ label: match[1], slug: match[2] });
    }
    if (links.length > 0) groups.push({ heading, links });
  }

  return groups;
}
