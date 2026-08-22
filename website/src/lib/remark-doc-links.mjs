import { visit } from 'unist-util-visit';

// Matches a same-directory relative link to another doc, with or without a
// "./" prefix, e.g. "ownership.md", "./ownership.md", "ownership.md#foo".
// Docs don't currently use "../" or subdirectory links, so those aren't
// handled -- if that changes, this needs to widen too.
const MD_LINK = /^(?!https?:\/\/)(?:\.\/)?([a-zA-Z0-9_-]+)\.md(#.*)?$/;

export function remarkDocLinks() {
  return (tree) => {
    visit(tree, 'link', (node) => {
      const match = node.url.match(MD_LINK);
      if (match) {
        const [, slug, anchor = ''] = match;
        node.url = `/docs/${slug}/${anchor}`;
      }
    });
  };
}
