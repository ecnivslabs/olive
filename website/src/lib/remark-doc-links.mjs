import { visit } from 'unist-util-visit';

const MD_LINK = /^(?!https?:\/\/)([a-zA-Z0-9_-]+)\.md(#.*)?$/;

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
