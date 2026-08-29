export type Locale = "zh" | "en";
export type SurfaceKey = "tool" | "mcp" | "okf" | "flow" | "skill" | "ui";
export type InstallerKey = "unix" | "windows";

type SurfaceCopy = {
  label: string;
  kind: string;
  title: string;
  body: string;
  evidence: string[];
};

type AssuranceCopy = {
  title: string;
  body: string;
};

type InstallerCopy = {
  kicker: string;
  title: string;
  body: string;
  platformSelector: string;
  unix: string;
  windows: string;
  atomicInstall: string;
  noPathMutation: string;
  cliTitle: string;
  cliBody: string;
  previewTitle: string;
  previewBody: string;
};

export type HomeCopy = {
  heroKicker: string;
  titleLead: string;
  titleAccent: string;
  subtitle: string;
  previewNotice: string;
  exploreModel: string;
  github: string;
  assuranceLabel: string;
  assurances: AssuranceCopy[];
  installer: InstallerCopy;
  copy: string;
  copying: string;
  copied: string;
  copyFailed: string;
  modelKicker: string;
  modelTitle: string;
  modelBody: string;
  nativeTitle: string;
  nativeBody: string;
  cognitiveTitle: string;
  cognitiveBody: string;
  surfaceHint: string;
  surfaces: Record<SurfaceKey, SurfaceCopy>;
  lifecycleKicker: string;
  lifecycleTitle: string;
  lifecycleBody: string;
  lifecycle: Array<{ number: string; title: string; body: string }>;
  architectureKicker: string;
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
  trustKicker: string;
  trustTitle: string;
  trustBody: string;
  trustLedger: string;
  trustVerified: string;
  trustCards: Array<{ title: string; body: string }>;
  platformKicker: string;
  platformTitle: string;
  platformBody: string;
  fullGate: string;
  previewGate: string;
  ctaKicker: string;
  ctaTitle: string;
  ctaBody: string;
  ctaSecondary: string;
  footer: string;
};

