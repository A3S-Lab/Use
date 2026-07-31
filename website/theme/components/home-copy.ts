export type Locale = "zh" | "en";
export type SurfaceKey = "tool" | "mcp" | "skill" | "ui" | "okf";

type SurfaceCopy = {
  label: string;
  kind: string;
  title: string;
  body: string;
  evidence: string[];
};

type HomeCopy = {
  eyebrow: string;
  titleLead: string;
  titleAccent: string;
  subtitle: string;
  getStarted: string;
  github: string;
  statusLabel: string;
  available: string;
  building: string;
  foundationLabel: string;
  platformLabel: string;
  installLabel: string;
  installHint: string;
  copy: string;
  copied: string;
  consoleLabel: string;
  consolePackage: string;
  consoleTarget: string;
  consoleTrust: string;
  consoleGeneration: string;
  consoleReady: string;
  modelEyebrow: string;
  modelTitle: string;
  modelBody: string;
  nativePlane: string;
  nativeTitle: string;
  nativeBody: string;
  cognitivePlane: string;
  cognitiveTitle: string;
  cognitiveBody: string;
  surfaceHint: string;
  surfaces: Record<SurfaceKey, SurfaceCopy>;
  lifecycleEyebrow: string;
  lifecycleTitle: string;
  lifecycleBody: string;
  lifecycle: Array<{ number: string; title: string; body: string }>;
  architectureEyebrow: string;
  architectureTitle: string;
  architectureBody: string;
  source: string;
  manager: string;
  managerBody: string;
  engine: string;
  engineBody: string;
  planes: string;
  planesBody: string;
  hosts: string;
  hostsBody: string;
  architectureLink: string;
  trustEyebrow: string;
  trustTitle: string;
  trustBody: string;
  trustCards: Array<{ title: string; body: string; tag: string }>;
  platformEyebrow: string;
  platformTitle: string;
  platformBody: string;
  supported: string;
  preview: string;
  ctaEyebrow: string;
  ctaTitle: string;
  ctaBody: string;
  ctaPrimary: string;
  ctaSecondary: string;
  footer: string;
};

