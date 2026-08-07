import * as path from "node:path";
import { defineConfig } from "@rspress/core";
import { remarkAclSyntax } from "./remark-acl-syntax";

const base = process.env.DOCS_BASE ?? "/Use/";
const siteOrigin = process.env.DOCS_ORIGIN ?? "https://a3s-lab.github.io";

export default defineConfig({
  root: path.join(__dirname, "docs"),
  base,
  siteOrigin,
  title: "A3S Use",
  description:
    "A3S Use installs native tools and cognitive capabilities as one reviewed, exact package graph.",
  lang: "zh",
  icon: "/favicon.svg",
  logo: "/a3s-use-mark.svg",
  logoText: "A3S Use",
  outDir: "doc_build",
  llms: true,
  markdown: {
    remarkPlugins: [remarkAclSyntax],
  },
  locales: [
    {
      lang: "zh",
      label: "简体中文",
      title: "A3S Use",
      description:
        "用一张精确包图安装原生工具与认知能力；先审查，再原子发布新的能力代际。",
    },
    {
      lang: "en",
      label: "English",
      title: "A3S Use",
      description:
        "Install native tools and cognitive capabilities as one reviewed, exact package graph.",
    },
  ],
  head: [
    ["meta", { name: "theme-color", content: "#ffffff" }],
    ["meta", { property: "og:type", content: "website" }],
    ["meta", { property: "og:site_name", content: "A3S Use" }],
    [
      "meta",
      {
        property: "og:image",
        content: `${siteOrigin}${base}social-card.png`,
      },
    ],
    ["meta", { name: "twitter:card", content: "summary_large_image" }],
    (route) => [
      "link",
      {
        rel: "canonical",
        href: `${siteOrigin}${base.replace(/\/$/, "")}${route.routePath}`,
      },
    ],
  ],
  themeConfig: {
    darkMode: "force-light",
    search: true,
    localeRedirect: "never",
    enableContentAnimation: true,
    editLink: {
      docRepoBaseUrl: "https://github.com/A3S-Lab/Use/tree/main/website/docs",
    },
    lastUpdated: true,
    llmsUI: {
      placement: "outline",
      viewOptions: ["markdownLink", "chatgpt", "claude"],
    },
    socialLinks: [
      {
        icon: "github",
        mode: "link",
        content: "https://github.com/A3S-Lab/Use",
      },
    ],
  },
});
