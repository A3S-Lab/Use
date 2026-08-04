export type Locale = "zh" | "en";
export type SurfaceKey = "tool" | "mcp" | "okf" | "flow" | "skill" | "ui";

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
    available: "main 已可用",
    building: "开发中",
    foundationLabel: "v0.3 认知包依赖图",
    platformLabel: "认知插件平台",
    installLabel: "安装 A3S Use",
    installHint: "稳定发布版；v0.3 认知包图已进入 main",
    copy: "复制",
    copied: "已复制",
    consoleLabel: "PACKAGE INSPECTION",
    consolePackage: "包",
    consoleTarget: "目标",
    consoleTrust: "信任",
    consoleGeneration: "代际",
    consoleReady: "Code Flow 本地持久运行已就绪 · 分布式宿主集成中",
    modelEyebrow: "ONE PACKAGE · SIX CONTRIBUTIONS",
    modelTitle: "一个不可变身份，一套安装与移除边界",
    modelBody:
      "Tool、MCP、OKF、Flow、Skill 与 UI 是包拥有的贡献，不是可独立安装的包。A3S Use 验证它们共享的代际，只向宿主投影依赖已就绪的证据。",
    nativePlane: "NATIVE PLANE",
    nativeTitle: "平台原生执行",
    nativeBody: "目标相关的可执行文件、运行时资产、原生 argv 与标准进程状态。",
    cognitivePlane: "COGNITIVE PLANE",
    cognitiveTitle: "Agent 可发现能力",
    cognitiveBody:
      "内容绑定的工作流与指令、工具依赖、MCP 服务、沙箱 UI 与可共享 OKF 知识，不从文本获得额外权限。",
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
      flow: {
        label: "Flow",
        kind: "A3S FLOW · NATIVE TYPESCRIPT",
        title: "一个工作流引擎，多种宿主目标",
        body: "Flow 固定使用 a3s-flow 引擎，并显式依赖 Tool、MCP 与 OKF。native-ts 是执行适配器；flow.json 是同一身份的设计/部署文档。",
        evidence: ["source digest", "compiled artifact", "typed live catalog"],
      },
      skill: {
        label: "Skill",
        kind: "CONTENT-BOUND",
        title: "指令依赖真实可用能力",
        body: "Skill 与包内容摘要绑定，并声明所需 Flow、Tool、MCP 与 OKF；依赖未就绪时不会进入能力快照。",
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
        kind: "OPEN KNOWLEDGE FORMAT · NON-EXECUTABLE",
        title: "可共享、可索引的知识包",
        body: "OKF v0.2 用带 YAML frontmatter 的交叉链接 Markdown 表达概念。生命周期适配器已支持精确代际的 stage、promote、hide 与 receipt-owned remove；生产 A3S Knowledge 后端仍待接入。",
        evidence: [
          "content digest",
          "bounded conformance",
          "promoted observation",
        ],
      },
    },
    lifecycleEyebrow: "TRUSTED LIFECYCLE",
    lifecycleTitle: "正向准备，一次发布，反向移除",
    lifecycleBody:
      "在跨宿主变更前，一个持久包日志会绑定已审查计划、精确代际、六表面依赖图和幂等检查点。",
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
        title: "准备",
        body: "按依赖顺序准备 Runtime、Knowledge、A3S Flow、Skill 与 UI 宿主。",
      },
      {
        number: "06",
        title: "发布 / 移除",
        body: "一次发布；或先隐藏、排空，再反向移除 receipt-owned 资源。",
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
    engineBody: "verify · journal · prepare · publish · drain",
    planes: "原生 + 认知表面",
    planesBody: "Tool · MCP · OKF · Flow · Skill · UI",
    hosts: "A3S 宿主",
    hostsBody: "A3S Code · Web · Knowledge · agents",
    architectureLink: "阅读架构说明",
    trustEyebrow: "SECURE BY EVIDENCE",
    trustTitle: "包内容不能给自己授权",
    trustBody:
      "Flow/Skill 源码、UI 消息、OKF 知识、工具输出和远端内容都被视为数据。权限只能来自宿主策略、明确授权与代际绑定的收据。",
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
    ctaEyebrow: "START WITH THE PACKAGE GRAPH",
    ctaTitle: "一次安装认知包及其完整依赖",
    ctaBody:
      "用 a3s plugin 安装后，Code CLI/TUI/Web 会热插拔已验证的 Tool、MCP、Flow、Skill 与 UI，并共用精确 flow.json 身份和本地持久运行历史。生产 Runtime Service、HTTP MCP、OKF 与分布式 Flow 调度仍是发布门禁。",
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
    available: "Available on main",
    building: "In development",
    foundationLabel: "v0.3 cognitive package graph",
    platformLabel: "Cognitive plugin platform",
    installLabel: "Install A3S Use",
    installHint: "Stable release; the v0.3 cognitive graph is on main",
    copy: "Copy",
    copied: "Copied",
    consoleLabel: "PACKAGE INSPECTION",
    consolePackage: "package",
    consoleTarget: "target",
    consoleTrust: "trust",
    consoleGeneration: "generation",
    consoleReady:
      "Code Flow local durability ready · distributed hosts in progress",
    modelEyebrow: "ONE PACKAGE · SIX CONTRIBUTIONS",
    modelTitle: "One immutable identity. One install and removal boundary.",
    modelBody:
      "Tool, MCP, OKF, Flow, Skill, and UI are package-owned contributions—not independently installed packages. A3S Use verifies their shared generation and projects only dependency-ready evidence to hosts.",
    nativePlane: "NATIVE PLANE",
    nativeTitle: "Platform-native execution",
    nativeBody:
      "Target-specific executables, runtime assets, native argv, and standard process status.",
    cognitivePlane: "COGNITIVE PLANE",
    cognitiveTitle: "Agent-discoverable capabilities",
    cognitiveBody:
      "Content-bound workflows and instructions, tool dependencies, MCP services, sandboxed UI, and shareable OKF knowledge with no authority derived from text.",
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
      flow: {
        label: "Flow",
        kind: "A3S FLOW · NATIVE TYPESCRIPT",
        title: "One workflow engine across host targets",
        body: "Flow always uses the a3s-flow engine with explicit Tool, MCP, and OKF dependencies. native-ts is an execution adapter; flow.json is a design/deployment document for the same identity.",
        evidence: ["source digest", "compiled artifact", "typed live catalog"],
      },
      skill: {
        label: "Skill",
        kind: "CONTENT-BOUND",
        title: "Instructions depend on real capabilities",
        body: "A Skill is bound to package content and declares required Flow, Tool, MCP, and OKF surfaces. It stays out of the capability snapshot until dependencies are ready.",
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
        kind: "OPEN KNOWLEDGE FORMAT · NON-EXECUTABLE",
        title: "Shareable, indexable knowledge packages",
        body: "OKF v0.2 represents concepts as cross-linked Markdown with YAML frontmatter. The lifecycle adapter stages, promotes, hides, and receipt-removes exact generations; the production A3S Knowledge backend remains pending.",
        evidence: [
          "content digest",
          "bounded conformance",
          "promoted observation",
        ],
      },
    },
    lifecycleEyebrow: "TRUSTED LIFECYCLE",
    lifecycleTitle: "Prepare forward. Publish once. Remove in reverse.",
    lifecycleBody:
      "One durable package journal binds the reviewed plan, exact generation, six-surface dependency graph, and idempotent checkpoints before multi-host mutation.",
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
        title: "Prepare",
        body: "Prepare Runtime, Knowledge, A3S Flow, Skill, and UI hosts in dependency order.",
      },
      {
        number: "06",
        title: "Publish / remove",
        body: "Publish once, or hide and drain before reverse receipt-owned removal.",
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
    engineBody: "verify · journal · prepare · publish · drain",
    planes: "Native + cognitive surfaces",
    planesBody: "Tool · MCP · OKF · Flow · Skill · UI",
    hosts: "A3S hosts",
    hostsBody: "A3S Code · Web · Knowledge · agents",
    architectureLink: "Read the architecture guide",
    trustEyebrow: "SECURE BY EVIDENCE",
    trustTitle: "Package content cannot authorize itself",
    trustBody:
      "Flow and Skill source, UI messages, OKF knowledge, tool output, and remote content are data. Authority comes only from host policy, explicit grants, and generation-bound receipts.",
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
    ctaEyebrow: "START WITH THE PACKAGE GRAPH",
    ctaTitle: "Install a cognitive package and its complete dependency graph.",
    ctaBody:
      "Install with a3s plugin and Code CLI/TUI/Web hot-plugs verified Tool, MCP, Flow, Skill, and UI surfaces with one exact flow.json identity and durable local history. Production Runtime Service, HTTP MCP, OKF, and distributed Flow scheduling remain release gates.",
    ctaPrimary: "Open the quick start",
    ctaSecondary: "Read the roadmap",
    footer: "MIT licensed · Built in Rust · Linux / macOS / Windows",
  },
};