export const homeCopy: Record<Locale, HomeCopy> = {
  zh: {
    eyebrow: "OPEN SOURCE · AI NATIVE PACKAGE MANAGER",
    titleLead: "为软件安装包。",
    titleAccent: "为 Agent 安装能力。",
    subtitle:
      "A3S Use 为 Linux、macOS 与 Windows 上的原生工具和 A3S 认知插件提供统一、可验证的包生命周期。",
    getStarted: "开始使用",
    github: "查看 GitHub",
    statusLabel: "实现状态",
    available: "已可用",
    building: "开发中",
    foundationLabel: "v0.2 包管理基础",
    platformLabel: "认知插件平台",
    installLabel: "安装已验证版本",
    installHint: "通过 A3S CLI 安装完整发布包",
    copy: "复制",
    copied: "已复制",
    consoleLabel: "PACKAGE INSPECTION",
    consolePackage: "包",
    consoleTarget: "目标",
    consoleTrust: "信任",
    consoleGeneration: "代际",
    consoleReady: "能力投影已就绪",
    modelEyebrow: "ONE PACKAGE · TWO PLANES",
    modelTitle: "一个不可变身份，同时承载原生程序与认知表面",
    modelBody:
      "传统包管理器在文件落盘后结束。目标平台还会验证包声明的 Tool、MCP、Skill、UI 与 OKF 知识，并将可用证据投影给宿主。",
    nativePlane: "NATIVE PLANE",
    nativeTitle: "平台原生执行",
    nativeBody: "目标相关的可执行文件、运行时资产、原生 argv 与标准进程状态。",
    cognitivePlane: "COGNITIVE PLANE",
    cognitiveTitle: "Agent 可发现能力",
    cognitiveBody:
      "内容绑定的指令、工具依赖、MCP 服务、沙箱 UI 与可共享 OKF 知识，不从文本获得额外权限。",
    surfaceHint: "选择一个表面查看它的运行边界",
    surfaces: {
      tool: {
        label: "Tool",
        kind: "TASK / SERVICE",
        title: "保留原生 CLI 或 HTTP 合约",
        body: "Tool 是由 Runtime 管理的工作负载，不是私有 action 协议，也不是 MCP tools/list 项。",
        evidence: ["provider evidence", "exact generation", "bounded I/O"],
      },
      mcp: {
        label: "MCP",
        kind: "STDIO / STREAMABLE HTTP",
        title: "使用标准 MCP 传输",
        body: "stdio 会话受监督；Streamable HTTP 运行在私有 Runtime Service 后，并在发布前完成协议探测。",
        evidence: ["standard protocol", "health probe", "scoped binding"],
      },
      skill: {
        label: "Skill",
        kind: "CONTENT-BOUND",
        title: "指令依赖真实可用能力",
        body: "Skill 与包内容摘要绑定，并声明所需 Tool 与 MCP；依赖未就绪时不会进入能力快照。",
        evidence: [
          "content digest",
          "dependency closure",
          "managed projection",
        ],
      },
      ui: {
        label: "UI",
        kind: "SANDBOXED STATIC",
        title: "静态界面不等于 Runtime 工作负载",
        body: "HTML、CSS 与 JavaScript 由 A3S Code/Web 沙箱渲染，只能访问包声明并获授权的后端绑定。",
        evidence: ["integrity bound", "declared backend", "host sandbox"],
      },
      okf: {
        label: "OKF",
        kind: "OPEN KNOWLEDGE FORMAT · CONTRACT",
        title: "可共享、可索引的知识包",
        body: "OKF v0.2 用带 YAML frontmatter 的交叉链接 Markdown 表达概念，并兼容 v0.1。a3s-use-core 已冻结精确内容摘要与有界 conformance；manifest/lifecycle 仍 fail-closed，A3S Knowledge 原子索引待接入。",
        evidence: [
          "content digest",
          "bounded conformance",
          "atomic index target",
        ],
      },
    },
    lifecycleEyebrow: "TRUSTED LIFECYCLE",
    lifecycleTitle: "先证明，再发布能力",
    lifecycleBody:
      "搜索只读取已签名元数据；真正变更之前，目标、权限、提供者和影响都被绑定进不可变计划。",
    lifecycle: [
      {
        number: "01",
        title: "发现",
        body: "刷新并搜索 TUF 签名目录，不下载包体。",
      },
      {
        number: "02",
        title: "计划",
        body: "固定包摘要、表面、权限与 Runtime 证据。",
      },
      {
        number: "03",
        title: "授权",
        body: "ACL 策略和用户确认绑定同一个计划摘要。",
      },
      {
        number: "04",
        title: "暂存",
        body: "在有界目录中验证归档、ACL 清单与内容。",
      },
      {
        number: "05",
        title: "协调",
        body: "准备授权、Runtime 绑定、投影与依赖闭包。",
      },
      {
        number: "06",
        title: "发布",
        body: "原子切换能力代际，并排空旧代际调用。",
      },
    ],
    architectureEyebrow: "CLEAR OWNERSHIP",
    architectureTitle: "一个 Manager，一套生命周期事实",
    architectureBody:
      "CLI、Web 与 Agent 管理 MCP 共用同一个 Plugin Manager。A3S Use 管包与证据；Runtime 管工作负载；宿主管策略、凭据和渲染。",
    source: "包来源",
    manager: "共享 Plugin Manager",
    managerBody: "目录 · 策略 · 确认 · plan/apply · replay",
    engine: "A3S Use 包引擎",
    engineBody: "verify · stage · receipt · grant · reconcile",
    planes: "原生 + 认知表面",
    planesBody: "Tool · MCP · Skill · UI · OKF",
    hosts: "A3S 宿主",
    hostsBody: "A3S Code · Web · Knowledge · agents",
    architectureLink: "阅读架构说明",
    trustEyebrow: "SECURE BY EVIDENCE",
    trustTitle: "包内容不能给自己授权",
    trustBody:
      "Skill 文本、UI 消息、OKF 知识、工具输出和远端内容都被视为数据。权限只能来自宿主策略、明确授权与代际绑定的收据。",
    trustCards: [
      {
        title: "可验证供应链",
        body: "固定 TUF 根、签名元数据、长度与 SHA-256，并拒绝回滚和过期状态。",
        tag: "PROVENANCE",
      },
      {
        title: "默认拒绝漂移",
        body: "应用前重新解析；版本、内容、权限或提供者变化都会要求重新审查。",
        tag: "PLAN DIGEST",
      },
      {
        title: "精确代际授权",
        body: "Grant、Runtime binding、route lease 与能力快照都绑定同一包代际。",
        tag: "NO AMBIENT AUTHORITY",
      },
    ],
    platformEyebrow: "CROSS-PLATFORM",
    platformTitle: "一个包模型，覆盖三类桌面平台",
    platformBody:
      "macOS 与 Linux 已覆盖完整发布包和包生命周期；Windows x86_64 当前为 Preview，并持续补齐运行时与插件生命周期门禁。",
    supported: "SUPPORTED",
    preview: "PREVIEW",
    ctaEyebrow: "START WITH THE FOUNDATION",
    ctaTitle: "先检查本机能力，再构建认知包",
    ctaBody:
      "安装 v0.2 发布版，运行 doctor 和 capability snapshot；认知插件开发请以 schema-v3 合约与路线图为准。",
    ctaPrimary: "打开快速开始",
    ctaSecondary: "查看路线图",
    footer: "MIT 开源 · Rust 编写 · Linux / macOS / Windows",
  },
  en: {
    eyebrow: "OPEN SOURCE · AI NATIVE PACKAGE MANAGER",
    titleLead: "Packages for software.",
    titleAccent: "Capabilities for agents.",
    subtitle:
      "A3S Use gives native tools and A3S cognitive plugins one verifiable package lifecycle across Linux, macOS, and Windows.",
    getStarted: "Get started",
    github: "View on GitHub",
    statusLabel: "Implementation status",
    available: "Available",
    building: "In development",
    foundationLabel: "v0.2 package foundation",
    platformLabel: "Cognitive plugin platform",
    installLabel: "Install the verified release",
    installHint: "Install the complete release through the A3S CLI",
    copy: "Copy",
    copied: "Copied",
    consoleLabel: "PACKAGE INSPECTION",
    consolePackage: "package",
    consoleTarget: "target",
    consoleTrust: "trust",
    consoleGeneration: "generation",
    consoleReady: "capability projection ready",
    modelEyebrow: "ONE PACKAGE · TWO PLANES",
    modelTitle: "One immutable identity for native programs and cognition",
    modelBody:
      "Traditional package managers stop after placing files. The target platform also verifies declared Tool, MCP, Skill, UI, and OKF knowledge surfaces, then projects readiness evidence to hosts.",
    nativePlane: "NATIVE PLANE",
    nativeTitle: "Platform-native execution",
    nativeBody:
      "Target-specific executables, runtime assets, native argv, and standard process status.",
    cognitivePlane: "COGNITIVE PLANE",
    cognitiveTitle: "Agent-discoverable capabilities",
    cognitiveBody:
      "Content-bound instructions, tool dependencies, MCP services, sandboxed UI, and shareable OKF knowledge with no authority derived from text.",
    surfaceHint: "Select a surface to inspect its execution boundary",
    surfaces: {
      tool: {
        label: "Tool",
        kind: "TASK / SERVICE",
        title: "Keep the native CLI or HTTP contract",
        body: "A Tool is a Runtime-managed workload—not a private action protocol and not an MCP tools/list item.",
        evidence: ["provider evidence", "exact generation", "bounded I/O"],
      },
      mcp: {
        label: "MCP",
        kind: "STDIO / STREAMABLE HTTP",
        title: "Use standard MCP transports",
        body: "stdio sessions are supervised. Streamable HTTP runs behind a private Runtime Service and passes a protocol probe before publication.",
        evidence: ["standard protocol", "health probe", "scoped binding"],
      },
      skill: {
        label: "Skill",
        kind: "CONTENT-BOUND",
        title: "Instructions depend on real capabilities",
        body: "A Skill is bound to package content and declares required Tools and MCP. It stays out of the capability snapshot until dependencies are ready.",
        evidence: [
          "content digest",
          "dependency closure",
          "managed projection",
        ],
      },
      ui: {
        label: "UI",
        kind: "SANDBOXED STATIC",
        title: "Static UI is not a Runtime workload",
        body: "A3S Code/Web renders HTML, CSS, and JavaScript in a sandbox with access only to declared and authorized backend bindings.",
        evidence: ["integrity bound", "declared backend", "host sandbox"],
      },
      okf: {
        label: "OKF",
        kind: "OPEN KNOWLEDGE FORMAT · CONTRACT",
        title: "Shareable, indexable knowledge packages",
        body: "OKF v0.2 represents concepts as cross-linked Markdown with YAML frontmatter and preserves v0.1 compatibility. a3s-use-core now freezes exact content identity and bounded conformance; manifest/lifecycle acceptance remains fail-closed while A3S Knowledge indexing is pending.",
        evidence: [
          "content digest",
          "bounded conformance",
          "atomic index target",
        ],
      },
    },
    lifecycleEyebrow: "TRUSTED LIFECYCLE",
    lifecycleTitle: "Prove readiness before publishing capability",
    lifecycleBody:
      "Search reads signed metadata only. Before mutation, target, permissions, provider, and impact are bound into one immutable plan.",
    lifecycle: [
      {
        number: "01",
        title: "Discover",
        body: "Refresh and search a TUF-signed catalog without package payloads.",
      },
      {
        number: "02",
        title: "Plan",
        body: "Bind package digests, surfaces, permissions, and Runtime evidence.",
      },
      {
        number: "03",
        title: "Authorize",
        body: "Bind ACL policy and user confirmation to the same plan digest.",
      },
      {
        number: "04",
        title: "Stage",
        body: "Verify the archive, ACL manifest, and content in a bounded root.",
      },
      {
        number: "05",
        title: "Reconcile",
        body: "Prepare grants, Runtime bindings, projections, and dependency closure.",
      },
      {
        number: "06",
        title: "Publish",
        body: "Switch capability generation atomically, then drain the old one.",
      },
    ],
    architectureEyebrow: "CLEAR OWNERSHIP",
    architectureTitle: "One Manager, one lifecycle truth",
    architectureBody:
      "CLI, Web, and agent management MCP share one Plugin Manager. A3S Use owns packages and evidence; Runtime owns workloads; hosts own policy, credentials, and rendering.",
    source: "Package sources",
    manager: "Shared Plugin Manager",
    managerBody: "catalog · policy · confirmation · plan/apply · replay",
    engine: "A3S Use package engine",
    engineBody: "verify · stage · receipt · grant · reconcile",
    planes: "Native + cognitive surfaces",
    planesBody: "Tool · MCP · Skill · UI · OKF",
    hosts: "A3S hosts",
    hostsBody: "A3S Code · Web · Knowledge · agents",
    architectureLink: "Read the architecture guide",
    trustEyebrow: "SECURE BY EVIDENCE",
    trustTitle: "Package content cannot authorize itself",
    trustBody:
      "Skill text, UI messages, OKF knowledge, tool output, and remote content are data. Authority comes only from host policy, explicit grants, and generation-bound receipts.",
    trustCards: [
      {
        title: "Verifiable supply chain",
        body: "Pin TUF roots, signed metadata, length, and SHA-256 while rejecting rollback and expired state.",
        tag: "PROVENANCE",
      },
      {
        title: "Fail closed on drift",
        body: "Resolve again before apply. Version, content, permission, or provider changes require a new review.",
        tag: "PLAN DIGEST",
      },
      {
        title: "Exact-generation authority",
        body: "Grants, Runtime bindings, route leases, and capability snapshots bind to one package generation.",
        tag: "NO AMBIENT AUTHORITY",
      },
    ],
    platformEyebrow: "CROSS-PLATFORM",
    platformTitle: "One package model across three desktop families",
    platformBody:
      "macOS and Linux cover complete release archives and package lifecycle. Windows x86_64 is currently Preview while runtime and plugin lifecycle gates are completed.",
    supported: "SUPPORTED",
    preview: "PREVIEW",
    ctaEyebrow: "START WITH THE FOUNDATION",
    ctaTitle: "Inspect local capabilities. Build a cognitive package.",
    ctaBody:
      "Install the v0.2 release and run doctor plus capability snapshot. Use the schema-v3 contracts and roadmap for cognitive plugin development.",
    ctaPrimary: "Open the quick start",
    ctaSecondary: "Read the roadmap",
    footer: "MIT licensed · Built in Rust · Linux / macOS / Windows",
  },
};
