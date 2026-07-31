interface MarkdownNode {
  children?: MarkdownNode[];
  lang?: string | null;
  meta?: string | null;
  type?: string;
}

/**
 * Shiki does not ship an ACL grammar. ACL uses HCL-compatible lexical
 * constructs, so highlight ACL fences with that grammar while preserving ACL
 * as the language shown to readers. Parsing remains owned by a3s-acl.
 */
export function remarkAclSyntax() {
  return (tree: MarkdownNode) => {
    const visit = (node: MarkdownNode) => {
      if (node.type === "code" && node.lang?.toLowerCase() === "acl") {
        node.lang = "hcl";
        node.meta = [node.meta, "displayLanguage=ACL"]
          .filter(Boolean)
          .join(" ");
      }

      node.children?.forEach(visit);
    };

    visit(tree);
  };
}