export const homeCopy: Record<Locale, HomeCopy> = {
  zh: {
    heroKicker: "A3S USE · DEVELOPMENT PREVIEW",
    titleLead: "AI Native",
    titleAccent: "包管理器",
    subtitle:
      "用一张精确依赖图安装原生工具与认知能力。每次变更先审查，再一次切换到新的能力代际。",
    previewNotice:
      "当前是开发预览，协议与持久化状态仍可能变化，不用于生产环境。",
    exploreModel: "查看包模型",
    github: "查看 GitHub",
    assuranceLabel: "A3S Use 的三条生命周期保证",
    assurances: [
      {
        title: "精确包图",
        body: "依赖按精确版本锁定，正向安装、反向回收。",
      },
      {
        title: "审查后变更",
        body: "计划、权限与确认绑定同一个摘要。",
      },
      {
        title: "原子发布",
        body: "六类表面在同一能力代际中一起生效。",
      },
    ],
    installer: {
      kicker: "A3S CLI INSTALLER",
      title: "先把 a3s 装到系统里",
      body: "安装脚本从 A3S-Lab/CLI 的稳定 GitHub Release 识别当前平台，校验 SHA-256，再以用户权限原子替换二进制。",
      platformSelector: "选择 A3S CLI 安装平台",
      unix: "macOS / Linux",
      windows: "Windows",
      atomicInstall: "原子替换，可回滚",
      noPathMutation: "默认不修改 PATH",
      cliTitle: "安装稳定版 A3S CLI",
      cliBody:
        "脚本只从官方 Release 下载匹配的二进制和 Web 资产，不需要管理员权限。",
      previewTitle: "再安装 Use 预览组件",
      previewBody:
        "这一步安装的是开发预览，不代表 A3S Use 已达到生产发布门槛。",
    },
    copy: "复制命令",
    copying: "正在复制",
    copied: "已复制",
    copyFailed: "复制失败",
    modelKicker: "PACKAGE MODEL",
    modelTitle: "一个包图，六类能力表面",
    modelBody:
      "Tool、MCP、OKF、A3S Flow、Skill 与 UI 都归属于同一个包图。它们共享身份、依赖、授权和退役边界，不能各自绕过生命周期。",
    nativeTitle: "平台原生执行",
    nativeBody: "目标相关的可执行文件、运行时资产、原生 argv 与标准进程状态。",
    cognitiveTitle: "Agent 可发现能力",
    cognitiveBody:
      "工作流、知识、指令和界面与内容摘要绑定；文本本身不会获得权限。",
    surfaceHint: "选择一种能力表面查看它的运行边界",
    surfaces: {
      tool: {
        label: "Tool",
        kind: "TASK / SERVICE",
        title: "保留原生 CLI 或 HTTP 合约",
        body: "Tool 是由 Runtime 管理的工作负载，不会被改造成私有 action 协议或伪装成 MCP 工具。",
        evidence: ["provider evidence", "exact generation", "bounded I/O"],
      },
      mcp: {
        label: "MCP",
        kind: "STDIO / STREAMABLE HTTP",
        title: "只使用标准 MCP 传输",
        body: "stdio 会话受监督；Streamable HTTP 位于 Runtime Service 后，并在发布前通过协议与健康探测。",
        evidence: ["standard protocol", "health probe", "scoped binding"],
      },
      okf: {
        label: "OKF",
        kind: "KNOWLEDGE / NON-EXECUTABLE",
        title: "可共享、可引用的知识包",
        body: "OKF v0.2 使用交叉链接 Markdown 表达概念。内置 SQLite/FTS5 后端已支持精确代际的暂存、发布、搜索和移除。",
        evidence: ["content digest", "FTS5 citation", "promoted generation"],
      },
      flow: {
        label: "Flow",
        kind: "A3S FLOW / NATIVE TYPESCRIPT",
        title: "一个工作流引擎，多种宿主目标",
        body: "Flow 固定使用 a3s-flow 引擎，并显式声明 Tool、MCP 与 OKF 依赖。flow.json 记录同一身份的设计与部署信息。",
        evidence: ["source digest", "compiled artifact", "typed catalog"],
      },
      skill: {
        label: "Skill",
        kind: "CONTENT-BOUND",
        title: "指令依赖真实可用能力",
        body: "Skill 与包内容摘要绑定，并声明所需表面。依赖未就绪时，它不会进入能力快照。",
        evidence: [
          "content digest",
          "dependency closure",
          "managed projection",
        ],
      },
      ui: {
        label: "UI",
        kind: "SANDBOXED STATIC",
        title: "静态界面不是 Runtime 工作负载",
        body: "宿主只在沙箱中渲染完整性绑定的 HTML、CSS 与 JavaScript，并限制它访问已声明、已授权的后端。",
        evidence: ["integrity bound", "declared backend", "host sandbox"],
      },
    },
    lifecycleKicker: "REVIEWED LIFECYCLE",
    lifecycleTitle: "正向准备，一次发布，反向移除",
    lifecycleBody:
      "持久操作日志把已审查计划、精确包图、授权和幂等检查点放在同一条恢复路径上。",
    lifecycle: [
      { number: "01", title: "发现", body: "刷新签名目录，不下载包体。" },
      { number: "02", title: "解析", body: "冻结版本、依赖边和来源证据。" },
      { number: "03", title: "审查", body: "展示影响、权限和精确计划摘要。" },
      {
        number: "04",
        title: "暂存",
        body: "在有界目录验证归档、ACL 清单与内容。",
      },
      { number: "05", title: "准备", body: "按依赖顺序准备六类表面的宿主。" },
      {
        number: "06",
        title: "切换",
        body: "发布新代际；退役时先排空，再反向移除。",
      },
    ],
    architectureKicker: "OWNERSHIP",
    architectureTitle: "一个 Manager，一套生命周期事实",
    architectureBody:
      "CLI、TUI 与 Agent 管理 MCP 共用同一个 Plugin Manager。Use 管包和证据，Runtime 管工作负载，各宿主管策略、凭据与渲染。",
    source: "包来源",
    manager: "共享 Plugin Manager",
    managerBody: "catalog / policy / review / plan and apply / replay",
    engine: "A3S Use 包引擎",
    engineBody: "verify / journal / prepare / cutover / drain",
    planes: "六类能力表面",
    planesBody: "Tool / MCP / OKF / Flow / Skill / UI",
    hosts: "A3S 宿主",
    hostsBody: "A3S Code / OS / Knowledge / agents",
    architectureLink: "阅读架构说明",
    trustKicker: "TRUST BOUNDARY",
    trustTitle: "包内容不能给自己授权",
    trustBody:
      "Flow、Skill、UI、OKF、工具输出和远端内容都只是数据。权限只来自宿主策略、明确授权与代际绑定的收据。",
    trustLedger: "完整性账本",
    trustVerified: "全部通过",
    trustCards: [
      {
        title: "验证供应链",
        body: "固定 TUF 根、签名元数据、长度和 SHA-256，拒绝回滚与过期状态。",
      },
      {
        title: "漂移即停止",
        body: "应用前重新解析；版本、内容、权限或提供者变化都会要求重新审查。",
      },
      {
        title: "授权绑定代际",
        body: "Grant、Runtime binding、generation lease 与能力快照指向同一包代际。",
      },
    ],
    platformKicker: "PLATFORM GATES",
    platformTitle: "同一个模型，三类桌面平台",
    platformBody:
      "Linux 与 macOS 运行完整工作区门禁；Windows x86_64 目前是较窄的 Preview 编译与 facade 门禁。",
    fullGate: "完整门禁",
    previewGate: "预览门禁",
    ctaKicker: "START WITH EVIDENCE",
    ctaTitle: "从一份可审查的包图开始",
    ctaBody:
      "先验证当前实现和边界，再决定是否把开发预览接入你的 A3S 宿主。路线图会持续列出尚未完成的发布门槛。",
    ctaSecondary: "查看路线图",
    footer: "MIT 开源 · Rust 构建 · A3S-Lab/Use",
  },
  en: {
    heroKicker: "A3S USE · DEVELOPMENT PREVIEW",
    titleLead: "The AI Native",
    titleAccent: "package manager",
    subtitle:
      "Install native tools and cognitive capabilities as one exact dependency graph. Review every change, then cut over to the next capability generation once.",
    previewNotice:
      "Development preview: protocols and persisted state may still change. Do not use it in production.",
    exploreModel: "Explore the package model",
    github: "View on GitHub",
    assuranceLabel: "Three A3S Use lifecycle guarantees",
    assurances: [
      {
        title: "Exact package graph",
        body: "Lock exact versions, install forward, and retire in reverse.",
      },
      {
        title: "Reviewed mutation",
        body: "Bind the plan, grants, and confirmation to one digest.",
      },
      {
        title: "Atomic publication",
        body: "Publish all six surfaces in one capability generation.",
      },
    ],
    installer: {
      kicker: "A3S CLI INSTALLER",
      title: "Install a3s on your system first",
      body: "The installer detects the current platform, resolves the stable A3S-Lab/CLI GitHub Release, verifies SHA-256, and atomically replaces the user-scoped binary.",
      platformSelector: "Choose an A3S CLI installation platform",
      unix: "macOS / Linux",
      windows: "Windows",
      atomicInstall: "Atomic replacement with rollback",
      noPathMutation: "PATH unchanged by default",
      cliTitle: "Install the stable A3S CLI",
      cliBody:
        "The script downloads matching binaries and Web assets only from the official Release. Administrator access is not required.",
      previewTitle: "Then install the Use preview",
      previewBody:
        "This installs a development preview. It does not claim that A3S Use has passed its production release gates.",
    },
    copy: "Copy command",
    copying: "Copying",
    copied: "Copied",
    copyFailed: "Copy failed",
    modelKicker: "PACKAGE MODEL",
    modelTitle: "One package graph, six capability surfaces",
    modelBody:
      "Tool, MCP, OKF, A3S Flow, Skill, and UI belong to one package graph. They share identity, dependencies, grants, and retirement boundaries instead of bypassing lifecycle control independently.",
    nativeTitle: "Platform-native execution",
    nativeBody:
      "Target-specific executables, runtime assets, native argv, and standard process status.",
    cognitiveTitle: "Agent-discoverable capabilities",
    cognitiveBody:
      "Workflows, knowledge, instructions, and UI bind to content digests. Text never grants itself authority.",
    surfaceHint: "Choose a capability surface to inspect its runtime boundary",
    surfaces: {
      tool: {
        label: "Tool",
        kind: "TASK / SERVICE",
        title: "Keep the native CLI or HTTP contract",
        body: "A Tool is a Runtime-managed workload. It is not converted into a private action protocol or disguised as an MCP tool.",
        evidence: ["provider evidence", "exact generation", "bounded I/O"],
      },
      mcp: {
        label: "MCP",
        kind: "STDIO / STREAMABLE HTTP",
        title: "Use standard MCP transports only",
        body: "stdio sessions are supervised. Streamable HTTP runs behind Runtime Service and passes protocol and health probes before publication.",
        evidence: ["standard protocol", "health probe", "scoped binding"],
      },
      okf: {
        label: "OKF",
        kind: "KNOWLEDGE / NON-EXECUTABLE",
        title: "Shareable knowledge with exact citations",
        body: "OKF v0.2 represents concepts as cross-linked Markdown. The built-in SQLite/FTS5 backend stages, publishes, searches, and removes exact generations.",
        evidence: ["content digest", "FTS5 citation", "promoted generation"],
      },
      flow: {
        label: "Flow",
        kind: "A3S FLOW / NATIVE TYPESCRIPT",
        title: "One workflow engine across host targets",
        body: "Flow always uses a3s-flow and declares Tool, MCP, and OKF dependencies explicitly. flow.json records design and deployment for the same identity.",
        evidence: ["source digest", "compiled artifact", "typed catalog"],
      },
      skill: {
        label: "Skill",
        kind: "CONTENT-BOUND",
        title: "Instructions depend on ready capabilities",
        body: "A Skill binds to package content and declares required surfaces. It stays out of the snapshot until every dependency is ready.",
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
        body: "Hosts render integrity-bound HTML, CSS, and JavaScript in a sandbox with access only to declared and authorized backends.",
        evidence: ["integrity bound", "declared backend", "host sandbox"],
      },
    },
    lifecycleKicker: "REVIEWED LIFECYCLE",
    lifecycleTitle: "Prepare forward. Publish once. Remove in reverse.",
    lifecycleBody:
      "One durable operation journal keeps the reviewed plan, exact graph, grants, and idempotent checkpoints on the same recovery path.",
    lifecycle: [
      {
        number: "01",
        title: "Discover",
        body: "Refresh the signed catalog without package payloads.",
      },
      {
        number: "02",
        title: "Resolve",
        body: "Freeze versions, dependency edges, and source evidence.",
      },
      {
        number: "03",
        title: "Review",
        body: "Show impact, grants, and the exact plan digest.",
      },
      {
        number: "04",
        title: "Stage",
        body: "Verify the archive, ACL manifest, and content in a bounded root.",
      },
      {
        number: "05",
        title: "Prepare",
        body: "Prepare hosts for all six surfaces in dependency order.",
      },
      {
        number: "06",
        title: "Cut over",
        body: "Publish a new generation; drain and retire in reverse.",
      },
    ],
    architectureKicker: "OWNERSHIP",
    architectureTitle: "One Manager, one lifecycle truth",
    architectureBody:
      "CLI, TUI, and agent management MCP share one Plugin Manager. Use owns packages and evidence, Runtime owns workloads, and hosts own policy, credentials, and rendering.",
    source: "Package sources",
    manager: "Shared Plugin Manager",
    managerBody: "catalog / policy / review / plan and apply / replay",
    engine: "A3S Use package engine",
    engineBody: "verify / journal / prepare / cutover / drain",
    planes: "Six capability surfaces",
    planesBody: "Tool / MCP / OKF / Flow / Skill / UI",
    hosts: "A3S hosts",
    hostsBody: "A3S Code / OS / Knowledge / agents",
    architectureLink: "Read the architecture guide",
    trustKicker: "TRUST BOUNDARY",
    trustTitle: "Package content cannot authorize itself",
    trustBody:
      "Flow, Skill, UI, OKF, tool output, and remote content are data. Authority comes only from host policy, explicit grants, and generation-bound receipts.",
    trustLedger: "Integrity ledger",
    trustVerified: "All checks passed",
    trustCards: [
      {
        title: "Verify the supply chain",
        body: "Pin TUF roots, signed metadata, length, and SHA-256 while rejecting rollback and expired state.",
      },
      {
        title: "Stop on drift",
        body: "Resolve again before apply. Version, content, grant, or provider changes require another review.",
      },
      {
        title: "Bind authority to a generation",
        body: "Grants, Runtime bindings, generation leases, and snapshots point to the same package generation.",
      },
    ],
    platformKicker: "PLATFORM GATES",
    platformTitle: "One model across three desktop families",
    platformBody:
      "Linux and macOS run the full workspace gates. Windows x86_64 currently runs a narrower Preview compile and facade gate.",
    fullGate: "Full gate",
    previewGate: "Preview gate",
    ctaKicker: "START WITH EVIDENCE",
    ctaTitle: "Start with a package graph you can review",
    ctaBody:
      "Verify the current implementation and boundaries before connecting this development preview to an A3S host. The roadmap keeps every unfinished release gate visible.",
    ctaSecondary: "View roadmap",
    footer: "MIT licensed · Built in Rust · A3S-Lab/Use",
  },
};
