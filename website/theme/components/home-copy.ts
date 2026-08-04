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
  titleLead: string;
  titleAccent: string;
  subtitle: string;
  heroImageAlt: string;
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
  copying: string;
  copied: string;
  copyFailed: string;
  modelTitle: string;
  modelBody: string;
  nativeTitle: string;
  nativeBody: string;
  cognitiveTitle: string;
  cognitiveBody: string;
  surfaceHint: string;
  surfaces: Record<SurfaceKey, SurfaceCopy>;
  lifecycleTitle: string;
  lifecycleBody: string;
  lifecycle: Array<{ number: string; title: string; body: string }>;
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
  trustTitle: string;
  trustBody: string;
  trustImageAlt: string;
  trustCards: Array<{ title: string; body: string }>;
  platformTitle: string;
  platformBody: string;
  supported: string;
  preview: string;
  ctaTitle: string;
  ctaBody: string;
  ctaSecondary: string;
  footer: string;
};

export const homeCopy: Record<Locale, HomeCopy> = {
  zh: {
    titleLead: "为软件安装包。",
    titleAccent: "为 Agent 安装能力。",
    subtitle:
      "A3S Use 为 Linux、macOS 与 Windows 上的原生工具和认知插件提供统一、可验证的包生命周期。",
    heroImageAlt: "六个石墨色模块由一条琥珀色验证轨道锁定为完整软件包。",
    getStarted: "开始使用",
    github: "查看 GitHub",
    statusLabel: "版本与实现状态",
    available: "main 已可用",
    building: "开发中",
    foundationLabel: "v0.3 认知包依赖图",
    platformLabel: "认知插件平台",
    installLabel: "安装 A3S Use",
    installHint: "稳定发布版，v0.3 认知包图已进入 main",
    copy: "复制",
    copying: "正在复制",
    copied: "已复制",
    copyFailed: "复制失败",
    modelTitle: "一个不可变身份，一套安装与移除边界",
    modelBody:
      "Tool、MCP、OKF、Flow、Skill 与 UI 是同一包拥有的贡献，不是可独立安装的包。Use 只向宿主投影依赖已就绪的同代际证据。",
    nativeTitle: "平台原生执行",
    nativeBody: "目标相关的可执行文件、运行时资产、原生 argv 与标准进程状态。",
    cognitiveTitle: "Agent 可发现能力",
    cognitiveBody:
      "内容绑定的工作流与指令、工具依赖、MCP 服务、沙箱 UI 与 OKF 知识，不从文本获得额外权限。",
    surfaceHint: "选择一个表面查看运行边界",
    surfaces: {
      tool: {
        label: "Tool",
        kind: "TASK / SERVICE",
        title: "保留原生 CLI 或 HTTP 合约",
        body: "Tool 是 Runtime 管理的工作负载，不是私有 action 协议，也不是 MCP tools/list 项。",
        evidence: ["provider evidence", "exact generation", "bounded I/O"],
      },
      mcp: {
        label: "MCP",
        kind: "STDIO / STREAMABLE HTTP",
        title: "使用标准 MCP 传输",
        body: "stdio 会话受监督。Streamable HTTP 位于私有 Runtime Service 后，并在发布前完成协议探测。",
        evidence: ["standard protocol", "health probe", "scoped binding"],
      },
      flow: {
        label: "Flow",
        kind: "A3S FLOW / NATIVE TYPESCRIPT",
        title: "一个工作流引擎，多种宿主目标",
        body: "Flow 固定使用 a3s-flow 引擎，并显式依赖 Tool、MCP 与 OKF。native-ts 是执行适配器，flow.json 是同一身份的设计与部署文档。",
        evidence: ["source digest", "compiled artifact", "typed live catalog"],
      },
      skill: {
        label: "Skill",
        kind: "CONTENT-BOUND",
        title: "指令依赖真实可用能力",
        body: "Skill 与包内容摘要绑定，并声明所需 Flow、Tool、MCP 与 OKF。依赖未就绪时不会进入能力快照。",
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
        body: "A3S Code/Web 在沙箱中渲染 HTML、CSS 与 JavaScript，只访问已声明且获授权的后端绑定。",
        evidence: ["integrity bound", "declared backend", "host sandbox"],
      },
      okf: {
        label: "OKF",
        kind: "OPEN KNOWLEDGE FORMAT / NON-EXECUTABLE",
        title: "可共享、可索引的知识包",
        body: "OKF v0.2 用带 YAML frontmatter 的交叉链接 Markdown 表达概念。生命周期适配器已支持精确代际的 stage、promote、hide 与 receipt-owned remove，生产 A3S Knowledge 后端仍待接入。",
        evidence: [
          "content digest",
          "bounded conformance",
          "promoted observation",
        ],
      },
    },
    lifecycleTitle: "正向准备，一次发布，反向移除",
    lifecycleBody:
      "跨宿主变更之前，一份持久包日志会绑定已审查计划、精确代际、六表面依赖图和幂等检查点。",
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
        body: "一次发布，或先隐藏、排空，再反向移除 receipt-owned 资源。",
      },
    ],
    architectureTitle: "一个 Manager，一套生命周期事实",
    architectureBody:
      "CLI、Web 与 Agent 管理 MCP 共用同一个 Plugin Manager。Use 管包与证据，Runtime 管工作负载，宿主管策略、凭据和渲染。",
    source: "包来源",
    manager: "共享 Plugin Manager",
    managerBody: "目录 / 策略 / 确认 / plan and apply / replay",
    engine: "A3S Use 包引擎",
    engineBody: "verify / journal / prepare / publish / drain",
    planes: "原生与认知表面",
    planesBody: "Tool / MCP / OKF / Flow / Skill / UI",
    hosts: "A3S 宿主",
    hostsBody: "A3S Code / Web / Knowledge / agents",
    architectureLink: "阅读架构说明",
    trustTitle: "包内容不能给自己授权",
    trustBody:
      "Flow、Skill、UI、OKF、工具输出与远端内容都只是数据。权限只来自宿主策略、明确授权与代际绑定的收据。",
    trustImageAlt:
      "石墨色软件包模块拆分为可检查的层，并由琥珀色防篡改结构贯穿。",
    trustCards: [
      {
        title: "可验证供应链",
        body: "固定 TUF 根、签名元数据、长度与 SHA-256，并拒绝回滚和过期状态。",
      },
      {
        title: "默认拒绝漂移",
        body: "应用前重新解析。版本、内容、权限或提供者变化都会要求重新审查。",
      },
      {
        title: "精确代际授权",
        body: "Grant、Runtime binding、route lease 与能力快照绑定同一包代际。",
      },
    ],
    platformTitle: "一个模型，覆盖三类桌面平台",
    platformBody:
      "macOS 与 Linux 已覆盖完整发布包和生命周期。Windows x86_64 当前为 Preview，并持续补齐运行时与插件生命周期门禁。",
    supported: "支持",
    preview: "预览",
    ctaTitle: "一次安装认知包及其完整依赖",
    ctaBody:
      "用 a3s plugin 安装后，Code CLI/TUI/Web 会热插拔已验证的 Tool、MCP、Flow、Skill 与 UI，并共用精确 flow.json 身份和本地持久运行历史。生产 Runtime Service、HTTP MCP、OKF 与分布式 Flow 调度仍是发布门禁。",
    ctaSecondary: "查看路线图",
    footer: "MIT 开源，Rust 构建，支持 Linux / macOS / Windows",
  },
  en: {
    titleLead: "Packages for software.",
    titleAccent: "Capabilities for agents.",
    subtitle:
      "A3S Use gives native tools and cognitive plugins one verifiable lifecycle across Linux, macOS, and Windows.",
    heroImageAlt:
      "Six graphite modules locked into one software package by an amber verification rail.",
    getStarted: "Get started",
    github: "View on GitHub",
    statusLabel: "Release and implementation status",
    available: "Available on main",
    building: "In development",
    foundationLabel: "v0.3 cognitive package graph",
    platformLabel: "Cognitive plugin platform",
    installLabel: "Install A3S Use",
    installHint: "Stable release, with the v0.3 cognitive graph on main",
    copy: "Copy",
    copying: "Copying",
    copied: "Copied",
    copyFailed: "Copy failed",
    modelTitle: "One immutable identity. One install and removal boundary.",
    modelBody:
      "Tool, MCP, OKF, Flow, Skill, and UI are contributions owned by one package, not independently installed packages. Use projects only same-generation evidence with ready dependencies.",
    nativeTitle: "Platform-native execution",
    nativeBody:
      "Target-specific executables, runtime assets, native argv, and standard process status.",
    cognitiveTitle: "Agent-discoverable capabilities",
    cognitiveBody:
      "Content-bound workflows and instructions, tool dependencies, MCP services, sandboxed UI, and OKF knowledge with no authority derived from text.",
    surfaceHint: "Choose a surface to inspect its execution boundary",
    surfaces: {
      tool: {
        label: "Tool",
        kind: "TASK / SERVICE",
        title: "Keep the native CLI or HTTP contract",
        body: "A Tool is a Runtime-managed workload. It is not a private action protocol or an MCP tools/list item.",
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
        kind: "A3S FLOW / NATIVE TYPESCRIPT",
        title: "One workflow engine across host targets",
        body: "Flow always uses the a3s-flow engine with explicit Tool, MCP, and OKF dependencies. native-ts is an execution adapter, while flow.json documents design and deployment for the same identity.",
        evidence: ["source digest", "compiled artifact", "typed live catalog"],
      },
      skill: {
        label: "Skill",
        kind: "CONTENT-BOUND",
        title: "Instructions depend on real capabilities",
        body: "A Skill binds to package content and declares required Flow, Tool, MCP, and OKF surfaces. It stays out of snapshots until dependencies are ready.",
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
        kind: "OPEN KNOWLEDGE FORMAT / NON-EXECUTABLE",
        title: "Shareable, indexable knowledge packages",
        body: "OKF v0.2 represents concepts as cross-linked Markdown with YAML frontmatter. Its lifecycle adapter stages, promotes, hides, and receipt-removes exact generations, while the production A3S Knowledge backend remains pending.",
        evidence: [
          "content digest",
          "bounded conformance",
          "promoted observation",
        ],
      },
    },
    lifecycleTitle: "Prepare forward. Publish once. Remove in reverse.",
    lifecycleBody:
      "Before multi-host mutation, one durable package journal binds the reviewed plan, exact generation, six-surface dependency graph, and idempotent checkpoints.",
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
    architectureTitle: "One Manager, one lifecycle truth",
    architectureBody:
      "CLI, Web, and agent management MCP share one Plugin Manager. Use owns packages and evidence, Runtime owns workloads, and hosts own policy, credentials, and rendering.",
    source: "Package sources",
    manager: "Shared Plugin Manager",
    managerBody: "catalog / policy / confirmation / plan and apply / replay",
    engine: "A3S Use package engine",
    engineBody: "verify / journal / prepare / publish / drain",
    planes: "Native and cognitive surfaces",
    planesBody: "Tool / MCP / OKF / Flow / Skill / UI",
    hosts: "A3S hosts",
    hostsBody: "A3S Code / Web / Knowledge / agents",
    architectureLink: "Read the architecture guide",
    trustTitle: "Package content cannot authorize itself",
    trustBody:
      "Flow, Skill, UI, OKF, tool output, and remote content are data. Authority comes only from host policy, explicit grants, and generation-bound receipts.",
    trustImageAlt:
      "A graphite package module separated into inspectable layers and threaded by an amber tamper-evident mechanism.",
    trustCards: [
      {
        title: "Verifiable supply chain",
        body: "Pin TUF roots, signed metadata, length, and SHA-256 while rejecting rollback and expired state.",
      },
      {
        title: "Fail closed on drift",
        body: "Resolve again before apply. Version, content, permission, or provider changes require a new review.",
      },
      {
        title: "Exact-generation authority",
        body: "Grants, Runtime bindings, route leases, and capability snapshots bind to one package generation.",
      },
    ],
    platformTitle: "One model across three desktop families",
    platformBody:
      "macOS and Linux cover complete release archives and lifecycle. Windows x86_64 is currently Preview while runtime and plugin lifecycle gates are completed.",
    supported: "Supported",
    preview: "Preview",
    ctaTitle: "Install a cognitive package and its complete dependency graph.",
    ctaBody:
      "Install with a3s plugin and Code CLI/TUI/Web hot-plugs verified Tool, MCP, Flow, Skill, and UI surfaces with one exact flow.json identity and durable local history. Production Runtime Service, HTTP MCP, OKF, and distributed Flow scheduling remain release gates.",
    ctaSecondary: "View roadmap",
    footer: "MIT licensed, built in Rust, available on Linux / macOS / Windows",
  },
};
