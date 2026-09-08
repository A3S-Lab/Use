<p align="center">
  <img
    src="assets/readme/hero.svg"
    width="100%"
    alt="A3S Use 解析一个精确的认知包图，并通过一次原子切换发布 Tool、MCP、OKF、A3S Flow、Skill 与 UI"
  />
</p>

<p align="center">
  <strong>Language / 语言:</strong>
  <a href="README.md">English</a> ·
  <a href="README.zh-CN.md">中文</a>
</p>

<p align="center">
  <strong>面向原生能力与版本化认知包的 AI 原生包管理器。</strong>
</p>

<p align="center">
  <a href="https://a3s-lab.github.io/Use/">网站</a> ·
  <a href="#安装或构建">安装</a> ·
  <a href="#认知包格式">包格式</a> ·
  <a href="#可替换-registry-与精确锁">Registry</a> ·
  <a href="#当前契约基线">契约</a> ·
  <a href="#实现状态">状态</a> ·
  <a href="ROADMAP.md">路线图</a>
</p>

> [!WARNING]
> **开发预览 — 尚未可用于生产。** 认知包平台尚未发布受支持的产品版本。预发布清单、回执、操作记录、目录元数据与宿主协议均非兼容性目标：不受支持的状态会被拒绝，并给出清理与重装指引。版本标签不改变此发布状态。

## A3S Use 是什么

A3S Use 解析、验证、安装、升级并移除一个精确的 SemVer 包图。认知包可贡献六种命名表面：**Tool、MCP、OKF、A3S Flow、Skill 与 UI**。包是生命周期单元；其 User 或 Workspace 安装是一致性单元。各表面一并准备，并通过一次不可变的能力快照切换对外可见。

它面向 Linux、macOS 与 Windows 上的 A3S 宿主设计。它不试图替代 `apt`、Homebrew 或 WinGet 来管理任意系统软件。A3S Use 拥有包信任、不可变代际、回执、依赖顺序、生命周期日志与能力证据。Runtime、Gateway、Flow、Knowledge 与 UI 宿主仍拥有执行与呈现。

当前架构有五条不可协商的属性：

- **一个安装图：** 每个显式 User 或 Workspace 安装拥有一个单调生成的 `InstallationSnapshot`。它拥有统一解析图以及各包的启用状态与所选表面意图。根锁是派生视图；依赖正向安装，退役反向执行，同一安装中同一包 ID 不能在不同根下解析出不同结果。
- **一条经审查的变更路径：** 规划为只读；apply 接受经审查的操作 ID、计划摘要与确认。不存在直接的 enable/disable 变更 API。
- **一次串行安装变更：** install、upgrade、uninstall、enable、disable 与精确恢复共享跨进程写者围栏。每次经审查的切换绑定到预期能力代际；失败的并发计划在 provider 或包发布效果之前即失败。
- **一个不可变内容身份：** 已验证的原始目标与展开的包树仅以摘要为键存放在全局 Artifact Store 中。Registry 源保留观测与部分下载；安装拥有选择与生命周期代际，从不保留相同内容的私有副本。
- **一个有界的 Registry 权威边界：** 安装的权威 `registry.json` 快照仅通过拥有的目录链、no-follow/reparse-safe 文件句柄、4 MiB 字节上限与原子临时文件替换来读取与发布。读取者重新检查已打开文件，拒绝同路径变更，而非解析无界或重定向文件。
- **一条当前协议基线：** 预发布格式被拒绝，而非解码、迁移或静默默认。

## 本仓库中的验证

实现与 fixture 直接演练产品模型：

- [`plugin-v3-cognitive`](crates/extension/fixtures/packages/plugin-v3-cognitive/) 是包含全部六种表面类型的内容寻址包。
- [`plugin-v3-mhs-bridge`](crates/extension/fixtures/packages/plugin-v3-mhs-bridge/) 证明硬件适配器复用标准 MCP、Flow、Skill 与 UI 图，在没有精确托管 gateway 绑定时保持未发布，且无需 MHS 专用包表面。
- [`PluginPackageResolver`](crates/core/src/plugin/package_resolution.rs) 解析有界 SemVer 闭包，并拒绝环、不兼容发布与跨 Registry 歧义。
- [`InstallationSnapshot`](crates/core/src/plugin/installation_snapshot.rs) 拥有一个作用域的期望根、统一锁图、包状态代际、启用状态与精确所选表面发布意图。
- [`RegistrySourceStore`](crates/extension/src/registry_sources/mod.rs) 持久化规范修订寻址的 ACL 源配置，导入摘要绑定的信任根，并按源身份隔离 TUF 元数据与缓存。
- [`ArtifactStore`](crates/extension/src/artifact_store.rs) 在单一分片全局 SHA-256 路径存储展开包，按摘要串行化并发提交，拒绝链接/reparse-point 祖先，且不携带安装或激活权威。
- [`CapabilityGatewayCatalogStore`](src/capability_catalog_store.rs) 拥有一个安装的精确面向 Agent 的目录 payload。
  它发布不可变规范记录，
  支持显式受保护集保留，
  并持久化有界恢复日志，
  以便中断的修剪可通过 `recover_retention()` 恢复，
  而不发明生命周期权威。
  Gateway 会话工厂的 `from_published` 与 `replace_published` 路径在暴露 live 端点前验证此精确持久发布。
  inactive Control 组合还将发布身份与已应用能力切换及已发布游标绑定在同一事务中。
  其生命周期协调读取该游标与所属操作，
  其 drain-and-retain 路径在关闭 live 端点前需要精确目录身份与 Control 签发的代际租约。
  协调备份清单在单一能力 payload 族下一起验证其规范记录与 signed/legacy 描述符快照。
- [`RegistryNetworkPolicy`](crates/extension/src/remote/network.rs) 让嵌入宿主为不受信任的 Registry 端点选择严格的公网边界。
  该模式要求 HTTPS、固定已检查 DNS 应答、拒绝非公网地址空间与代理、禁用自动重定向、在每一跳重新检查有界目标重定向，
  并同等应用于 TUF 元数据、bootstrap 根、规划目标与包目标。
- [`CognitivePackageManager`](src/cognitive_package/) 绑定 signed 目录证据、精确锁、经审查计划、授权与崩溃重放。
- [`ExtensionRegistry`](crates/extension/src/registry.rs) 将有界、owned、跨平台的文件 IO 置于已发布安装快照之后，使畸形、超大、链接或并发替换的权威不能被接纳为能力代际。
- [`CognitivePackageHostManager`](src/cognitive_package/host_manager.rs) 实现 typed host-protocol-v6 端口，
  用于一个精确托管作用域围栏。
  它将请求 ID 持久绑定到 Use 拥有的计划与终端结果，
  同时将 Registry 解析、准入、生命周期、Grants 与观测委托给与其他宿主相同的 `CognitivePackageManager`。
- [`bind_cognitive_package_provider_plan`](src/cognitive_package/provider_plan.rs) 执行授权安全的两阶段 provider 协议：未绑定草稿、assigned-provider 预检、宿主权威、规范 Grant 语义与漂移检查的最终选择。
- [`PluginPackageGraphLifecycleCoordinator`](src/plugin_lifecycle/graph.rs) 准备依赖闭包，
  执行一次持久 Registry 切换，
  调用可选的宿主 owned Gateway 激活边界，
  排空已接受调用，
  并退役精确先前代际。
  激活 hook 可重放安全，
  将其不透明键绑定到拥有已发布游标的持久 Control 操作，
  并在任何先前代际 drain 之前运行；
  inactive Control 组合为 live 会话替换提供 Control-lease 支持的适配器，
  并拒绝 copied 或无关会话的 drain 请求。
- [`RuntimeTaskDispatcher`](src/plugin_runtime/task_dispatch.rs) 重新打开审查时选定的精确 v4 Task 绑定与 provider，而能力快照 v5 仅发布具有完整安装与生命周期身份的匹配 release-backed Tasks。
- [`SqliteOkfKnowledgeAdapter`](src/okf_knowledge/sqlite/mod.rs) 对有作用域隔离的 OKF 投影进行 stage、promote、search、read 与 remove，
  附带精确包代际引用、保留的源 Markdown、有界回执计量存储、全局 tombstone 修剪、移除后的物理 SQLite 压缩、源/索引完整性审计、非覆盖 verified backup、精确计划 oldest-first 备份轮换、保留权威的 FTS 修复，
  以及权威绑定数据库加 missing-binding restore。
- [`A3sFlowLifecycleHost`](src/flow_runtime/lifecycle.rs) 将 Flow 预检委托给真实 `a3s-flow` Native TypeScript runtime，并记录精确代际绑定。
- [`StandaloneCognitivePackageLifecycleFactory`](src/cognitive_package/hosts.rs) 仅从显式绝对编译器路径组合该宿主；失败的预检保持未发布，并可从精确持久证据重放。
- [`crates/core/fixtures/plugins`](crates/core/fixtures/plugins/) 下的契约 fixture 冻结当前 schema 的规范 JSON 与 SHA-256 摘要。

CI 运行格式化、完整 A3S Use workspace 测试、Clippy、release-container 一致性以及平台任务。
Windows 预览门现在执行完整当前 workspace 套件，包括对共享 reparse-point guard 的真实 directory-junction 回归。
原生 Windows 套件还证明 Registry cutover-capacity 拒绝发生在任何 lifecycle-receipt 替换之前，且 Box 委托通过原生 command script 保留参数、输出与退出状态。
可恢复 Registry partial 在不跟随其最终路径的情况下打开，并仍由单一句柄拥有；
Windows 门证明 active partial 允许读取者但在事务释放前拒绝外部写入与移除。
Signed Registry、dependency-graph、Grant、Flow-preflight/lifecycle 与 standalone OKF 场景也通过真实 CLI 运行。

其 killed-process 覆盖现在包括：在持久 Registry 图发布之后、依赖日志与安装快照完成之前被 kill 的多节点 install；
upgrade cutover 后的 removed-dependency cleanup；
以及在持久 Registry hide 之后、包 hide 回执之前被 kill 的 uninstall。
install 离线重放精确 cutover，无需另一次代际或网络请求。
uninstall 从同一计划重启，在 accepted-call generation lease 上阻塞，然后 drain 并退役 scoped generation 权威，无需另一次 Registry 代际；
缺少精确 cutover 的包状态仍 fail closed。

Lifecycle commit 与 cleanup 对 Windows access、sharing 与 lock violation 每次阻塞变更最多重试两秒。
对 active artifact staging 目录、选定 upgrade receipt、removal receipt 或嵌套 abandoned staging 文件的 transient scanner 句柄让同一 commit 或权威退役继续。
persistent active-staging 句柄在 receipt 或 Registry-snapshot 变更之前失败，保留残余树，并在释放后让 commit 精确重放。
persistent selected-receipt lock 保留有效全局候选 artifact，并回滚 retained-receipt state，同时保留 byte-exact 先前 receipt 与已发布代际；
upgrade 重放在释放后成功。
对完整全局 artifact 的 persistent reader 不阻塞 uninstall，共享字节仍可用。

测试二进制 subprocess 矩阵还在每个规范 install、upgrade、enable、disable 与 uninstall 检查点、每次持久宿主效果之后、其 receipt 之前退出；
恢复复用精确幂等键而不重复效果，终端重放不再调用宿主。
第二个测试二进制 subprocess 矩阵覆盖带 grant 的 install、upgrade 与 uninstall 图 cutover：它在 atomic publish 或 hide 效果之后、包发布 receipt 与 Grant cutover 证据之前退出，
然后证明 exact-key 恢复、单一图效果、完成的包与 Grant 日志，
以及终端重放而无需再次 publish 或 hide。

Separate managed-scope manager 进程在 five-node install、upgrade 与 uninstall 期间、Registry publish/hide 之后、一个 dependency publication receipt 待处理且 Grant 日志仍 prepared 时被外部 kill。
重启在禁用 reauthorization 的情况下运行，
不发起网络请求，
保留精确 candidate Grant，
仅退役绑定的 prior Grant，
完成包与 Grant 日志，
且不再推进 Registry 代际。

五个真实 `CognitivePackageHostManager` 协议子进程在 Registry 服务器停止后 additionally 覆盖完整经审查 apply 集。
Install、upgrade 与 uninstall 在对应五节点图 publish/hide 边界被 kill。
Disable 在 root package binding 被 hide 且 Grant cutover 提交、accepted-call lease 阻塞 drain 之后被 kill；
enable 在 Registry publication 之后、其 candidate Grant 仍 prepared 时被 kill。
重启消费持久经审查计划与确认；
install 与 upgrade 还仅使用 verified planning cache。
恢复不 reauthorize，收敛精确 candidate/prior Grant 或 enablement regrant/revocation，在不 inflate generation 的情况下完成 drain 与两个日志，persist Host 结果，并保持终端可重放。
这些路径不替代仍开放的 actual product-host 与完整跨-platform failure-injection 门。

Grant Store 自身还在其规范 two-candidate/two-retirement 生命周期全部 14 个持久检查点运行测试二进制 subprocess 矩阵：forward prepare、cutover/retirement 与 pre-cutover rollback 各包含每个 candidate receipt、prior revocation 与 candidate restoration。
参见[平台支持](#平台支持)。

## 安装或构建

带标签的归档仍为开发预览。安装程序选择当前 OS 与架构，要求 Cosign，针对精确 A3S Use 标签 workflow 身份与 GitHub OIDC issuer 认证 `checksums.txt`，在解压前验证所选归档 SHA-256，拒绝不安全归档条目，并原子发布 user-scoped 命令。先下载安装程序以便执行前审查。

Linux 或 macOS：

```bash
curl --proto '=https' --tlsv1.2 -fsSLo /tmp/a3s-use-install.sh \
  https://raw.githubusercontent.com/A3S-Lab/Use/main/install.sh
sh /tmp/a3s-use-install.sh
```

Windows x86_64，使用 Windows PowerShell 5.1 或 PowerShell 7：

```powershell
$installer = Join-Path $env:TEMP 'a3s-use-install.ps1'
Invoke-WebRequest https://raw.githubusercontent.com/A3S-Lab/Use/main/install.ps1 -OutFile $installer
& $installer
```

`cosign` 必须安装在 `PATH` 上；Unix 上可用 `--cosign <path>`，Windows 上可用 `-CosignPath <path>` 选择显式可信可执行文件。

Unix 上传 `--version <version>`，Windows 上传 `-Version <version>` 以固定标签。
Unix 安装于 `$XDG_DATA_HOME/a3s-use`（或 `$HOME/.local/share/a3s-use`），并从 `$HOME/.local/bin` 链接。
Windows 使用 `%LOCALAPPDATA%\A3S\Use`，在 `%LOCALAPPDATA%\A3S\bin` 下创建 owned command shim，并将该 bin 目录加入用户 `PATH`，除非设置 `-NoPathUpdate`。
托管 launcher 绑定打包的 OCR 模型、OCR Skills 与 Browser Skills，同时保留显式环境覆盖。
重装相同版本会重新验证完整已安装树。
缺少 Cosign、无效 Sigstore 证据、checksum 不匹配、被篡改的现有 release、不安全路径、link/reparse point、并发安装程序或 unmanaged command 冲突会在不改变 active command 的情况下失败。
verified checksum manifest 与 Sigstore bundle 保留在不可变版本目录中。
参见[已验证发布安装](docs/release-installation.md)了解信任边界与自定义路径选项。

带标签 release workflow 设计为发布确定性序列化归档、每平台一份 SPDX JSON SBOM、GitHub OIDC build-provenance 与 SBOM attestations，以及 `checksums.txt` 的 keyless Sigstore bundle。
它固定每个 Action 以及 Rust、Python、Syft 与 Cosign 版本，从标签 commit 派生归档时间戳，并在发布前验证其 checksum 签名。
安装程序 fail closed，除非 Cosign 在下载归档前针对相同标签身份认证该 bundle。
对每个目标，第二个无编译 artifact 缓存的 clean runner 重建所有 shipped 原生可执行文件，且必须在 deterministic `.reproducibility.json` 证据可被 attest、checksum、签名并发布到归档旁之前与主归档 byte-match。

`v0.3.7` Rust 兼容性 release 在携带 path-free Capability Gateway descriptor/catalog 适配器的同时，
保留 post-`v0.3.3` 的 atomic-snapshot-lease、shared manager、Runtime service rebinding 与 standard MCP manager 契约。
Complete snapshot 通过 clean-target staging、activation 与 crash replay 携带有界、规范的 Runtime plan archive；
artifact reachability 保留 committed plan 引用的 blob。
它将 facade 的精确 `a3s-flow 1.1.0` registry 依赖与 `a3s-code-core 8.0.3` 对齐，并发布 `a3s-use-core 0.2.6`、`a3s-use-extension 0.3.7` 与 `a3s-use 0.3.7`。
facade 继续使用与 A3S Search 相同的 released Browser 0.3.2 provider，因此 packaged consumer 可解析一个 nominal Browser/Core/Flow 能力图。
这是兼容性 release，不改变开发预览状态。
Gateway 适配器在 lifecycle lease/drain、authentication、CLI wiring 与 independent-client qualification 完成前仍是 contract-level increment。

带标签 `v0.3.2` workflow 在五个目标中的四个上暴露原生 linker 元数据漂移，因此未创建 GitHub Release。
未发布的[qualification run 33651777660](https://github.com/A3S-Lab/Use/actions/runs/33651777660) 冻结 `main` commit `4f6e4725205d06ab81f8ea98bfee85c7eb4b2bcd`，
并通过完整五平台归档、isolated-install path scan、SBOM 与 attestation，
以及 cache-free byte-for-byte rebuild 矩阵；
它仍是历史证据，从不发布资产。
较早的 `v0.3.5` 发布尝试因 public `a3s-use-core` crate 仍为 `0.2.4` 而未创建 GitHub Release。
Release workflow [33675697857](https://github.com/A3S-Lab/Use/actions/runs/33675697857) 随后从精确 `main` commit `54758910f2f4ad9498137410e0a2207d412e99a1` 构建 tag `v0.3.6`，
通过全部 primary 与 independent 五目标 job，
并发布开发预览 [v0.3.6 Release](https://github.com/A3S-Lab/Use/releases/tag/v0.3.6) 及 `a3s-use-core 0.2.5`、`a3s-use-extension 0.3.6` 与 `a3s-use 0.3.6` 包。
Release workflow [33687297386](https://github.com/A3S-Lab/Use/actions/runs/33687297386) 随后从精确 `main` commit `48a0b76f8a4a87a11d16627c7bd7567920852508` 构建 tag `v0.3.7`，
通过全部 primary 与 independent 五目标 job，
并发布开发预览 [v0.3.7 Release](https://github.com/A3S-Lab/Use/releases/tag/v0.3.7) 及 `a3s-use-core 0.2.6`、`a3s-use-extension 0.3.7` 与 `a3s-use 0.3.7` 包。
Release workflow [33720485826](https://github.com/A3S-Lab/Use/actions/runs/33720485826) 随后从精确 `main` commit `6d3a7baf32ce998a2e487c40fbf78b4a6cda2579` 构建 tag `v0.3.8`，
通过完整 validation、五目标 primary build 与 independent cache-free rebuild 门，
并发布开发预览 [v0.3.8 Release](https://github.com/A3S-Lab/Use/releases/tag/v0.3.8) 及 `a3s-use-core 0.2.7`、`a3s-use-extension 0.3.8` 与 `a3s-use 0.3.8` 包。
Release workflow [33756618837](https://github.com/A3S-Lab/Use/actions/runs/33756618837) 随后从精确 `main` commit `a5f3cc40bfb0a1021ca150d2ce4295409b74d220` 构建 tag `v0.3.9`，
通过完整 validation、五目标 primary build 与五 independent cache-free rebuild，
并在 [v0.3.9 Release](https://github.com/A3S-Lab/Use/releases/tag/v0.3.9) 发布 19 个 verified release 资产，
包括归档、安装程序、checksums/Sigstore、SBOM 与 reproducibility 证据，
以及 `a3s-use-core 0.2.7`、`a3s-use-extension 0.3.9` 与 `a3s-use 0.3.9` 包。
Release workflow [33791616307](https://github.com/A3S-Lab/Use/actions/runs/33791616307) 随后从精确 `main` commit `c4c80a223bfff3698ca4b4598e7175c6e3303239` 构建 tag `v0.3.10`，
通过完整 validation、五目标 primary build 与五 independent cache-free rebuild，
并在 [v0.3.10 Release](https://github.com/A3S-Lab/Use/releases/tag/v0.3.10) 发布 19 个 verified release 资产，
包括归档、安装程序、checksums/Sigstore、SBOM 与 reproducibility 证据，
以及 `a3s-use-core 0.2.8`、`a3s-use-extension 0.3.10` 与 `a3s-use 0.3.10` 包。
Release workflow [33830280138](https://github.com/A3S-Lab/Use/actions/runs/33830280138) 随后从精确 `main` commit `c25028ae0245ba1d28f7e2837e2a87f7e9f6fe40` 构建 tag `v0.3.11`，
通过 validation、五目标 primary build 与五 independent cache-free rebuild，
并在 [v0.3.11 Release](https://github.com/A3S-Lab/Use/releases/tag/v0.3.11) 发布 19 个 verified release 资产，
包括归档、安装程序、checksums/Sigstore、SBOM 与 reproducibility 证据，
以及 `a3s-use-core 0.2.9`、`a3s-use-extension 0.3.11` 与 `a3s-use 0.3.11` 包。
外部运营的全归档 witness、GitHub Release 之外的证据保留，以及剩余产品门仍开放，因此不改变上述预览状态。
操作者还可按照[已验证发布安装](docs/release-installation.md#additional-independent-verification)验证成功的 GitHub attestations。

### 构建与验证

需要 Rust 1.85 或更新版本。在产品 release 门完成前，从源码构建：

```bash
git clone https://github.com/A3S-Lab/Use.git
cd Use
cargo build --workspace --bins --locked
./target/debug/a3s-use doctor \
  --scope-kind user --scope-id user/alice --json
./target/debug/a3s-use capability snapshot \
  --scope-kind user --scope-id user/alice --json
```

Rust 嵌入宿主可将同一权威 Extension Registry 绑定到 typed capability bridge，并固定一个完整已发布代际：

```rust
use a3s_use::capability_registry::CapabilityRegistry;

let capabilities = CapabilityRegistry::new(extension_registry);
let observed = capabilities.snapshot().await?;
let lease = capabilities
    .acquire_snapshot_lease(observed.cursor())
    .await?
    .ok_or_else(|| a3s_use::core::UseError::new(
        "host.capability_snapshot_stale",
        "The observed A3S Use generation is no longer callable.",
    ))?;
```

游标绑定 Installation Snapshot 代际与摘要、能力修订、Registry 修订，以及排序后的精确包代际。
Acquisition 按规范顺序获取每个 package-generation lease，并在持有完整 batch 后重新检查两个不可变权威。
hidden、stale、mixed、contended 或 digest-mismatch 代际不返回 lease；
没有不可变 lifecycle 证据的 enabled legacy package binding fail closed。
不可 clone 的 RAII lease 为 `Send + Sync`，因此 A3S Code 可在 Run scope 中保留它，而 Use lifecycle retirement 等待 accepted work drain。
Drop 仅释放同步 generation lock；
异步 cleanup 仍由 Use lifecycle coordinator 显式拥有。

Capability watch 现在订阅 atomic Extension Registry 发布，而非在固定间隔重建完整投影。
优先使用原生 filesystem backend，同时运行有界 target-metadata probe 以捕获平台 backend 可能 coalesce 或省略的 atomic replacement；
当 native registration 不可用时使用 metadata-only polling backend。
事件经 target 过滤并 coalesce 为单一有界信号；
validated `registry.json` 仍是权威。
`CapabilityRegistry` 在 subscription setup、真实 generation advance 之后，以及 timeout 时各重建并 hash 完整投影一次，以关闭最终 race。
这从正常 wait 路径移除了重复 package scan 与 asset hashing，而不创建第二个 mutable generation cursor。
在 lifecycle Capability Index 中持久化完整面向 agent 的 descriptor catalog 仍是单独的产品门。

`capability snapshot --json` schema v5 仍是外层 CLI envelope。
它暴露 Installation Snapshot 代际与摘要，而完整 in-process cursor 故意不追加到该独立发布的 schema。
Managed-MCP、Skill identity 与 UI dependency 字段是显式的。
每个 extension MCP 表面保留其规范 ID 与 multiplicity、collision-resistant host server name、activation、package/manifest/generation identity、经审查 file-evidence digest，
以及一个 transport-specific launch projection。
Stdio projection 仅包含 package-relative executable 与有界 arguments。
Streamable HTTP projection 仅包含 package-relative release、opaque endpoint reference/path，以及精确 Runtime/Gateway readiness digest；
resolved URL 与 credentials 从不进入 snapshot。
每个 UI 贡献携带 `a3s.use.ui-dependency-evidence.v1`，以便空 dependency list 可与未发布 dependency evidence 的旧宿主区分。

独立 CLI 当前暴露 package-graph lifecycle、diagnostics、capability observation、内置 Browser/OCR 路由、cited OKF search，以及 exact-scope Knowledge storage 操作：

```text
a3s-use install <publisher/name> --scope-kind <user|workspace> --scope-id <id> [--registry-name <name>] [--offline] [--json]
a3s-use upgrade <publisher/name> --scope-kind <user|workspace> --scope-id <id> [--registry-name <name>] [--offline] [--json]
a3s-use uninstall <publisher/name> --scope-kind <user|workspace> --scope-id <id> [--json]
a3s-use plugin search <query> --scope-kind <user|workspace> --scope-id <id> [--kind <flow|mcp|okf|skill|tool|ui>] [--channel <stable|beta|nightly>] [--cursor <cursor>] [--limit <n>] [--offline] [--json]
a3s-use plugin inspect <publisher/name> --scope-kind <user|workspace> --scope-id <id> [--version <semver>] [--channel <stable|beta|nightly>] [--offline] [--json]
a3s-use plugin list-installed --scope-kind <user|workspace> --scope-id <id> [--cursor <cursor>] [--limit <n>] [--json]
a3s-use plugin status <publisher/name> --scope-kind <user|workspace> --scope-id <id> [--json]
a3s-use plugin plan-install <publisher/name> --scope-kind <user|workspace> --scope-id <id> [--registry-name <name>] [--version-requirement <semver-range>] [--channel <stable|beta|nightly>] [--surface <kind/id>]... [--offline] [--json]
a3s-use plugin plan-upgrade <publisher/name> --scope-kind <user|workspace> --scope-id <id> [--version-requirement <semver-range>] [--channel <stable|beta|nightly>] [--surface <kind/id>]... [--offline] [--json]
a3s-use plugin plan-uninstall <publisher/name> --scope-kind <user|workspace> --scope-id <id> [--json]
a3s-use plugin plan-enable|plan-disable <publisher/name> --scope-kind <user|workspace> --scope-id <id> [--json]
a3s-use plugin apply-plan --operation-id <id> --plan-digest <sha256> --scope-kind <user|workspace> --scope-id <id> --yes [--json]
a3s-use plugin observe-operation <publisher/name> --operation-id <id> --plan-digest <sha256> --scope-kind <user|workspace> --scope-id <id> [--json]
a3s-use plugin watch-operation <publisher/name> --operation-id <id> --plan-digest <sha256> --scope-kind <user|workspace> --scope-id <id> [--after-revision <sha256>] [--timeout-ms <ms>] [--json]
a3s-use plugin cancel-operation <publisher/name> --operation-id <id> --plan-digest <sha256> --scope-kind <user|workspace> --scope-id <id> --yes [--json]
a3s-use extension inspect <publisher/name> --scope-kind <user|workspace> --scope-id <id> [--json]
a3s-use extension diagnose <publisher/name> --scope-kind <user|workspace> --scope-id <id> [--history] [--json]
a3s-use knowledge search <query> --scope-kind <user|workspace> --scope-id <id> [--limit <n>] [--json]
a3s-use knowledge usage --scope-kind <user|workspace> --scope-id <id> [--json]
a3s-use knowledge audit --scope-kind <user|workspace> --scope-id <id> [--json]
a3s-use knowledge backup <path> --scope-kind <user|workspace> --scope-id <id> [--json]
a3s-use knowledge verify-backup <path> --scope-kind <user|workspace> --scope-id <id> [--json]
a3s-use knowledge backup-retention <directory> --scope-kind <user|workspace> --scope-id <id> [--max-backups <n>] [--max-bytes <n>] [--plan-digest <sha256> --yes] [--json]
a3s-use knowledge plan-restore <path> --scope-kind <user|workspace> --scope-id <id> [--json]
a3s-use knowledge restore <path> --plan-digest <sha256> --scope-kind <user|workspace> --scope-id <id> --yes [--json]
a3s-use knowledge restore-status --scope-kind <user|workspace> --scope-id <id> [--json]
a3s-use knowledge repair-search-index --scope-kind <user|workspace> --scope-id <id> --yes [--json]
a3s-use registry source list [--json]
a3s-use registry source add <name> (--url <https-url> | --github <owner/repository>) --trust-root <sha256> [source options] [--json]
a3s-use registry source replace <name> (--url <https-url> | --github <owner/repository>) --trust-root <sha256> --expected-revision <sha256> --yes [source options] [--json]
a3s-use registry source default|enable|disable|remove <name> --expected-revision <sha256> --yes [--json]
a3s-use registry cache usage [--registry-name <name>] [--json]
a3s-use registry cache prune [--registry-name <name>] [cache options] --yes [--json]
a3s-use state backup <path> --scope-kind <user|workspace> --scope-id <id> [--json]
a3s-use state verify-backup <path> --scope-kind <user|workspace> --scope-id <id> [--json]
a3s-use state backup-retention <directory> --scope-kind <user|workspace> --scope-id <id> [--max-backups <n>] [--max-bytes <n>] [--plan-digest <sha256> --yes] [--json]
a3s-use state plan-restore <backup> --scope-kind <user|workspace> --scope-id <id> [--json]
a3s-use state restore <backup> --rollback-backup <external-path> --plan-digest <sha256> --scope-kind <user|workspace> --scope-id <id> --yes [--json]
a3s-use state restore-status --scope-kind <user|workspace> --scope-id <id> [--json]
a3s-use capability snapshot|watch --scope-kind <user|workspace> --scope-id <id> [options] [--json]
a3s-use mcp serve manager --scope-kind <user|workspace> --scope-id <id> [--offline]
```

`mcp serve manager` 在 stdout 上 speak standard MCP，
因此不得与 `--json` 组合。
`manager`、`package-manager` 与 `use/package-manager` 目标名称等价。
它组合 CLI 与 TUI 使用的同一 typed `PluginManagerService`；
它不创建第二个 catalog、plan、confirmation 或 mutation 路径。

独立 Registry-backed `install`、`upgrade` 与 `uninstall` 现在通过共享 `PluginManagerService` 规划并 apply。
其现有 component 与 `packageGraph` 字段仍可用，而 JSON 输出还包含 `pluginManager` 对象，内含精确 operation ID、plan digest、经审查 Host plan result 与 terminal Host apply result。
重复 unchanged operation 返回 durable replay result。
Offline planning 与 recovery 保持 zero-network，supplied package-lock digest 在任何 target download 之前被拒绝。
兼容性命令仅 auto-apply 无权限的 `Allow` 计划。
一对一 `plugin` 命令暴露与 manager toolset v5 相同的四个 read 操作、五个 read-only planning 操作、digest-only apply、exact operation observation/watch，以及显式 cancellation 边界。
每个成功 JSON `data` 值是精确 typed service result，包括完整 Host plan、package lock、source 与 permission evidence、operation ID、plan digest、confirmation decision 与 terminal apply result。
Planning 从不 mutate package state。
`plugin apply-plan` 仅重新打开精确 durable `(operation ID, plan digest)` 对并要求 `--yes`；
普通 CLI 调用不隐含 user confirmation，`Ask` plan 仅在该显式边界获得 confirmation。
Exact apply 与 replay 使用 verified planning cache 而无需 Registry 访问。
A3S Code CLI、TUI `/packages` 与 standard manager-v5 MCP 现在组合同一 service，而无 presentation-owned plan、confirmation 或 mutation 路径。
每个独立 manager 命令要求显式 User 或 Workspace 安装。
所选 `InstallationId` 拥有 manager 与全部 mutable state；
相同 textual ID 的 User 与 Workspace 安装仍是 distinct authority domain。

Runtime、Flow、Knowledge 与 lifecycle evidence store 在构造时捕获该精确 `InstallationId`。
另一 installation 的 receipt、query、recovery item 或 lifecycle intent 在 Use 派生 path、获取 store lock、创建 database 或写入 evidence 之前以 `use.installation.identity_mismatch` 失败。
Separate installation 使用 separate store，而不可变 Registry 与 artifact 输入仍可共享。

Scoped layout 与 global Artifact Store 是有意的预发布 clean cutover，而非 migration。
若报告 `use.installation.legacy_state_unsupported` 或 `use.artifact_store.legacy_state_unsupported`，停止旧 Use 宿主，保留先前 root 供 incident review，并仅移除已证明的 legacy 条目后再用显式 scope flag 重装。
这些条目包括旧 global `data/extensions`、installation-scoped `data/installations/<kind>/<key>/extensions`，
以及旧 state-level `extensions`、`registry.json`、generation、Grant、binding、lifecycle、Knowledge、graph、enablement、Host Manager、route-lock/generation-lease 与 mutation-lock 路径。
保留 global `registries.acl`、Registry trust root、TUF metadata/targets 与 `data/artifacts`；
它们是 installation 共享的输入。

展开内容位于 `data/artifacts/expanded-packages/sha256/<prefix>/<digest>/content`。
不同 installation 可指向同一 exact tree，同时保留独立 receipt、generation、enablement、Grant、binding 与 lease。
Use 在 publication 与 use 前 rehash 内容。
Global byte 仅通过显式 confirmed Artifact Store garbage-collection plan 删除；
source cleanup 与 scoped uninstall 仍从不删除它们。
Cross-process shared/exclusive boundary 现在防止 future inventory 与 collection 与 raw-target observation、lifecycle receipt、applying lifecycle journal、installation snapshot 或 pending graph operation 竞争。
Source observation 与 resumable partial 仍为 Registry-source scoped；
其 verified byte 使用 global Blob tier。
库在 exact exclusive store guard 下暴露有界、确定性、path-free 物理 inventory。
它分别报告 canonical content 与 abandoned staging，并在 unknown layout、link/reparse point、special file 或 traversal limit 上 fail closed。
Separate path-free Registry reference inventory 在同一 guard 下从所有保留 source datastore（包括 replaced source）派生每个 canonical blob observation。
Global path-free `a3s.use.artifact-reference-inventory.v1` view 现在将这些 observation 与每个 installation snapshot、current 与 retained receipt、non-cancelled package-graph operation、applying 或 rolling-back lifecycle journal，
以及 immutable Runtime plan payload 聚合。
Runtime plan record 在持有 installation maintenance 与 plan-store lock 时解码，因此其引用的 Blob artifact 在 cleanup 期间仍 reachable。
通过 `ExtensionPaths`-bound plan store 的生产 publication 在该 installation fence 之前获取 global reference admission；
为 isolated offline/test state 创建的 store 不携带 global Artifact Store boundary。
它验证 installation identity 与 source layout，拒绝 conflicting physical expectation，并在 content missing 时仍 retain reference。
Joined `a3s.use.artifact-reachability-inventory.v1` view 在一次 guarded collection pass 中捕获该 logical evidence 与 physical inventory。
New publication 被冻结；
reference retirement 只能留下 conservative extra owner。
每个 artifact 一行，保持 owner、physical measurement、expectation status 与 checked global storage usage 分离。

Artifact Store 现在在 `data/artifacts/storage-quota.acl` 拥有可选 durable hard-quota policy。
`ArtifactStore::storage_quota`、`set_storage_quota` 与 `clear_storage_quota` 通过 revision compare-and-swap 暴露 canonical ACL state。
Policy 约束 logical regular-file length 与 digest container，而非 allocated filesystem block。
Publication 总是先进入 reference admission，再进入 global storage boundary，最后进入 exact digest lock。
无 policy 时 publisher 共享 storage boundary。
有 policy 时，最终 Blob 或 expanded-package publication 独占持有它，扫描 current content 加 abandoned staging，project exact prepared write，并在 staging cleanup 与 atomic commit 期间 retain lock。
因此 distinct process 不能同时 spend 同一 remaining capacity。
若 operator 将 policy 收紧到低于 current usage，exact replay 与不 worsen 任一 exceeded dimension 的 cleanup 仍可能。
Malformed policy 在不 suppress physical inventory evidence 的情况下 fail closed write。

`ArtifactStore::audit_digests` 现在在 exact store-bound collection guard 下执行显式 full-store integrity pass。
其 deterministic、path-free `a3s.use.artifact-store-digest-audit.v1` report 用 raw SHA-256 顺序 rehash 完整 raw Blob，
用 admission 时相同的 canonical package fingerprint rehash expanded package。
它报告 `verified`、`mismatch` 与未 hash 的 `incomplete` outcome，以及 checked byte/file total。
Pass 在返回前重复 bounded physical inventory，因此 admitted publication 在整个 operation 期间被冻结，observable layout 或 measurement drift fail closed。
Digest mismatch 仍是 evidence；
audit 从不 remove、overwrite、quarantine 或 rehydrate content。

Effect owner 现在有 path-free verified read boundary，而非将 `expanded_package_path` 视为 authority。
`ArtifactStore::acquire_verified_package` 接受一个 complete verified catalog record，
以 shared mode 获取 global reachability 与 per-artifact mutation lock，
拒绝 interrupted collection 与 logical quarantine，
并 revalidate 完整 package fingerprint、manifest digest、exact byte/file count、manifest-to-catalog surface graph，
以及每个 declared surface file。
不可 clone 的 lease 仅暴露 catalog identity 与 parsed manifest；
其 `Debug` 形式不含 local path。
Manifest read 在 ACL parsing 前有界，missing lock 从不由 read 创建，`verify_unchanged` 重复完整 verification 以在 adapter 记录 success 前检测 uncoordinated local tampering。

Logical corruption quarantine 是单独的 exact-plan operation。
`ArtifactStore::plan_quarantine` 仅接受同一 exact collection guard 下 fresh audit 的一个 complete mismatch，并返回 canonical、path-free evidence。
`apply_quarantine` re-audit byte，要求 exact reviewed plan digest，并 atomically publish bounded canonical `quarantine.json` record，而不 move 或 overwrite `content`。
同一 record 的 replay 是 idempotent。
Failed recovery 保留 bounded temporary fail-closed sentinel，因此 ordinary access 在 retry 之间不 reopen。
Physical inventory 验证 active 与 interrupted quarantine metadata，但将其排除在 content 与 staging quota measurement 之外。
New Blob open、observation 与 commit，以及 expanded-package validation 与 commit，在 marker 存在后 fail closed。
该 marker 保留 forensic byte 并 block ordinary future use；
它不 revoke already-open handle、rewrite admitted generation、authorize rehydration 或 authorize deletion。

Verified rehydration 是由 `ArtifactStoreMaintenance` 协调的 separate reference-aware mutation。
Planning 与每个 nonterminal apply 获取 exact global collection guard 并 rescan 每个 Registry observation、installation snapshot、current 或 retained receipt、pending package graph 与 nonterminal lifecycle operation；
target 在 replacement 前必须有 zero durable reference。
Independently supplied candidate 必须位于 Artifact Store 之外，并匹配 expected Blob SHA-256 或 canonical expanded-package fingerprint。
Planning 仅 emit path-free evidence。
Initial apply 要求其 exact canonical digest，
reverify candidate 与 quarantine binding，
durably publish prepared evidence，
并在 stage 与 switch canonical content 期间保持 ordinary access fail-closed。
Matching completion record 打开 access。
Exact terminal replay 是 read-only：它 validate completion、quarantine binding 与 canonical replacement，而不 reopen external candidate 或要求 later owner 再次 retire。
Interrupted preparation 或 content switching 从 bounded state resume，moved 或 conflicting record fail closed，hard quota admission 计入 temporary recovery peak。
Apply 消费 reviewed corrupt forensic byte；
需要更长 evidence retention 的 operator 必须在 confirmation 前在 store 外 archive。
Existing open handle 不被 revoke，但 no admitted package generation 可在 replacement 期间 reference target。

Confirmed Artifact Store garbage collection 是由 `ArtifactStoreMaintenance` 协调的 separate reference-aware mutation。
其 policy 是非空、有界、canonical allowlist，包含 exact `(kind, digest)` target；
没有 timer、age threshold、quota-triggered sweep 或 implicit「all unreferenced」模式。
Planning 持有 global collection guard，
在每个 Registry、installation、receipt、snapshot 与 nonterminal operation 上证明 zero durable owner，
并将 exact physical measurement 以及 ordinary、quarantined 或 completed-rehydration lifecycle evidence 绑定到 path-free plan。
Apply 重复 zero-reference proof，且仅接受 reviewed canonical plan digest。
在任何 namespace mutation 之前，它 publish durable global prepared record。
每个 reviewed digest container 随后在其 shard 内 atomically rename 到 deterministic tombstone，并通过 bounded、no-link residual-tree check 移除。
Prepared 或 temporary state 在 restart 前 block 新 reference admission，直到同一 plan resume。
Durable completion record 使 exact replay read-only，因此 old confirmation 不能删除 later recreated 或 newly referenced 的 identical digest；
每个 later plan 链接到 previous completion digest。
Quarantine、rehydration、audit、quota pressure 与 physical unreachability 仅是 evidence，从不 independently authorize deletion。

Joined quota assessment 仍仅是 evidence；
它不 authorize deletion。
Hard admission 故意 serialized，而非实现为 parallel durable reservation ledger。
`complete` 仍仅是 physical publication state；
explicit digest audit 产生 separate integrity result。
Exact-plan logical quarantine 与 zero-reference verified rehydration 仍与 explicit confirmed garbage collection 分离；
none 授予另一者的 authority。

默认 Knowledge policy 将每个完整 User 或 Workspace scope 限制为 512 MiB receipt-accounted expanded content、256 retained projection、每 surface 32 generation 与 256 removal tombstone。
Staging 原子检查整个 scope；
receipt-owned removal 释放 quota、prune old tombstone，并 compact SQLite 及其 WAL。
`knowledge usage --json` 报告 exact scope、current count、quota、allocated database byte 与 reclaimable byte。
这些 standalone control 还 audit SQLite、receipt、scope、foreign-key 与 FTS consistency。
Backup 写入一个 versioned、SHA-256-bound SQLite snapshot，而不 overwrite existing file；
verification 离线 reopen 并 audit embedded database。
`knowledge backup-retention` 在一个 owned directory 中 verify 每个 managed `*.a3s-okf-backup` candidate，isolate exact scope，并返回 oldest-first bounded plan。
在提供 `--yes` 与 unchanged canonical `planDigest` 之前它 remove nothing，从不 remove 最后一个 verified scope backup，并将 partial deletion 报告为 outcome-unknown。
Search-index repair 要求 `--yes`，并仅 rebuild 来自已 validated document 的 FTS row。
它从不 rewrite package receipt、projection state 或 authorization evidence。
Authority-bound restore 将 path-free plan review 与 digest-only confirmed apply 分离，
verify 完整 Registry/package/lifecycle/Grant authority 与 exact-subset binding inventory，
bind live main/WAL/SHM evidence，
仅 restore missing binding file，
preserve prior file，
并在 process exit 后 resume six-state durable journal。
Conflicting 或 newer binding evidence fail closed。
`knowledge restore-status --json` 读取所选 installation 的 active marker 与 bounded path-free history，无需 backup path 或 plan digest；
它报告 current phase、exact digest、retained directory count、unrecorded marker-handoff directory 与 remaining capacity，而不改变 restore 或 database evidence。

Backup 是 integrity-checked scope database snapshot，而非 signed trust artifact 或 whole-product restore。
Standalone restore 仅可在 current set 是 backup 的 exact subset 且 Registry receipt、immutable package root、lifecycle journal 与 Grant 仍 exact 时 recreate binding file。
它不能 recreate 那些 independent authority。
更广泛的 authority recovery、clean-machine recovery、cross-platform operational drill 与 whole-product rollback-evidence retention 仍需要 procedure。
每个 installation-scoped operation 要求显式 `--scope-kind` 与 `--scope-id`；
CLI 从不猜测 current User 或 Workspace identity。
参见 [OKF Knowledge 操作](docs/okf-knowledge-operations.md)。

对于 quiescent whole-installation inventory，
`state backup` 获取该 installation 的 exclusive maintenance fence，
并 snapshot Registry、installation-snapshot、retained-generation、Grant、binding、lifecycle/package-operation、Knowledge、enablement 与 Host Manager control state。
Expanded package byte 是 global immutable input，不被复制。
其 `a3s.use.state-backup.v2` manifest 绑定 exact installation，
且仅包含 portable relative path、per-file length/SHA-256/mode evidence、family accounting、Registry generation/digest，
以及 sorted installed-receipt digest。
Creation scan、copy with exact hashing，然后在 non-overwriting publication 前 rescan。
Lock 被排除；
active restore、pending cutover/operation、link/reparse point、special file、unknown state family、installation data payload 或 non-portable path fail closed。
`state verify-backup` 离线 validate canonical manifest byte、complete archive length 与每个 payload digest，无需 extraction 或 local Use state。
Archive 包含 raw state，必须作为 sensitive data 保护。
`state backup-retention` 获取 separate external-directory lock，
fully verify 每个 managed archive，
并返回 path-free oldest-first plan，
绑定 exact file name、modification time、length、manifest digest、inventory digest 与 Registry evidence。
Confirmed apply 仅接受 unchanged canonical `planDigest`，synchronize 每个 deletion，按 exact installation filter archive，并 always retain 至少 newest two verified archive。
Global Registry source/trust/TUF state、Artifact Store 与 derivable Flow compiled artifact 故意在此 backup 之外。
`state plan-restore` 仅当 backup 精确匹配 current Use version、OS、architecture 与 independently retained Registry/receipt/Grant authority 时，构建 path-free Add/Replace/Remove/Retain review。
Confirmed `state restore` 首先 create 或 verify explicit external rollback archive，
仅 stage publication candidate，
并 advance durable seven-phase journal，
其 15 process-exit boundary idempotently converge。
Active marker 在 live mutation 前 publish；
candidate link/reparse point 与 marker 或 journal substitution fail closed；
completed history 有界为 64 record；
`state restore-status` 是 path-free 且 read-only。
Archive 仍是 integrity evidence，而非 signature 或 missing-authority recovery mechanism；
clean-machine recovery 与 operational disaster-recovery drill 仍开放。
参见 [协调状态备份操作](docs/state-backup-operations.md)。

`extension inspect --json` 包含显式所选 installation 的最新与上一个 durable lifecycle operation。
Versioned diagnostic projection 报告 action、status、generation、artifact digest、checkpoint progress、bounded error code、timing 与 rollback evidence。
它故意 omit checkpoint idempotency key、credential、token、secret value 与 package-authored error text。
这是用于 diagnosis 的 checkpoint evidence，而非 telemetry service 或 backup/restore mechanism。
一个 reviewed graph operation 可为同一 package 创建 consecutive candidate 与 retirement phase intent。
这些 record 故意共享 `operationId`；
consumer 通过 `intentDigest`、action、generation 与 artifact digest 区分 exact phase。

`extension diagnose --json` 读取一个 exact retained install、upgrade 或 uninstall graph、一个 active admitted enable/disable operation，
或所选 User 或 Workspace scope 最新且尚未 admitted 的 Host-reviewed enable/disable plan，
无需 network I/O、reconciliation、recovery 或 write。
其 `a3s.use.plugin-operation-diagnostic.v1` projection 绑定 reviewed plan 与 lock digest、path-free Registry name 与 TUF role version、current Registry generation 与 cutover evidence、provider identity/readiness、Grant journal phase、lifecycle publication/drain/rollback state，
以及 stable recovery guidance。
Graph diagnostic 覆盖 retained planned、admitted 与 cancelled operation，并在仅存在 reviewed pending plan 时于 installation 之前工作。
在 enable/disable admission 之前，
digest-bound observation index 按 `(plannedAtMs, requestId)` 选择 newest exact Host plan，
并 project `planned` 或 `cancelled`、selected provider 与 awaiting-Grant state、expected lifecycle-unit count，
以及 current Registry cutover evidence。
Index 保留 managed Host scope 仅用于 resolve 其 immutable request；
Host ID、authority/fence value、request ID 与 private path 从不进入 public projection。
Active Use-owned enablement evidence 优先，durable Host outcome 或 completed Use operation 抑制 stale plan。
URL、path、idempotency key、credential、token、secret name/value、package content 与 arbitrary package-authored text 被排除。
对于 retained install 或 upgrade graph，projection 还报告 total expected 与 currently retained archive byte，以及每个 exact target 的 `missing`、`partial` 或 `complete` cache state；
aggregate 为 `missing`、`in-progress` 或 `complete`。
在 reviewed graph 存在之前，Use 在 process-held package lock 下 durable record exact non-authoritative package lock 与 selected archive set。
`extension diagnose` 随后返回带相同 byte evidence 的 `a3s.use.plugin-download-attempt-diagnostic.v1`。
Record 在 download failure 或 process exit 后 survive，later attempt 仅可在 process lock 释放后 replace，并仅在 reviewed pending graph durable 后 remove。
两个 projection 还通过 `planningBytes`、`planningRetainedBytes`、aggregate `planning` 与 per-package `planningTargets` 报告 exact retained package lock 所选 separately signed executable-planning target。
每个 target 仅暴露 package ID、Registry name、target digest、expected/retained byte 与 `missing`/`partial`/`complete` state。
Static package 报告 `not-required`。

在 exact package lock 存在之前，Use 还将 Registry/TUF 工作记录为 `a3s.use.plugin-resolution-attempt.v1`。
Record 在 refreshed 或 cached metadata access 之前开始，并跟踪 requested version/channel 以及每个 root 或 dependency Registry 为 pending、verifying、verified 或 failed。
它仅暴露 path-free Registry name、source-identity/trust-root digest、verified TUF role version、bounded target count、stable error code 与 terminal package-lock digest/count。
Killed 或 failed resolver 仍可诊断；
successful resolution 在 remove 此 evidence 之前写入 download attempt。
当 graph 与 download attempt 都不存在时，`extension diagnose` 返回 phase `pre-lock` 且 access `refreshed` 或 `cached` 的 `a3s.use.plugin-resolution-attempt-diagnostic.v1`。
它从不暴露 Registry URL、path、raw transport error、credential 或 metadata byte。

`extension diagnose --history --json` 为同一 explicit scope 返回 `a3s.use.plugin-operation-history-diagnostic.v1`。
它在 exact 8 MiB store bound 内 retain newest 16 retired operation，
包括其 complete path-free operation snapshot 与 separately validated `completed`/`rolled-back` operation 或 `cancelled` graph-plan outcome。
History 在 remove pending graph 或 active enablement recovery authority 之前写入；
同一 `(operationId, planDigest)` occurrence 的 replay 是 idempotent。
Textual graph operation ID 在 exact reinstall 后可能 legitimately recur，因此 plan digest 仍是 occurrence identity 的一部分。
History 在 uninstall 后仍 available。
Unknown field、identity/outcome conflict、link 或 reparse point，以及 oversized record fail closed，而不 echo retained byte 或 path。

Graph 与 download projection 从 retained signed provenance 派生 historical Registry datastore，不发起 network request 或 write，不暴露 path，也不 acquire target-cache lock。
Complete archive 或 planning target 是 canonical source observation 加 owned exact-length global blob；
diagnostic 不 rehash，partial 也不是 apply、planning 或 recovery authority。
Resolution diagnostic 同样 read-only 且 zero-network，不 wait 或 acquire package lock。
Real-process test 证明 killed planning-target observation 与 exact Range resume、reviewed Host planned/cancelled enablement projection（无 admission、authorization 或 network access），
以及 completed-Use/unfinished-Host outcome window 期间的 suppression。

### 可替换 Registry 源

独立 CLI 在规范 A3S ACL 中持久化有界 named Registry source 集。
第一个 enabled source 成为 default。
每个 enabled source 都提供给 dependency resolution，
而 `--registry-name` 为一次 operation 选择 root source。
跨 enabled source 的 duplicate package identity 作为 ambiguous 失败。

在 package resolution 之前配置 trust：

```bash
a3s-use registry source add packages \
  --url https://packages.example.org/a3s/ \
  --trust-root sha256:<64-hex-digits> \
  --json
```

GitHub 仓库可用作 Homebrew-tap 式 authoring 与 static distribution source，而不将 Git history 作为 trust root：

```bash
a3s-use registry source add official \
  --github A3S-Lab/Use-Registry \
  --trust-root sha256:<64-hex-digits> \
  --json
```

简写解析为 `https://raw.githubusercontent.com/<owner>/<repository>/main/registry/`。
`--github-ref` 与 `--github-path` 可选择 canonical tag/branch name 与 repository subtree。
它们仅是 address input：caller-pinned TUF root、signed catalog-v3 metadata、archive hash、reviewed plan 与 Grant 仍是 installation 与 activation authority。
A3S Use 从不 clone 或 execute repository checkout。

`--trusted-root /absolute/path/root.json`  additionally 将 exact digest-matching root 导入 managed、content-addressed trust-root store。
Source list 输出包含 complete configuration revision。
Replacing authority 需要该 reviewed revision 与 explicit confirmation：

```bash
a3s-use registry source list --json

a3s-use registry source replace packages \
  --url https://mirror.example.org/a3s/ \
  --trust-root sha256:<64-hex-digits> \
  --expected-revision sha256:<reviewed-configuration-revision> \
  --yes \
  --json
```

Replacing、disabling 或 removing source 从不 rewrite installed receipt，也从不 delete 其 identity-bound TUF metadata、observation、partial 或 global blob。
Re-enabling 或 restoring exact name、URL 与 bootstrap-root digest 复用该 exact source state。
Changed source identity 获得 separate datastore，防止 old metadata 或 observation 跨越 trust boundary。

来自 configured Registry 的示例开发 install：

```bash
a3s-use install acme/research \
  --scope-kind workspace \
  --scope-id workspace/acme-project \
  --registry-name packages \
  --version 2.0.0 \
  --json
```

当 lock 被 separately reviewed 时，将 apply 绑定到它：

```bash
a3s-use install acme/research \
  --scope-kind workspace \
  --scope-id workspace/acme-project \
  --registry-name packages \
  --package-lock-digest sha256:<64-hex-digits> \
  --json
```

Mismatched lock digest 在 archive download 之前失败。上述示例 package 与 Registry name 仅作说明；本仓库不 advertise public production Registry。

Online install 验证 current TUF metadata，
并将每个 selected archive 与 signed `planning-v1.json` target 存入 Registry datastore 的 content-addressed cache。
在该 exact graph 被 remove 之后，
可再次 install 而无需 network access：

```bash
a3s-use install acme/research \
  --scope-kind workspace \
  --scope-id workspace/acme-project \
  --registry-name packages \
  --version 2.0.0 \
  --offline \
  --json
```

同一 flag 支持 upgrade，仅当 host 已 refresh candidate 的 TUF metadata 并将每个 selected target verify 到同一 cache：

```bash
a3s-use upgrade acme/research \
  --scope-kind workspace \
  --scope-id workspace/acme-project \
  --registry-name packages \
  --version 2.1.0 \
  --offline \
  --json
```

Offline mode 是 explicit 且 fail-closed。
它 load 同一 persisted Registry source revision；
revalidate cached TUF signature、expiry、source identity、target length 与 SHA-256；
并在 JSON 中返回 `registryAccess: "cached"` 加 `registrySourceRevision`。
Normal online operation 返回 `registryAccess: "refreshed"`。
Missing、disabled、expired 或 tampered source 或 cache evidence 是 error。
Online command 在 network 或 refresh failure 后从不 fallback 到 cached target。

### 已验证目标缓存操作

每个 Registry 有独立的 default logical working-set limit：4 GiB 与 4,096 个 combined target observation 与 resumable partial，以及 256 MiB source-partial/staging free-space reserve。
Interrupted HTTP download 保留 digest-bound `.target-<sha256>.part`，并仅从 exact signed Range response retry。
Fully verified byte 通过 transaction-owned handle copy 并 rehash 到 `<data-root>/artifacts/blobs/sha256/<shard>/<digest>/content`，
synchronize，
并在不 replace existing content 的情况下 publish。
仅 then Registry source publish canonical `<digest>.json` observation metadata 并 remove 其 partial。
Cached staging reopen 并 rehash global blob；
corruption fail closed 且从不 silently replace。

Windows-native test 建模 blob publication 与 source cleanup 上的 scanner contention。
若 final partial deletion 仍 locked，durable blob 与 observation 仍 usable，retry 移除 redundant partial 而无需 network transfer。
Source prune 移除 stale write，然后 inactive partial，然后 oldest observation。
它释放 logical source-policy capacity，但从不 delete global blob、installed artifact、receipt、generation 或 journal。
Global reference 现在与 physical evidence 及跨 source、installation 与 operation 的 bounded quota assessment join。
Optional global hard quota admission 与 read-only digest audit 覆盖两个 publication tier；
exact-plan logical quarantine block newly observed corrupt content 同时 preserve 其 byte，verified rehydration 需要 independent candidate 加 fresh zero-reference proof。
Global deletion 现在 additionally 需要 bounded explicit target policy 与其 exact confirmed GC plan digest。

Inspect cache usage 而无需 Registry request：

```bash
a3s-use registry cache usage \
  --registry-name packages \
  --json
```

Pruning 可 discard resumable progress 与 source observation，因此 standalone CLI 要求 explicit confirmation：

```bash
a3s-use registry cache prune \
  --registry-name packages \
  --cache-max-bytes 2147483648 \
  --cache-max-entries 2048 \
  --cache-min-free-bytes 536870912 \
  --yes \
  --json
```

Durable policy 在 `registry source add` 或 `replace` 上配置。
Confirmed prune 可使用 stricter one-command override；
它不 rewrite source configuration。
Embedding host 使用同一 typed `VerifiedTargetCachePolicy`。
Cache usage 与 pruning 是 zero-network operation，并在 inspect 或 delete source state 前 validate 任何 retained catalog-cache source identity。
Schema v3 将 `targetBytes` 报告为 logical referenced blob byte，而非 prune 回收的 physical byte。
此 source-cache GC 从不 change global raw 或 expanded artifact、receipt、capability generation 或 lifecycle journal。
参见 [Registry 缓存操作](docs/registry-cache-operations.md)。

## 认知包格式

认知包是 npm 式 immutable distribution unit，
具有一个 `<publisher>/<name>` identity、一个 SemVer version、required ACL manifest、required package documentation、optional package dependency，
以及零个或多个 named surface contribution。

```text
acme-research/
├── a3s-use-extension.acl   package identity, dependencies, surfaces
├── README.md               required package documentation
├── tools/                  native Task or Service artifacts
├── releases/               immutable Tool or MCP descriptors
├── flows/                  A3S Flow TypeScript sources
├── skills/                 SKILL.md files and supporting content
├── ui/                     integrity-bound static assets
└── okf/                    Open Knowledge Format bundles
```

仅 manifest 与 `README.md` name 是 fixed。Contribution path 由 manifest 拥有。Manifest 是 A3S ACL (`.acl`)，必须用 [`a3s-acl`](https://github.com/A3S-Lab/ACL) 解析；ACL 不是 HCL。

```acl
extension "acme/research" {
  schema_version = 3
  version        = "2.0.0"
  route          = "research"
  requires_use   = ">=0.3.0, <0.4.0"
  actions        = ["read", "execute"]

  dependency "acme/base" {
    version = "^1.4.0"
  }

  repository {
    url      = "https://github.com/acme/research"
    revision = "0123456789abcdef0123456789abcdef01234567"
  }

  tool "convert" {
    workload    = "task"
    interface   = "cli"
    executable  = "tools/convert/bin/convert"
    command     = "acme-research-convert"
    json_output = true
    interactive = false
    timeout_ms  = 120000
    activation  = "lazy"
    optional    = false
  }

  mcp "library" {
    transport  = "stdio"
    executable = "tools/library/bin/library-mcp"
    args       = []
    activation = "lazy"
    optional   = false
  }

  okf "domain" {
    format_version         = "0.2"
    root                   = "okf/domain"
    content_digest         = "sha256:355b6f00153630b082e60a0f7e0b67fbbb74b2a29067bca481f7eefecbb86c7a"
    concept_count          = 4
    file_count             = 7
    expanded_bytes         = 2053
    max_files              = 256
    max_concepts           = 64
    max_expanded_bytes     = 67108864
    max_document_bytes     = 1048576
    max_links_per_document = 2048
    optional               = false
  }

  flow "review" {
    engine         = "a3s-flow"
    runtime        = "native-ts"
    source         = "flows/review.ts"
    export         = "run"
    requires_tool  = ["convert"]
    requires_mcp   = ["library"]
    requires_okf   = ["domain"]
    optional       = false
  }

  skill "review" {
    path          = "skills/review/SKILL.md"
    requires_tool = ["convert"]
    requires_mcp  = ["library"]
    requires_okf  = ["domain"]
    requires_flow = ["review"]
    optional      = false
  }

  ui "review" {
    entry     = "ui/review/index.html"
    skill     = "review"
    bind_mcp  = ["library"]
    bind_flow = ["review"]
    optional  = false
  }
}
```

`route` attribute 是 optional，仅作为 human-facing CLI alias 保留。
它不要求 unique，且从不 own installation state、accepted-call lease、cursor package identity 或 Tool/MCP host name。
Automation 应通过 `<publisher>/<name>` 寻址 package，通过 canonical kind 与 surface ID 寻址 surface；
ambiguous alias lookup fail closed。

| 表面 | 包贡献 | 就绪 owner |
| --- | --- | --- |
| Tool | Package-local native Task 或 digest-pinned Task/Service release | Signed planning launcher 加 native provider，或 explicitly selected Runtime |
| MCP | Package-local stdio server 或 digest-pinned HTTP release | Signed stdio launcher 加 native provider，或 Runtime/Gateway readiness |
| OKF | Open Knowledge Format concept graph | Knowledge host stage、promotion、observation 与 cited retrieval |
| A3S Flow | 带 explicit surface edge 的 TypeScript workflow source | `a3s-flow` preflight 与 exact compiled binding |
| Skill | Canonical surface ID 加 content-bound `SKILL.md` 与支持文件 | Required dependency ready 后的 static projection；host 保持 manifest ID 与从 document 解析的 presentation metadata 分离 |
| UI | Integrity-bound static entry point | Lifecycle 验证 entry 与 exact asset digest，project canonical sorted Skill/Tool/MCP/Flow dependency set 及 versioned completeness marker，仅 publish complete dependency evidence，并在 remove 时 clear receipt-owned projection。Sandboxing、rendering、state 与 backend binding 仍由 host 拥有 |

Surface 可选用于 projection，但不能在其 owning package generation 之外 independently install、upgrade 或 remove。

## 单一 A3S Flow 生命周期

A3S Use 不定义第二个 workflow engine。

- Package manifest 声明 `flow` surface、source digest、export 与 Tool/MCP/OKF dependency。
- `a3s-flow` 拥有 compilation 与 execution semantics。
- Host 可将 `flow.json` 用作 visual design 或 deployment document，但它不是另一个 package receipt、dependency resolver 或 lifecycle journal。
- A3S Code 是 local host，A3S OS 可以是 remote execution target；两者必须 resolve 同一 package-owned Flow identity。

Required Flow publication 在 embedding host 未 inject declared Flow runtime 时 fail closed。
没有 source-presence 或 `PATH` fallback。
Standalone CLI 对 install、upgrade 与 uninstall 跨 process restart 使用同一 reviewed absolute compiler path opt in：

```bash
A3S_FLOW_NATIVE_TS_COMPILER=/opt/a3s/bin/a3s-flow-native-compiler \
  a3s-use install acme/workflows \
    --scope-kind workspace \
    --scope-id workspace/acme-project \
    --registry-name packages \
    --json
```

`CognitivePackageManager::new` 保持 provider-free 且 deterministic；
`CognitivePackageManager::from_env` 是 explicit standalone composition。
Missing 或 failing compiler 留下 installed-disabled candidate receipt，但 immutable capability snapshot 仍在其 exact prior generation，不 project staged package state。
Lifecycle diagnostic 保留 bounded failure evidence。
Repaired retry resume 同一 admitted plan 与 exact package generation，然后 publish 一次 reviewed capability cutover，而非 guess 或 expose partial state。

## 可替换 Registry 与精确锁

Registry URL 与 trust root 是 host input，
从不 compile 进 resolver。
Host 可选择 mirror、private Registry 或另一个 explicitly trusted TUF source，
而无需 change package logic。
每个 dependency 可从不同 enabled source resolve，
但同一 package 出现在多个 enabled source 中会被作为 ambiguous 拒绝。

当前 Registry 规则：

- Managed host 首先通过 state-free `inspect_bootstrap_root` 从 supplied byte 派生 exact digest/version/size evidence，
  然后通过 `TrustedRegistry::pin_trusted_root` pin 相同 byte。
  两个 API 共享单一 public one-MiB bound 与 decoder；
  pinning additionally 在 ordinary refresh 执行 complete TUF chain、expiration 与 rollback verification 之前 enforce configured digest、regular-file check、metadata lock 与 immutable replay。
- TUF target `custom.a3s` metadata 包含一个 complete catalog-v3 record。
- 每个 executable catalog 携带一个 separately signed `planning-v1.json` target。它在 archive download 之前区分 package-local Tool/stdio MCP launcher 与 release-backed Runtime workload。
- Mixed package 被 planned 为一个 exact provider set：native Tool Task 与 stdio MCP 留在 built-in launcher，
  而 release-backed Tool Task、Tool Service 与 HTTP MCP 需要 typed `RuntimeClientRegistry` 的 explicit host assignment。
  Missing Grant、generation、assignment 或 provider 无 fallback 即失败。
- Provider selection 是两阶段。
  Capability preflight 将 real provider enforcement 暴露给 host policy；
  final pass 绑定 canonical Grant semantics，
  且必须 retain 相同 provider ID、build、normalized capability 与 enforcement。
  Final policy decision 也必须 unchanged。
- Installed schema-v6 receipt 为每个 executable package retain exact installation ID、optional non-owning CLI alias 与 signed planning bundle。
  因此 enablement 可在 restart 后再次 reviewed，
  而无需 consult mutable Registry，
  同时 catalog、manifest 与 installed package byte 仍被 revalidate。
- Apply-time host adapter 从 immutable reviewed plan 与 durable snapshot re-derive Grant proposal，
  reconstruct exact Runtime selection，
  并要求 provider evidence byte-for-byte match。
  Shared A3S CLI、TUI 与 managed-host enablement path 持久化 reconstruction input，
  而非 process-local client。
- Retirement 从不 choose new activation provider。
  Disable、uninstall 与 prior-generation upgrade cleanup reopen exact Runtime binding receipt 记录的 provider；
  在 Service drain 与 remove 之前 recheck provider ID、build 与 normalized capability。
- Release-backed Runtime Task binding 使用 current self-contained receipt：argument-free reviewed Runtime template、Grant/descriptor/provider evidence、capture contract 与 exact lifecycle generation 在 process restart 后 survive，
  而不依赖 short-lived operation record。
  每次 invocation 仅 derive unique unit ID 与 bounded argv，
  reopen receipt-owned provider，
  并在 output capture 与 cleanup 期间 hold exact published-generation lease。
  Hidden 或 replaced generation 拒绝 new call。
- Runtime Task publication 与 dispatch 还将 durable binding cross-check 到 installed package 的 retained planning evidence。
  Registry-trusted package 必须 retain catalog-bound signed planning bundle 与 exact release descriptor digest；
  self-consistent 但 substituted descriptor、package generation 或 missing evidence 在 provider connection 之前被 omit 或 reject。
- Catalog record、archive、expanded package 与 manifest 均有 exact digest/size evidence。
- Archive admission 将每个 planning launcher rebind 到 exact digest-bound `.acl` manifest 与 release descriptor；
  surface kind、activation、executable、argv、command、timeout 与 transport drift fail closed。
- Prepared download 与 installed Registry/TUF receipt 必须 retain full verified catalog record 及其 provenance。
- Online preparation 在 `<registry-datastore>/verified-targets/sha256/<digest>.json` 保留 source observation，
  并将 verified archive、planning target 与 presentation media commit 到 global sharded blob tier。
  Cache read 拒绝 link 与 non-regular file，
  通过 retained handle rehash blob，
  并在 admission 前 verify signed length。
- Explicit cached resolution revalidate last trusted、unexpired TUF metadata 与 exact Registry name、URL 与 trust root。它从不 refresh network，也从不 weaken source 或 package-lock provenance。
- Typed per-Registry policy 约束 logical referenced byte、observation 与 partial，
  并 reserve source/staging disk space。
  Digest-bound partial 在 process interruption 后 survive，
  仅通过 exact HTTP range response resume，
  且从不 staged 于 full signed-length 与 SHA-256 verification 之前。
  Automatic 与 confirmed source cleanup 在同一 cache lock 下 remove stale write，
  然后 oldest partial 与 observation。
  它从不将 source-reference removal 视为 global blob deletion。
- Real-process recovery coverage 还在 verified archive extraction 期间 kill installation，
  证明 no receipt、installation snapshot、pending operation 或 package root 被 publish，
  并从 revalidated cache 完成 explicit zero-network retry。
- 以下 real-process package-copy interruption 保留 exact pending plan 与 applying journal，
  但无 receipt、installation snapshot 或 package publication。
  Offline replay reclaim physical `.artifact-staging-*` residue，
  并 exactly once publish reviewed generation。
- Real-process uninstall interruption replay exact lifecycle identity，finish scoped receipt 与 authority retirement，preserve global artifact byte，且不再第二次 advance Registry generation。
- Real-process multi-node install interruption 在 atomic Registry graph publication 之后 retain 一个 complete visible closure 与其 durable cutover，
  但无 installation snapshot。
  Offline replay 完成每个 package journal，
  write exact snapshot，
  retire cutover，
  且 Registry generation 保持 1 而无需 network request。
- Watcher 读取 immutable publication 而无需 wait behind writer。
  若 one-time crash reconciliation 短暂 own Registry lock，
  lifecycle writer 异步 wait 最多两秒；
  genuinely concurrent mutation 仍以 `use.extension.busy` 失败。
- Installed receipt 仍 bound 到其 source name、URL、root digest、release channel、target 与 TUF role version。
- Replacing source configuration 从不 rewrite installed receipt provenance；restore exact source 或 reinstall 在 upgrade 之前是 required。

Canonical package lock 冻结 selected version、dependency edge、host target、`requires_use`、archive 与 package digest，
以及每个 node 的 Registry identity 与 TUF provenance。
Resolution 在 cycle、incompatible constraint、missing provider、source ambiguity 与 configured search bound 上 fail closed。

## 经审查的生命周期

```text
verified catalog set
        ↓
resolve SemVer closure → freeze exact lock → build immutable plan
        ↓                                      ↓
policy + confirmation                  reviewed plan digest
        └──────────────────────┬───────────────┘
                               ↓
download changed nodes → commit disabled → prepare dependencies forward
                               ↓
                  one durable capability cutover
                               ↓
             drain prior calls → retire generations reverse
```

Install、upgrade、uninstall、enable 与 disable 是 durable operation。
Apply 在 mutation 前 revalidate exact package lock、catalog evidence、host capability、policy authority、scope、confirmation 与 current state。
Upgrade plan 绑定 prior 与 candidate lock，并将每个 node 分类为 `Add`、`Replace`、`Remove` 或 `Retain`。

Managed activation 与 retirement 故意使用不同 evidence。
Enable 或 candidate install/upgrade 使用 host-owned two-pass provider selection。
Disable、uninstall 或 prior-generation upgrade cleanup 不携带 candidate selection，并 retire exact receipt-owned binding。
若 stopped binding 以 new authorization semantics 被 re-enable，old binding 在 same package generation rebind 之前被 retire；
conflicting immutable receipt 从不 in-place overwrite。

Manager MCP toolset 将 read-only planning 与 mutation 分离暴露：

```text
plugin_plan_install     plugin_plan_upgrade     plugin_plan_uninstall
plugin_plan_enable      plugin_plan_disable     plugin_apply_plan
plugin_observe_operation plugin_watch_operation plugin_cancel_operation
```

`plugin_apply_plan` 是唯一 manager package-state mutation 入口；
`plugin_cancel_operation` 是 separate pre-admission control-plane mutation，不能 publish package generation。
`NoChange` enablement result 是 terminal，无 synthetic mutation identity。
Crash recovery resume exact stored plan 与 authorization；
re-read finished operation 返回 durable result 而不 repeat side effect。
Applying 与 rolling-back record 均 retain exclusive operation ownership；
不同 intent 在其 reach terminal record 之前不能 replace 任一 record。
Inspection 在同一 package-scoped journal lock 下读取 latest 与 previous record。

`PluginManagerService` 现在是 `CognitivePackageHostManager` 之上的 shared typed application boundary。
它拥有 deterministic request identity、Registry-bound search cursor、stable installed-state pagination、SemVer install/upgrade selection、全部五个 planning path、durable plan reopening 与 digest-only apply。
`PluginManagerMcpServer` 通过 standard MCP initialization、`tools/list` 与 `tools/call` 暴露 exact thirteen v5 tool；
其 schema 与 annotation 从 frozen toolset 生成。
MCP apply 与 cancellation 向 injected trusted host confirmation provider 询问 existing exact evidence，且从不将 agent tool call 视为 user confirmation。
Standalone CLI 的 Registry-backed install、upgrade 与 uninstall mutation 使用此 service，并在 released output field 旁暴露 exact reviewed Host plan/result。
其 `plugin` surface 将全部十三个 manager operation 映射到同一 service，
保持每个 plan read-only，
暴露 exact operation observation/watch，
且 apply 或 cancellation 要求 exact operation ID、plan digest 与 explicit `--yes`。
Code TUI `/packages` 与 Code-side manager MCP 现在使用该同一 service。
Human CLI 与 TUI review 从 immutable Manager envelope 派生一个 deterministic、read-only projection，
展示 exact plan identity、candidate/prior package graph、source、transition、complete permission ceiling、provider/impact/state evidence 与 confirmation boundary，
而不改变 machine JSON。
TUI 在 exact apply 前 scroll 完整 review。
此 qualification 在 A3S CLI `main` commit `bef7c913cbefba62638b37f91ce9263f4db2ffbb` 落地；
CI run [32786647662](https://github.com/A3S-Lab/CLI/actions/runs/32786647662) 通过全部五个 main、Linux、macOS 与 Windows job。
Six-surface product-host E2E 仍是 release gate。

Host protocol v6 绑定 explicit User 或 Workspace scope kind，并仅从 durable evidence project exact operation state。
不同 kind 中 equal textual scope ID 不能 share fence、plan、request replay record 或 Host operation。
Protocol 报告 factual phase 与 bounded checkpoint count，而非 invented percentage；
将每个 status revision 绑定到 complete status，支持 revision-based long polling，且仅在接受 durable graph 或 enablement admission 之前 accept explicit-user cancellation。
Publication 使 cancellation 为时已晚；
仅 durable Host outcome 报告 `Completed`。

A1 two-installation qualification matrix 将同一 signed OKF package 驱动通过 concurrent User 与 Workspace installation（相同 textual ID）的 install、Host restart、exact capability snapshot、leased query、upgrade、uninstall 与 terminal replay。
每次 mutation 使另一 installation 的 cursor unchanged 且其 retained lease callable，而 immutable package byte 通过 shared Artifact Store deduplicate。

Production managed-host adapter 仅存储 protocol request/operation binding 与 terminal projection。
它不创建第二个 package、authorization 或 recovery state machine。
Expired plan 仍 unusable，除非 exact Use-owned evidence 证明它已在 original review window 内被 admitted 或 completed；
merely planned operation 必须再次 planned 与 reviewed。

Workspace Grant 被 compose 进同一 graph saga。
Candidate grant 在 package preparation 之前 persist，
exact Registry cutover 被 record，
accepted call 在 prior grant revoke 之前 drain，
pre-cutover failure 将 package 与 Grant candidate 一起 rollback。

## 架构

<p align="center">
  <img
    src="assets/readme/architecture.svg"
    width="100%"
    alt="可信源进入经审查的 Plugin Manager 与 A3S Use 图生命周期，随后原子能力快照到达 A3S 宿主"
  />
</p>

| 边界 | 拥有 | 不拥有 |
| --- | --- | --- |
| Host Plugin Manager | Registry 配置、trust root、policy、user confirmation、reviewed plan/apply | Package byte 或 provider scheduling 内部 |
| A3S Use | Verification、exact lock、immutable generation、receipt、grant、lifecycle journal、cutover evidence | Generic scheduling 或 UI rendering |
| Runtime/Gateway | Tool 与 MCP provider execution、health 与 drain | Package resolution 或 trust policy |
| A3S Flow | Workflow compilation、execution、replay 与 observation | 并行 package lifecycle |
| Knowledge host | OKF validation、indexing、promotion、cited search | Process execution |
| A3S Code/OS | Product UX、workspace/session scope、rendering、injected provider | 第二个 package manager |

参见 [Plugin Platform Architecture](docs/plugin-platform-architecture.md)、
[Lifecycle and Security](docs/plugin-platform-lifecycle-and-security.md)、
[ADR-002](docs/adr-002-cognitive-package-lifecycle-saga.md)，以及
[Control Store transaction boundary](docs/adr-003-control-store-transaction-boundary.md)。
Machine-checked
[coordinated cutover inventory](docs/control-store-cutover.md) 分类每个
current state leaf、external owner、operational file 与 consumer，它们必须一起切换；
它 explicitly 保持 production activation inactive，并 forbid dual write 或 legacy fallback read。

Private A2 Control Store kernel 现在 qualify 其 clean-state schema-v11 aggregate。
每个 operation 存储 canonical complete reviewed Plan envelope 与 versioned authorization evidence，
然后 derive 并 revalidate 其 operation ID、Plan 与 authorization digest、action、root package、
installation scope 与 generation cursor（在 restart 与 offline export verification 期间）。
Authorization evidence v2 仅 retain exact prior Grant snapshot、reviewed change set 与 confirmation fact；
resolved Grant 及其 receipt revision 是 derived output，而非 caller authority。
Installation generation、desired package-state generation、immutable package-lifecycle generation
与 Grant receipt revision 保持 distinct。

Commit 前，kernel 从 reviewed Plan、exact prior generation、bounded committed history
与 reviewed Grant evidence reconstruct complete target snapshot、两个 package generation axis
与 complete target Grant inventory。全部五个 action、User 与 Workspace installation、
multi-root shared dependency，以及 uninstall/reinstall 因此 reject caller-selected package 或 Grant identity。
Offline export 与 restore verifier 再次运行同一 projection。

Projection 还为每个 enabled Tool 与 MCP surface reconstruct complete reviewed Runtime provider selection。
它 retain unrelated selection，remove disabled 或 removed surface，
store full provider build/capability/semantics/enforcement evidence，
并从 reviewed Plan evidence 上的 versioned canonical descriptor derive 每个 selection digest。
Flow、OKF、Skill 与 UI 仍是 typed host effect，而非被 assign fictional Runtime provider。

Separate candidate capability digest 从 target snapshot、package lifecycle identity、
Grant revision 与 provider selection derive。它仅描述 committed desired capability identity；
endpoint、readiness、compiled artifact 与 Knowledge application observation 仍是 post-commit evidence。

同一 projection derive complete bounded sequence of work，不能 join local transaction：
surface preparation、capability cutover、accepted-call drain 与 surface stop 或 removal。
Dependency surface 在 dependant 之前 prepare；retirement 反向该顺序；
upgrade 在 cutover 之前 prepare new incarnation，并在 removal 之前 drain old incarnation。
每个 effect 命名 typed Capability Index、invocation-lease、Runtime、Flow、Knowledge、Skill 或 UI owner。
Tool 与 MCP effect 携带 exact reviewed provider ID 与 selection digest；
static host 从不 receive fictional Runtime selection。
Optional selected surface 可在 cutover 前 degrade，但其 required dependency closure 与每个 retirement effect 仍 required。

Package selection、lifecycle identity、Grant 与 reviewed provider selection 已在 aggregate 中 commit，
因此不 duplicated 为 pseudo external effect。
Canonical payload byte、其 domain-separated idempotency key、digest 与 relational projection 一起 commit，
并在 restart 与 offline export verification 后再次 verify。

Applied outcome persist canonical、owner-specific evidence，而非 arbitrary success digest：
Capability Index receipt 现在 bind exact immutable Agent-facing catalog digest/generation/revision、
invocation-lease receipt、exact Runtime selection 加 portable Task 或 opaque `gateway:` Service binding/readiness evidence、
Flow artifact digest、Knowledge projection digest，以及 immutable Skill/UI content digest。
每个 application rebind exact idempotency key 与 intent。
Deferred、rejected 与 unknown outcome 仅 retain diagnostic evidence。
Deferred 保留给证明 accepted no effect 的 owner；
它 persist bounded not-before time 以便用 same key 自动 retry。

Recording applied capability-cutover observation 在 drain、retirement 或 operation completion 之前
retire prior publication、publish exact candidate，并在同一 transaction 中 advance capability cursor 及该 catalog binding。
Required post-cutover failure 因此仍 reconciliation-pending，且必须 reuse original identity；
它不能 rollback already visible generation。Completion 不能 predates 任何 provider observation。

Kernel 还 qualify typed generation transition、full Grant 与 reviewed provider selection evidence、
idempotent outbox reconciliation、bounded execution、corruption check，
以及 deterministic offline-verifiable export 加 staged restore。
其 inactive dispatcher 现在从 claim 到 later observation 持有 one installation-wide shared maintenance fence，
至多 claim 一个 committed effect，在进入 owner 之前 release claim transaction 与 bounded executor，
将 Capability Index、invocation-lease、Runtime、Flow、Knowledge、Skill 与 UI work 路由到 separate typed port，
然后在 later transaction 中 record owner-specific applied、deferred、rejected 或 unknown evidence。

Deferred effect 在其 durable not-before time 之前不能 reclaimed，然后以 same key 自动 retried。
Provider timeout 必须在其 claim lease 内 leave fixed observation budget；timeout 是 durable unknown evidence。
Timeout 或 cancellation 仅 detach wait，而非 possibly accepted effect task：
该 task retain same shared fence 直到 actually finish。Process exit 仍 require explicit same-key reconciliation。

Test 证明 commit-before-effect、provider I/O 期间 Store re-entry、unobserved process exit 后 exact-key recovery、
hung-provider bounding 与全部七个 owner route。
Concurrent whole-installation restore 不能 acquire exclusive maintenance fence，
直到 provider observation durable 且 any detached in-process effect future 已 finish。

Claim transaction 现在还 derive owner-shaped committed context：
package port 仅 receive exact package selection、lifecycle、host、snapshot identity 与 Grant；
Runtime 还 receive 其 full reviewed provider selection；
Capability Index receive candidate generation 加 retained multi-root history 中
每个 enabled selected surface 的 latest terminal preparation。
Optional rejection 是 explicit degradation，而 missing Grant coverage、nonterminal 或 teardown state
与 generation drift 在 owner I/O 之前 fail closed。
Multi-root test 还 fixed generation insertion，使同一 transaction 内所有 package node
precede 其 immediate-foreign-key dependency edge。
Dispatcher 不由 production lifecycle code 构造，且不 beside current JSON store 创建 second authority。

First concrete post-commit owner adapter 现在 against 该 boundary qualify immutable Skill 与 UI preparation。
它从 portable request re-derive typed owner 与 idempotency key，仅通过 verified artifact lease acquire exact package，
read 一个 named surface 而不 expose package root，并在返回 stable path-free receipt 前 re-verify complete package。
Claim attempt 与 deadline 不改变该 receipt。
Artifact contention 是 durable same-key deferral；tampering、missing content 或 authority substitution
是 proved-no-effect rejection；read-only adapter 从不 report ambiguous acceptance。
Static stop 与 remove 是 path-independent projection receipt，因此在 artifact collection 后仍 replayable。

Second concrete adapter 现在 against 同一 committed boundary qualify OKF Knowledge。
First preparation 消费 path-free、fully verified OKF byte payload；
stage receipt-owned SQLite/FTS5 state；在 promotion 前 persist staged evidence；
在 report applied 前 persist promoted evidence；并返回 exact observation 与 capability projection digest。
Retained promoted receipt replay 而无需 reopen Artifact Store。
Pre-effect contention safely defer，authority 或 byte drift reject，
任何 ambiguous stage、promote、remove 或 receipt-persistence boundary 对 explicit same-key reconciliation 仍 unknown。
Stop 是 path-independent checkpoint，remove 仅使用 retained projection receipt。

Composition test 现在 prove committed Control claim 通过 real Knowledge adapter 并 back 到 durable Control application observation。
Artifact admission separately idempotent 并在 create 无 installation lifecycle receipt 时 revalidate prepared source；
caller 必须通过 separate authority commit retain 其 global reference-admission guard。

Third concrete Capability Plane adapter 现在 own Capability Index publication 与 invocation drain。
Validate committed authority 后，它 call host-owned pure projector，
reject enabled 且 successfully prepared package incarnation 之外的 descriptor，
durably publish exact Agent catalog，并 materialize 一个 canonical content-addressed Index document 绑定该 publication。
不创建 second SQLite database 或 mutable `current` file。
Applied cutover observation 仍是 sole publication transaction，并以 catalog digest/generation/revision advance Control cursor。

Invocation admission reopen 并 rehash 那些 exact byte，verify Index，
在 shared lock 下 read Control publication 于每个 exact package lifecycle incarnation，若 cutover raced 则 return stale。
Drain 首先 prove old incarnation 不再 published，然后在 any accepted call retain shared lock 时 safely defer；
release 后 apply same effect key。
Catalog 与 Index publication 是 no-replace、no-follow、crash-replayable 且 path-free。
Index 是 derived operational state，excluded from backup；
coordinated state inventory 现在 register 并 semantically verify catalog 与 descriptor-snapshot record
作为 one `CapabilityPayloads` family，而 lock、staging、journal 与 lease file 仍 excluded。

`ControlCapabilityPayloadRestoreCoordinator` 现在在 one exclusive maintenance fence 下 bind catalog 与 descriptor plan，
preflight 两个 clean target，并 retry fixed-order activation 而不 clobber already-published owner。
`ControlCapabilityPayloadRetentionCoordinator` 现在在 same exclusive fence 下 bind 两个 owner retention plan，
preflight 两个 inventory（包括 exact pending journal），并 replay fixed-order deletion。

Inactive composition 现在 retain 同一 Capability Plane，
并可在 restart 后 reopen durable published Control cursor 而不 accept caller-selected cursor。
Reopening revalidate exact Index 与 catalog，reacquire 每个 package-generation lease，若 concurrent cutover wins 则 return stale。
Production Control owner registration、live Gateway session construction from returned lease、
lease drain 与 lifecycle retention authority 仍是 separate gate。
Real composition test join Knowledge、Skill、catalog/Index publication、exact payload admission、stale admission 与 drain。

Inactive composition 现在在 one lifecycle admission seam 接受 canonical cognitive-package Plan envelope、
authorization evidence 与 optional planned Grant transition。
它从 immutable Plan derive prior installation 与 capability cursor，而非 accept caller-selected value。
其 combined composition entry point retain one installation-wide fence，
同时 register exact reviewed operation、publish Runtime plan payload，并在 any provider effect 之前 commit projected generation。
Production 仍须 route live lifecycle 通过此 seam 并 compose dispatcher。

Inactive kernel 现在还有 committed-authority Flow owner：
它 read bounded Flow source 作为 path-free verified Artifact Store payload，
在 owner-controlled workspace publish durable no-clobber content-addressed copy，
并仅 invoke typed `a3s-flow` Native TypeScript preflight。
Package path 从不 cross 该 boundary；compiler/cache path 是 operational host configuration 而非 desired-state authority。
Source substitution 与 failed preflight reject 而无 Control observation，而 Artifact Store contention safely defer。
Stop/remove 是 path-independent receipt。此 qualification 在 production dispatcher composition cut over 之前 remain inactive。

Committed-authority Runtime owner 现在在 same boundary 上 qualify release-backed Tool Task、Tool Service 与 Streamable HTTP MCP。
First prepare 仅 consume path-free verified Tool/MCP release payload 与 explicit Runtime selection
（其 provider 与 full semantics digest match committed Control authority）。
Task persist self-contained binding 而不 start unit。
Service 首先 persist `requested`，然后 retain exact Runtime 与 typed Gateway readiness evidence，
并在 delete recovery authority 之前 commit final binding。
Exact final receipt replay 而无需 Artifact access；
retained terminal provisioning record reconcile 而无需 another Runtime apply；
stop/remove 仅 use receipt-owned provider、Gateway 与 generation evidence。
Pre-effect contention deferred，invalid authority 或 immutable byte rejected，
Runtime/Gateway effect 之后所有 persistence 或 protocol ambiguity 仍 unknown。

Runtime package 现在还 expose bounded canonical `RuntimeSurfacePlan` payload 与 `CommittedRuntimeSurfaceResolver`，
在 restart 后 reconstruct full plan 并 recheck provider evidence。
此 owner 仍 qualification-only：production composition 必须 supply durable host source 与 atomic dispatcher，
而非 retain process-local selection 作为 authority。

Inactive Control composition proof 随后仅 accept registered operation identity 与 host-produced immutable plan payload。
它在 Control 内 project 所有 mutable transition field，validate exact Runtime publication 与 Grant authority，
并在 one shared installation fence 下 order plan publication before generation commit。
这 narrow cutover boundary，而不 make private kernel 或 legacy consumer production-active。

Kernel 现在还 qualify path-free external-payload registration 与 snapshot-evidence boundary。
其六个 frozen owner identity 与 fixed backup policy 对照 ACL cutover inventory 检查。
Global Artifact Store explicitly excluded，而其余五个 owner 必须 produce one complete、canonically ordered receipt set，
绑定 exact installation、Control generation、registry digest、owner schema、inventory/manifest digest 与 bounded file/byte accounting。
Decoded evidence 在其 descriptor digest 被 accept 之前 revalidated。

Private snapshot session 现在 freeze one canonical Control export 及其 digest，
同时在 owner I/O 期间 retain same exclusive maintenance fence，而不 retain SQLite transaction 或 store-executor permit。
Knowledge owner adapter snapshot scope-local OKF SQLite/FTS5 Knowledge database 到 non-overwriting bounded archive，
derive canonical binding/selection inventory digest，并 offline re-verify archive。
Live receipt issuance 与 offline acceptance 均 require snapshot binding 命名的 same canonical Control export byte。

每个 retained Knowledge incarnation 必须 originate 于其 exact Control prepare intent 与 committed OKF bundle；
applied preparation 必须 match retained Knowledge observation 与 capability-projection digest。
此 join 在 destination archive 写入前 against temporary SQLite snapshot 运行，因此 semantic mismatch 不留 archive 或 receipt。
Removed 或 missing formerly applied payload 需要 same lifecycle 的 recorded remove effect，
而 deferred outcome 仍是 safe-no-effect scheduling evidence，claimed 或 unknown outcome 仍是 evidence to reconcile；
none 是 new desired-state authority。

Absent Knowledge database 产生 explicit zero-file manifest 而不 create live directory；manifest 与 receipt 不含 host path。
Offline-verified Knowledge snapshot 现在可 stream exact database 到 caller-owned、state-root-local candidate，而不 touch live payload。
Clean-target activation 要求 exact installation 的 exclusive maintenance guard，
re-audit candidate 及其 binding/selection inventory，reject unowned、existing 或 ambiguous payload state，
并通过 one atomic rename publish。Exact completed partial replayable。
While same staged attempt 与 exclusive guard retained，publication 后、return canonical path-free result 前的 retry reconcile exact live database。
Absent payload activation 不 create Knowledge state。

Second typed adapter 现在 snapshot planning-and-diagnostic observation owner。
它 archive 仅 owner-validated terminal diagnostic history 与 terminal resolution attempt；
active resolution 与 download attempt 加 operational lock 从不 restore 为 authority。
Exact active inventory count 与 digest 仍 bound 到 manifest。
Secure bounded traversal reject link、moved 或 foreign record、unknown layout、duplicate package identity 与 file/byte overrun。
Archive creation 是 no-clobber，publication 前 re-scan live state，并 emit path-free Control-export-bound receipt 可 offline verify。

Offline-verified observation snapshot 现在 copy exact archive 到 state-root-local staging directory，而不 touch live owner path。
First activation 要求 clean terminal/active record inventory 与 exact exclusive maintenance guard，
然后在 publish any record 前 atomically change archive candidate 为 `activating` marker。
Digest-named deterministic partial 使 interrupted per-record publication replayable；
activation 开始后仅 accept exact snapshot subset。
Candidate、target、link、active-record 与 archive drift fail closed，lock 仍 excluded，canonical result 不含 host path。
两个 adapter 仍 inactive qualification code，未 wired 到 current backup 或 restore scanner。

Host protocol projection 现在是第三个 qualified snapshot 与 clean-target restore adapter。
其 owner-native scanner archive 仅 immutable request-to-plan record、optional terminal outcome，
以及每个 exact operation binding 的一个 canonical cancellation。
Operation alias 与 latest-enablement diagnostic index 仍是 derived：
它们必须 complete 并与 source request 一致，但从不 enter archive。
Bounded no-follow traversal、second live scan、no-clobber publication 与 exact offline decoding
reject linked、moved、missing、stale 或 orphaned record 与 archive substitution。

Publication 前，Host plan、completion/cancellation evidence、package identity、desired state、selected surface
与 package/capability generation 必须 derivable 从 exact bound Control export；
Host receipt 与 health evidence 仍是 observation，不能 select desired state。
Manifest 与 receipt path-free，explicitly represent absence，并 preserve no-change request 而不 fabricate operation。

Offline-verified snapshot 现在 stage private archive copy 并在 target state root 下 build one complete `plugin-host-manager` candidate。
它 restore exact semantic source byte，rebuild 仅 canonical exact operation 与 latest-enablement index，
并 deliberately omit legacy alias 与 lock file。
Activation 要求 exact target 的 exclusive maintenance guard 与 absent live owner root，
revalidate exact tree 与 owner-native semantic scan，record snapshot-bound durable activation marker，
并通过 one atomic no-clobber directory move publish entire owner root。
Archive、record 与 activation-marker partial recover deterministically；
publication 后、pre-result replay 仅 accept same exact snapshot。
Candidate、live-root、link、archive 与 marker drift fail closed，absence 不 create owner root，result 不含 host path。
此 adapter 仍 inactive qualification code。

Restore Coordinator 现在是第四个 qualified snapshot owner。
其 owner-native journal scanner archive 仅 exact、canonically encoded completed restore operation（绑定 installation）。
Active marker 及其 exact operation excluded from payload authority，
但其 bounded count 与 digest inventory 仍 manifest-bound；marker-only handoff represented 而不 invent history。
Orphaned nonterminal record、pruning 或 temporary state、unknown entry、link、foreign installation 与 path/record rebinding fail closed。
Second scan precede no-clobber archive publication，path-free receipt 与 streaming offline verifier bind result 到 exact Control export。
Empty 或 active-only history 不 create archive。

Offline-verified snapshot 现在可在 target installation state root 下 build immutable candidate。
Because current restore own same journal，activation intentionally 不是 clean-target merge：
要求 exact exclusive maintenance guard 与 active marker，preserve marker 与 current operation，且仅 replace terminal history。
Durable activation descriptor bind snapshot、stable active identity 与 exact before/target inventory。
Existing terminal directory moved 到 retained staging tombstone，然后 candidate record published 而不 replacement。
Replay tolerate active operation advancing，同时 reject marker drift、link、unknown state、candidate 或 tombstone tampering 与 unexplained live change。
Marker-only handoff 与 absent history supported。

Legacy whole-installation marker reserve active operation 的 future terminal slot，
因此 64-record source deterministically drop same native oldest record journal 会 prune 的 record。
Typed complete-set marker 无 retained operation，因此 preserve 全部 64 source record。
Canonical result path-free 且 snapshot-bound。此仍 inactive qualification code。

Runtime plan owner 现在 snapshot immutable installation-scoped plan record，
verify 其 complete key 与 canonical envelope，并在 Host projection activation 前 restore；
referenced Runtime artifact digest 也 included 于 installation artifact-reachability evidence。

Private complete-set snapshot coordinator 现在在 one exact maintenance fence 与 timestamp 下
capture canonical Control export 与全部五个 registered owner snapshot。
它 bind fixed owner set、receipt、digest、schema 与 byte accounting 于 one path-free canonical manifest，
stream 到 single no-clobber archive（位于每个 Use data 与 state root 之外），
并在 publication 前 reuse 每个 owner-native verifier audit entire staged file offline。
Absent owner contribute receipt 但不 invent payload byte；global Artifact Store 仍在 installation backup 之外。
Archive header、manifest、length、payload digest、trailing-byte、link、drift、rebinding 与 overwrite failure 均 fail closed。
此 complete-set writer 也是 inactive qualification code。

Offline-verified complete snapshot 现在可在 retain exact target exclusive maintenance fence 下，
stage Control database 与全部五个 owner candidate 于 one fixed `.control-installation-restore` directory。
One canonical path-free attempt descriptor bind snapshot、installation、owner registry、Knowledge storage policy 与 fixed component set，
于 candidate I/O 开始前。
Control candidate 必须 round-trip 到 exact canonical export，checkpoint 到 one SQLite file，并 match durable byte digest；
每个 external candidate 由 owner-native adapter 在同一 guard 下 build 与 recheck。
Present 与 absent owner、completed retry 与 interrupted Control staging deterministic，
而 nonempty target、unknown 或 linked entry、snapshot/policy rebinding 与 completed-candidate drift fail closed 而不 touch live authority path。

Complete-set coordinator 现在 qualify 整个 cross-owner activation protocol。
Durable intent 前，每个 present 或 absent owner candidate revalidated against clean target。
Immutable attempt descriptor 仍是 restore identity；`activation.json` 是 sole mutable journal；
typed global `.maintenance.restore.json` marker bind attempt 到 one immutable activation operation。
Fixed owner order 是 Control Store、Runtime plan、Host projection、Knowledge、observation，然后 Restore Coordinator。
每步使用 same journal-marker-effect-checkpoint discipline，
每个 checkpoint bind canonical path-free owner result by length 与 domain-separated digest。
Restore Coordinator receive exact expected marker byte、length 与 digest，然后才 change history。
仅 sixth durable checkpoint permit global marker retirement。

Reopening reacquire exact exclusive guard，rebind same verified snapshot、attempt、owner registry 与 Knowledge policy，
并在 exact candidate/live boundary reconstruct 或 verify 每个 owner。
Journal 与 marker partial、每个 owner effect before checkpoint、final checkpoint before marker deletion、
marker deletion 后立即 process exit，以及每个 fixed-order staging retirement 后 exit 均 deterministic converge。
21-boundary subprocess matrix exercise 那些 top-level exit。
Missing marker 仅在有 complete six-checkpoint journal 时 accepted；
ambiguous marker、out-of-order live root、snapshot rebinding、linked path 或 evidence drift fail closed。
Completed replay 执行 no owner effect；它仅 resume bounded retirement of six link-free staging tree。
Surviving canonical `attempt.json` 与 complete `activation.json` 构成 exact installation-bound terminal receipt。

Legacy backup 与 artifact reachability 仅 exclude 该 two-file receipt；
incomplete、extended、linked 或 tampered evidence fail closed。
Production Grant conversion、Runtime/Flow dispatcher composition、backup/restore command wiring、
indivisible consumer cutover 与 deletion of legacy mutable store 仍 open。

Research-preview
[MHS integration profile](docs/mhs-integration.md) 定义 hardware adapter boundary，
而不 add 另一个 package surface 或 protocol fork。

## 当前契约基线

仅接受以下 cognitive-package protocol line：

| 契约 | 当前 schema |
| --- | --- |
| Package manifest | schema version `3` |
| Registry source configuration | ACL schema version `1` |
| Signed catalog record | `a3s.use.plugin-catalog.v3` |
| Installed receipt | schema version `6` |
| Installation snapshot | `a3s.use.installation-snapshot.v2` |
| Operation plan | `a3s.use.plugin-operation-plan.v4` |
| Host capabilities | `a3s.use.plugin-host-capabilities.v6` (protocol `6`) |
| Host managed scope | `a3s.use.plugin-managed-scope.v2` |
| Host operation observation | `a3s.use.plugin-host-operation-observation-request/result.v1` |
| Host operation watch | `a3s.use.plugin-host-operation-watch-request.v1` |
| Host cancellation | `a3s.use.plugin-host-cancel-request/result.v1` |
| Manager MCP toolset | `a3s.use.plugin-manager-tools.v5` (v4 migration contract 仍可读) |
| Pending package graph | `a3s.use.pending-package-graph-operation.v4` |
| Pre-lock resolution attempt | `a3s.use.plugin-resolution-attempt.v1` |
| Pre-plan download attempt | `a3s.use.plugin-download-attempt.v1` |
| Lifecycle diagnostic | `a3s.use.plugin-lifecycle-diagnostic.v1` |
| Operation diagnostic | `a3s.use.plugin-operation-diagnostic.v1` |
| Operation history | `a3s.use.plugin-operation-history.v1` / `a3s.use.plugin-operation-history-diagnostic.v1` |
| Pre-lock resolution diagnostic | `a3s.use.plugin-resolution-attempt-diagnostic.v1` |
| Pre-plan download diagnostic | `a3s.use.plugin-download-attempt-diagnostic.v1` |
| Enablement recovery projection | `a3s.use.cognitive-package-enablement-projection.v3` |
| Enablement operation | `a3s.use.cognitive-package-enablement-operation.v3` |
| Extension Registry snapshot | schema version `3` |
| Extension snapshot cursor | `a3s.use.extension-snapshot-cursor.v3` |
| Capability snapshot | schema version `5` |
| Capability snapshot cursor | `a3s.use.capability-snapshot-cursor.v4` |
| Capability descriptor | `a3s.use.capability-descriptor.v1` |
| Signed capability description | `a3s.use.capability-description-signature.v1` (Ed25519) |
| Control descriptor evidence snapshot | `a3s.use.control-capability-descriptor-snapshot.v1` (proof-only compatibility) / `v2` (signed envelope) |
| Control descriptor snapshot retention plan | `a3s.use.control-capability-descriptor-snapshot-retention-plan.v1` |
| Control descriptor snapshot retention result | `a3s.use.control-capability-descriptor-snapshot-retention-result.v1` |
| Control descriptor snapshot retention journal | `a3s.use.control-capability-descriptor-snapshot-retention-journal.v1` (internal) |
| Control descriptor snapshot restore plan | `a3s.use.control-capability-descriptor-snapshot-restore-plan.v1` |
| Control descriptor snapshot restore result | `a3s.use.control-capability-descriptor-snapshot-restore-result.v1` |
| Capability Gateway catalog | `a3s.use.capability-gateway-catalog.v1` |
| Capability Gateway catalog restore plan | `a3s.use.capability-gateway-catalog-restore-plan.v1` |
| Capability Gateway catalog restore result | `a3s.use.capability-gateway-catalog-restore-result.v1` |
| Capability payload restore plan | `a3s.use.control-capability-payload-restore-plan.v1` |
| Capability payload restore result | `a3s.use.control-capability-payload-restore-result.v1` |
| Capability payload retention plan | `a3s.use.control-capability-payload-retention-plan.v1` |
| Capability payload retention result | `a3s.use.control-capability-payload-retention-result.v1` |
| Capability payload retention coordinator journal | `a3s.use.control-capability-payload-retention-journal.v1` (internal, restart-recoverable phase boundary) |
| Capability consumer profile | `a3s.use.capability-consumer-profile.v1` |
| Capability consumer negotiation | `a3s.use.capability-consumer-negotiation.v1` |
| Runtime Task binding | `a3s.use.runtime-task-binding.v4` |
| Runtime Service provisioning | `a3s.use.runtime-service-provisioning.v1` |
| Runtime Service binding | `a3s.use.runtime-service-binding.v3` |
| Artifact Store physical inventory | `a3s.use.artifact-store-inventory.v1` |
| Artifact Store digest audit | `a3s.use.artifact-store-digest-audit.v1` |
| Artifact quarantine plan | `a3s.use.artifact-quarantine-plan.v1` |
| Artifact quarantine record | `a3s.use.artifact-quarantine-record.v1` |
| Artifact quarantine result | `a3s.use.artifact-quarantine-result.v1` |
| Artifact rehydration plan | `a3s.use.artifact-rehydration-plan.v1` |
| Artifact rehydration record | `a3s.use.artifact-rehydration-record.v1` |
| Artifact rehydration result | `a3s.use.artifact-rehydration-result.v1` |
| Registry artifact reference inventory | `a3s.use.registry-artifact-reference-inventory.v1` |
| Global artifact reference inventory | `a3s.use.artifact-reference-inventory.v1` |
| Joined artifact reachability inventory | `a3s.use.artifact-reachability-inventory.v1` |
| Coordinated Use state backup | `a3s.use.state-backup.v2` |
| Coordinated Use state backup retention plan | `a3s.use.state-backup-retention-plan.v2` |
| Coordinated Use state backup retention result | `a3s.use.state-backup-retention-result.v2` |
| Coordinated Use state restore plan | `a3s.use.state-restore-plan.v1` |
| Coordinated Use state restore operation | `a3s.use.state-restore-operation.v1` |
| Coordinated Use state restore result | `a3s.use.state-restore-result.v1` |
| Coordinated Use state restore diagnostic | `a3s.use.state-restore-diagnostic.v1` |
| OKF Knowledge search | `a3s.use.okf-knowledge-search-request.v1` / `a3s.use.okf-knowledge-search-response.v1` |
| OKF Knowledge citation | `a3s.use.okf-knowledge-citation.v1` |
| OKF Knowledge read | `a3s.use.okf-knowledge-read-request.v1` / `a3s.use.okf-knowledge-read-response.v1` |
| OKF Knowledge backup | `a3s.use.okf-knowledge-backup.v1` |
| OKF Knowledge backup retention plan | `a3s.use.okf-knowledge-backup-retention-plan.v1` |
| OKF Knowledge backup retention result | `a3s.use.okf-knowledge-backup-retention-result.v1` |
| OKF Knowledge restore plan | `a3s.use.okf-knowledge-restore-plan.v2` |
| OKF Knowledge restore operation | `a3s.use.okf-knowledge-restore-operation.v2` |
| OKF Knowledge restore result | `a3s.use.okf-knowledge-restore-result.v2` |
| OKF Knowledge restore diagnostic | `a3s.use.okf-knowledge-restore-diagnostic.v2` |

SemVer dependency constraint、`requires_use`、OS/target check 与 host/provider capability check 是 product behavior，
而非 backward-compatibility branch。
较旧的 pre-release schema 与 persisted state deliberately 不 migrated。
删除 unsupported state 并用 current build reinstall。

## 实现状态

Gateway embedding host 可通过
`CapabilityRegistrySnapshot::capability_gateway_catalog` 从一个
`CapabilityRegistrySnapshot` 派生 consumer-specific catalog；helper 在
`CapabilityGatewayMcpServer::from_registry_snapshot` acquire RAII lease 前
verify public projection revision 加 exact package/publication/readiness evidence。

对 live host，`from_verified_registry_snapshot_with_factory_and_options`
现在在 one constructor 中 compose verified description projection、cursor-bound resolver、
exact RAII lease、consumer negotiation 与 bounded admission policy；publication race 返回 no server。
Signature verification 与 receipt/Runtime/Grant-backed opaque-reference resolution 仍 host-owned，
product wiring 仍 open。

Inactive Control composition 现在对其自身 cursor 有 equivalent authority join：
`ControlCapabilityGatewayInvocationFactory` 在 descriptor byte-for-byte compare 与 durable catalog 后
receive exact reopened Control lease，`CapabilityGatewayResolvedProvider` 在整个 Tool、Resource 或 Prompt operation 期间 retain 该 lease。
这使 opaque-reference resolution 在 Control generation 上，而非 accidentally fallback 到 legacy Registry resolver；
host factory 仍 own private Grant/Runtime/provider binding，production activation 仍 open。

Gateway 现在有 typed consumer boundary。`CapabilityConsumerProfile`
区分 default generic MCP client 与 explicit A3S consumer，
而 `CapabilityConsumerNegotiation` 将 sorted、digest-bound extension set 绑定到 Gateway，
并 reject unsupported request 而非 silently downgrade。
Existing constructor 默认仍为 generic-MCP。Profile label 仅是 metadata。
Descriptor 可 declare canonical `requiredExtensions`，Gateway 在 compile discovery 或 invocation route 前
remove negotiated consumer 未 accept 的 requirement。

Standard adapter publish catalog-authorized、schema-validated MCP Tool
加 bounded opaque-URI Resource 与 declared Prompt；每个 discovery list deterministic 且 cursor-paginated。
Discovery cursor opaque，绑定 MCP surface、negotiated catalog digest 与 frozen principal visibility view，
因此 replaced publication 的 cursor fail closed 并 stale-cursor signal，而非 silently skip 或 repeat capability。
Host 可 inject `CapabilityGatewayDiscoveryPolicy` freeze principal-scoped Tool/Resource/Prompt visibility
per authenticated context；denied route 从 discovery 与 direct access 消失，而 provider per-operation authorization 仍 mandatory。
Existing constructor retain allow-all compatibility policy，因此 production multi-principal host 必须 explicitly opt in。
Flow/Knowledge/UI payload projection 与 production host composition 仍是 separate gate。

Adapter 还 consume rmcp per-request cancellation：cancel in-flight Tool、Resource 或 Prompt
会 drop provider future 及其 short-lived admission/resolver lease，
并在 protocol 仍可 deliver 时给出 typed secret-free cancellation result。
参见 [Capability consumer profiles](docs/capability-consumer-profiles.md) 了解 contract 及其 limit。

Agent-visible description 也可 cross explicit cryptographic trust boundary。
`a3s-use-core` 定义 canonical、domain-separated `SignedCapabilityDescription` envelope；
`a3s-use-extension` 用 bounded public-key trust store verify Ed25519 signature，enforce key rotation、expiry 与 revocation。
Gateway 现在 expose signed-description composition constructor，在 take Control snapshot lease 或 provider resolver 前 verify 每个 envelope。
Private `VerifiedCapabilityDescription` wrapper retain exact replay byte，restore 后必须 reverify。
Trust-store source 仍 host-supplied，此 path 尚未 wired 到 official Registry/TUF source 或 production Control lifecycle。
参见 [Capability description signatures](docs/capability-description-signatures.md)。

Gateway 还 expose shared、bounded `CapabilityGatewayNotificationHub`。
Client initialize 后，host 可 publish newer immutable catalog key 并 concurrently fan out standard MCP
`tools/list_changed`、`resources/list_changed` 与 `prompts/list_changed` notification。
Repeated 或 older publication key coalesced，closed 或 back-pressured peer retired。
这是 notification seam，非 mutable catalog：host 必须 switch new session 到 replacement server，
并 retain prior generation lease 直到 drain。
Session-factory replacement 带 new discovery-policy snapshot 也 treated 为 view change，
因此 initialized client 即使 source publication key unchanged 也 receive same notification。

需要 restart-safe ownership of Agent-facing payload 的 host 可使用 `CapabilityGatewayCatalogStore`。
它 validate installation binding 与 canonical catalog byte，
在 bounded SHA-256 content-addressed layout 下 store record，
使用 no-follow file check 加 deterministic staging 与 hard-link publication，
并 expose exact `get`、`get_exact` 与 bounded inventory read。
Store by design 无 mutable「current」pointer：Control/lifecycle cutover 必须 bind returned digest 到 committed generation，
并 retain corresponding session lease。

Inactive Control composition 现在 qualify 该 hand-off：host-owned、side-effect-free projector 仅 receive committed capability authority；
concrete owner validate 每个 projected descriptor against enabled package incarnation 与 terminal surface evidence，
durably publish catalog 与 Capability Index，然后 return both identity 作为 one typed application。
Recording applied observation atomically advance published Control cursor 及 catalog digest、generation 与 revision。
Live admission reopen 那些 exact byte 后再 take package-generation lease。

Strict descriptor projector 现在在 explicit package-scoped signer allowlist 下 consume host-verified signed proof，
check exact catalog surface dependency、terminal owner-specific receipt evidence、active Grant coverage
与 reviewed Tool/MCP workload shape，然后 derive opaque route reference。
它 intentionally 是 pure subset projection。

Installation-owned descriptor snapshot store 现在 support signed v2 admission path：
publication 前 verify 每个 canonical Ed25519 envelope，retain exact envelope beside derived proof projection，
restart projection 时对 current trust store 与 clock re-verify envelope。
Legacy v1 proof-only path 仍是 explicit compatibility mode，不能 downgrade signed v2 record。
Snapshot file content-addressed by canonical byte（而非 mutable key），
published 带 bounded no-follow staging/no-clobber replay，每次 restart read revalidated；
missing snapshot 是 safe retry，substitution、tampering、expiry 或 revocation rejected。

Coordinated state backup 现在仅 admit exact content-addressed catalog 与 descriptor-snapshot record
并 validate canonical owner byte；replay 仍 recheck signed envelope against current trust policy。
这仍是 qualification code：cryptographic key-source binding 到 official Registry/TUF metadata、
production Control/Runtime/receipt wiring 与 clean-target restore activation 仍是 host gate。

Runtime Tool release planning 现在 carry canonical input/output schema attestation 通过 plan、binding receipt 与 Control evidence；
verified artifact admission 与 strict descriptor projection compare same descriptor 与 schema digest。
Production Control activation、lifecycle-selected retention policy 与 retirement coordination 仍是 separate gate；
owner-native restore 与 retention coordinator 是 qualification boundary，直到该 authority composed 到 live host。
Retention 现在在 unlink 前 record durable paired-owner phase journal，backup/reachability refuse run 直到 pending journal recovered。

Embedding boundary 现在还 include `CapabilityGatewaySessionFactory`：
durable publication 后，host 可按 order replace immutable Gateway generation，
retain one standard MCP notification hub，keep old in-flight operation 在其 exact lease 上，
而 later request 于 same endpoint observe new catalog。
其 bounded `drain` transition close new request admission，wait already-admitted operation under deadline，
并 release factory source lease 以便 lifecycle owner enter exclusive retention 或 restore fence。

`from_published` 与 `replace_published` path re-read exact store publication
并 verify negotiated consumer projection 与 complete source catalog，然后 source 才 visible。
Replacement 使用 conditional source swap，因此 concurrent local cutover 在 publication verification 后不能 overwritten。

Inactive Control composition 还提供 `reopen_published_capability_gateway` 与 `replace_published_capability_gateway`：
两者从 durable Control authority derive lease，retain 于 immutable Gateway server 内，并 reject unleased replacement。
Successful Control-bound drain retain one-shot typed endpoint identity，
因此 exact shutdown retry 在 source lease detached 后仍 idempotent，
而 directly drained 或 copied unleased catalog 仍 rejected。
Conditional replacement 还 refuse overwrite newer local cutover 以 stale same-generation build。
Production Control activation、provider composition、retirement 与 retention coordination 仍是 host responsibility。

Session identity derived 从 complete immutable source publication before consumer negotiation，
因此 filtering optional descriptor 不 break Control lease binding 或 lifecycle reconciliation。
Upgrade 期间，reconciliation validate existing endpoint 的 prior Control lease against 其 own source identity，
然后 swap newly acquired publication lease。

Catalog payload cleanup 现在也是 explicit plan/apply operation：
`CapabilityGatewayCatalogStore` 要求 lifecycle-supplied protected digest set，
在其 mutation lock 下 revalidate canonical inventory，并 remove 仅 reviewed complement 带 durability check。
Destructive owner apply 现在 take installation exclusive maintenance fence，
因此 live Control-backed snapshot/Gateway lease 不能 pruned around。

Inactive Control composition 添加 `plan_published_capability_payload_retention` 与 `apply_published_capability_payload_retention`：
它们 derive durable published catalog（及 present 时 matching descriptor snapshot），
在该 exclusive fence 下 recheck cursor，并 reject 会 remove 它的 plan。
`drain_and_retain_published_capability_gateway` 为 shutdown 与 retirement path compose endpoint drain 与该 plan/apply sequence。
Host 仍 explicitly add independently managed rollback 或 legacy endpoint digest；store 从不从 in-memory pointer guess liveness。

同一 owner 现在通过 `plan_clean_restore` 与 `apply_clean_restore` expose plan-bound clean-target restore primitive。
Caller confirm canonical plan digest 并 supply exact catalog set；
adapter stage 并 verify complete owner directory，record durable activation marker，并用 no-clobber directory move publish。
Existing owner state 从不 merged 或 replaced，foreign staged plan rejected，retry 可 replay durable candidate。
这是 owner-native building block：Control registration、signed descriptor restoration、session drain
与 production rollback orchestration 仍 belong lifecycle host。

Control descriptor-snapshot owner 现在 supply corresponding clean-target adapter。
Plan bind 每个 snapshot digest 到 key digest、Control generation、canonical byte count 与 signed/proof-only mode；
apply recheck exact set，对 signed v2 record 在 staging 前 require current `CapabilityDescriptionTrustStore` 与 clock。
Candidate 与 activation evidence replayable，publication no-clobber。
`ControlCapabilityPayloadRestoreCoordinator` 在 single exclusive fence 下 compose 两个 owner plan 并按 fixed order replay；
owner publication 之间的 process stop recoverable by replay same plan。
这是 ordered、recoverable activation 而非 cross-directory atomic rename。

`ControlCapabilityPayloadRetentionCoordinator` 在 one exclusive fence 下 compose corresponding retention plan，
在 first unlink 前 verify 两个 inventory，并按 catalog → descriptor order resume exact owner journal。
这是 recoverable ordered deletion 而非 cross-directory atomic transaction。

Inactive Control composition 现在 supply restart-safe cursor-reopen boundary 与 cursor-bound retention plan/apply entry point；
production owner registration、live Gateway session replacement from that lease、
lifecycle invocation of drain-and-retain boundary 与 rollback authority 仍在 these store 之外。

Control descriptor snapshot 通过 `plan_retention`、`apply_retention` 与 `recover_retention` expose same owner-level contract。
Plan embed complete protected/removal partition，每个 unlink checkpointed 于 bounded canonical journal，
pending journal block publication 与 read，non-empty inventory 必须 retain 至少 one snapshot。
Production Control registration、lifecycle activation 与 trust-source selection
仍 supply authority 以 choose 与 reopen descriptor generation。

Paired `ControlCapabilityPayloadRetentionCoordinator` 添加 cross-owner boundary：
两个 inventory preflighted 于 one exclusive maintenance fence before catalog record removed，
然后 descriptor snapshot removed 于 fixed order；owner 之间 interruption resumed by replay same plan。


| 领域 | 状态 |
| --- | --- |
| 六表面 ACL 包契约 | 已实现并有 fixture 支撑 |
| MHS research-preview 适配器 profile | A3S Use 边界、least-authority ceiling、exact managed-MCP publication gate、dependency graph 与 no-implicit-write-retry 规则已文档化并 contract-tested。这不是 MHS 实现或 protocol-conformance 声明 |
| Signed catalog-v3、TUF verification、durable replaceable Registry source 与 opt-in public-endpoint SSRF policy | 在 engine 与 standalone CLI 中已实现；managed host 必须为 untrusted tenant endpoint 选择 strict policy |
| Shared Plugin Manager service、CLI、TUI 与 manager MCP | Typed application service 实现 search、inspect、stable installed listing、status、install/upgrade/uninstall 与 enable/disable planning、durable plan reopening、digest-only apply、exact operation observation/watch，以及 one Host Manager 上的 trusted pre-admission cancellation。Standard MCP adapter 暴露 thirteen-tool v5 inventory，mutation 或 cancellation 要求 injected trusted confirmation evidence。Standalone Registry-backed compatibility mutation 使用 service 而不 break existing JSON field，而 one-to-one `plugin` CLI 暴露全部十三个 operation、exact typed result、explicit digest-bound `--yes` apply/cancellation、durable replay 与 zero-network cached apply。A3S Code CLI、TUI `/packages` 与 product-host manager MCP compose 同一 service。Human CLI/TUI presentation 现在从 immutable envelope derive exact plan、graph、source、permission、operation status 与 confirmation boundary 而不改变 machine JSON；product-host E2E 仍 open |
| Capability Gateway contract 与 embedding MCP adapter | 已实现并 contract-tested：immutable path-free descriptor/catalog contract、opaque invocation/artifact/endpoint/resource reference、exact snapshot-lease 与 publication/lifecycle-generation binding、typed generic-MCP/A3S consumer-profile negotiation（canonical digest 与 no-silent-downgrade semantics）、route compilation 前 negotiated `requiredExtensions` catalog projection，以及 standard MCP `CapabilityGatewayMcpServer`（仅 route catalog-authorized Tool、Resource 与 Prompt 通过 injected `CapabilityGatewayInvocationProvider`）。Tool、resource 与 prompt discovery deterministic、bounded 且 cursor-paginated；resource read 要求 exact opaque URI；prompt argument closed against reviewed declaration；provider output bounded、path-free 且 catalog-linked。Host 可 inject bounded `CapabilityGatewayDiscoveryPolicy` freeze principal-scoped visibility 于 list 与 direct-access method，同时 retain provider authorization 为 separate gate。Host 可在 `/mcp` expose Streamable HTTP（bearer authentication、optional exact Origin policy、duplicate-header rejection、bounded in-flight/rolling-window admission、sanitized HTTP error、explicit pre-operation authorization hook 与 typed host-authenticated transport/principal context）。`CapabilityGatewayInvocationResolver` 与 `CapabilityGatewayResolvedProvider` 为 opaque reference 提供 single-resolution、lease-scoped host path；returned handle 必须在每个 operation 期间 retain exact package-generation lease。`CapabilityGatewayMcpServer::from_verified_registry_snapshot_with_factory_and_options` 将 verified catalog、same-cursor resolver、snapshot lease、negotiation 与 admission policy compose 为 one fail-closed construction boundary。`CapabilityGatewayNotificationHub` 将 immutable publication change bridge 到 standard MCP list-change notification，`CapabilityGatewayCatalogStore` 提供 bounded、canonical、content-addressed、restart-safe payload ownership（exact read、no mutable current pointer、plan-bound retention 与 strict clean-target restore adapter 及 durable activation replay）。Control descriptor-snapshot owner 现在提供 matching plan-bound restore 与 signed-v2 trust revalidation。Inactive Control kernel 现在 atomically bind 该 payload identity 到 applied capability cutover 与 exact published cursor；composition 可 reopen cursor 并 seed 或 replace live Gateway session 同时在 every server clone retain Control lease。Coordinated backup inventory validate 并 archive catalog/descriptor-snapshot record 于 one explicit `CapabilityPayloads` family。Independent Rust client discovery/invocation check 覆盖 path-free boundary。Production Control activation、owner registration、live lifecycle wiring、lease drain/retention coordination、complete receipt/Runtime/Grant-backed descriptor projection、CLI wiring、TLS termination 与 TypeScript/Python client/recovery matrix 仍 open |
| Registry target observation、explicit offline install/upgrade、bounded source working set、resumable download、usage 与 confirmed source cleanup | 已实现，含 interruption、range、tamper 与 zero-network test；cleanup 从不 claim global blob reclamation |
| Global raw-blob 与 expanded-package Artifact Store | Raw verified target 与 expanded tree 在 one global root 下按 SHA-256 分片，在 cross-process digest lock 下 commit，link/reparse checked，跨 Registry source 与 installation shared，在 source prune 与 scoped uninstall 后 retained，excluded from installation backup。Store-bound shared/exclusive reference boundary 防止 maintenance 或 whole-installation restore race durable reference publication。Physical、Registry-reference、global-reference 与 joined-reachability v1 evidence 覆盖 canonical content、staging、每个 durable owner、expectation mismatch 与 checked storage usage。Optional canonical hard quota、full digest audit、exact-plan logical quarantine 与 verified zero-reference rehydration 仍是 separate authority。Confirmed GC 现在仅 accept bounded explicit Blob/expanded-package digest allowlist，repeat complete zero-reference proof，bind physical 与 lifecycle evidence 加 predecessor completion 到 one canonical plan，并在 same-shard atomic retirement 与 bounded tombstone deletion 前 persist global fail-closed fence。Terminal replay read-only 且不能 delete later recreated object。Source prune、scoped uninstall、audit、quarantine、rehydration、quota pressure 与 unreachability 从不 independently authorize global deletion |
| Signed native Tool/stdio MCP planning 与 post-download manifest binding | 已实现并 contract-tested |
| Bounded SemVer dependency resolution 与 exact lock | 已实现 |
| Install、upgrade、uninstall graph ordering | 已实现 |
| Durable atomic Registry cutover 与 exact replay | 已实现 |
| Package-host side-effect/receipt ambiguity recovery | 每个 canonical install、upgrade、enable、disable 与 uninstall checkpoint 通过 subprocess-exit、exact-key recovery、single-effect 与 terminal-replay test。Real CLI multi-node install 还通过 durable-publish-before-journal kill、zero-network exact replay 与 no-generation-inflation check；uninstall 通过 equivalent hide、restart、accepted-call drain 与 removal boundary。Product-host 与 platform checkpoint 仍 open |
| Grant-bearing graph cutover effect/receipt ambiguity recovery | Install、upgrade 与 uninstall atomic publish/hide boundary 通过 subprocess-exit、exact-key recovery、single-effect、completed-journal 与 no-republication test。Externally killed managed-scope manager process 证明三个 five-node graph cutover recover 而无 reauthorization、network access 或 generation inflation，同时 preserve candidate Grant 并仅 retire exact prior Grant。Real Host protocol process additionally 证明 disable hide/drain/exact-revocation 与 enable publication/exact-regrant recovery，覆盖全部五个 reviewed mutation。Actual Code/Runtime product-host 与 cross-platform qualification 仍 open |
| Grant Store journal/receipt crash recovery | Canonical two-candidate/two-retirement lifecycle 全部 14 durable checkpoint 通过 subprocess-exit convergence 与 exact terminal replay（跨 prepare、cutover/retirement 与 pre-cutover rollback）；real CLI 与 cross-platform product qualification 仍 open |
| Windows atomic state publication contention | Registry source/trusted-root/catalog/target-cache、extension receipt/snapshot、Workspace Grant、package graph、Host plan/outcome、lifecycle、Runtime binding/provisioning、Flow、Knowledge binding/recovery/backup、enablement、whole-state backup、restore evidence 与 diagnostic-history publication 现在 share bounded blocking primitive（replace、no-clobber 与 transactional directory-move semantics）。Windows 仅 transient access、sharing 与 lock violation 最多 retry 两秒；released file 或 directory lock converge atomically，persistent replacement lock preserve prior target，failed recovery move retain replay source。Native lifecycle test 还将 active artifact-staging rename 与 selected upgrade-receipt replacement contention bind 到 pre-publication rollback 与 replay，而 uninstall 从不 wait reader of global artifact byte。Externally raced target、reboot recovery 与 external product-host contention 仍 open |
| Secret-free operation diagnostics | 已实现。Latest/previous package checkpoint 通过 `extension inspect --json` 暴露；`extension diagnose --json` project 一个 exact retained planned/admitted/cancelled install/upgrade/uninstall graph、active admitted enable/disable operation，或 newest Host-reviewed pre-admission enable/disable plan/cancellation（含 Registry/TUF、provider、Grant、cutover、publication、drain、rollback 与 recovery evidence）。`extension diagnose --history --json` 每 scope/package 在 8 MiB 内 retain newest 16 completed 或 rolled-back operation 与 cancelled graph plan，survive uninstall，deduplicate exact replay，damaged 或 linked state fail closed。Pre-lock Registry/TUF attempt 暴露 refreshed/cached per-Registry verification progress、trust/source digest、role version、bounded failure 与 terminal lock evidence。Retained graph 与 pre-plan attempt 从 historical provenance 暴露 zero-network expected/retained archive 与 executable-planning-target byte 加 exact-target `missing`/`partial`/`complete` state。Real killed-process 与 Host-process test 覆盖每个 handoff、partial observation、exact resume、zero-side-effect planned/cancelled enablement diagnosis 与 completed-Use outcome suppression。Path-free active/history/capacity restore evidence 通过 `knowledge restore-status --json` 暴露 |
| Watcher-safe bounded Registry mutation locking | 已实现并 real-process tested |
| Plan-v4 reviewed enable/disable 与 terminal `NoChange` | 在 manager contract 与 package engine 中已实现 |
| Typed managed Host Manager | `CognitivePackageHostManager` 实现 host protocol v6（explicit User/Workspace scope-kind binding、exact capability/fence validation、persisted plan/apply replay、selected-surface evidence、durable operation observation/watch、pre-admission cancellation、Registry provenance revalidation、从 exact planning cache zero-network install/upgrade apply、graph 与 enablement delegation、fail-closed expired-plan recovery from Use-owned admission/completion evidence）。Operation storage 按 exact plan digest 区分 repeated lifecycle operation ID，同时 retain legacy lookup alias；terminal outcome 仅在其 Use-owned graph、lifecycle completion 与 package state 仍 match 时 replayed。Same textual ID 于不同 scope kind retain distinct Host plan、installation snapshot、capability cursor、invocation lease 与 replay record；complete two-installation lifecycle matrix reject substitution 并在 upgrade 与 uninstall 期间 preserve opposite installation。Killed real Host protocol install、upgrade、uninstall、disable 与 enable apply 在 Registry offline 下 recover，converge exact Grant 而无 generation inflation，并 persist one terminal outcome；injection 到每个 external managed host 仍 open |
| Workspace Grant composition 与 drain-before-revoke | 在 core/standalone lifecycle path 中已实现 |
| Mixed native/managed provider planning | 在 Use 与 shared A3S host path 中已实现：unbound draft、assigned-provider preflight、host policy、canonical Grant-bound final selection、durable planning bundle/Grant snapshot/provider generation、exact apply-time reconstruction、restart replay 与 provider-drift rejection 已测试 |
| Exact published-generation Knowledge lease | 在 Use Registry 与 SQLite Knowledge host 中已实现。Acquisition bind complete capability projection 到 installed package、manifest、OKF bundle、lifecycle generation 与 generation lock；one lease retain generation 于 cited search/read，hide 后 reject new call，参与 drain，package 或 retained-content drift fail closed。A3S Code consumption 仍是 external integration task |
| Standalone Task、stdio MCP、explicit A3S Flow preflight、Skill/UI 与 SQLite/FTS5 OKF host | 已实现 |
| Managed Runtime receipt lifecycle | Self-contained release-backed Task template 支持 restart-safe exact-generation dispatch、receipt-owned provider reconnection、stale-generation rejection 与 accepted-call drain。Capability snapshot v5 仅 publish exact installation/package/generation-matched Task binding 与 stable host tool identity。Service preparation 现在在 Runtime apply 前 sync v1 provisioning receipt，通过 exact Runtime 与 Gateway evidence advance，并在 delete pending recovery authority 前 commit v3 binding。Tool 与 HTTP MCP bind failure、pre-apply rollback、candidate cleanup 与 final-binding/pending-receipt crash window replay 而无 second Runtime effect 或 residue。Test-binary subprocess matrix 在 Tool 与 HTTP MCP 全部六个 nested provisioning window exit，然后 prove exact replay、terminal idempotence 与 residue-free Gateway/Runtime removal。Typed endpoint、drain-before-stop、route-remove-before-Runtime-remove、exact prior-generation retirement 与 stopped-binding reauthorization contract-tested。A3S CLI `main` commit `563e7e139740e845369f9102a2d47026733797a8` 通过 production Box mapping、retained N/N+1 routing、standard MCP initialize、Gateway 与 lifecycle-host restart、drain、exact removal 与 zero-residue check qualify 四个 real Linux Tool 与 MCP process。Confirmed same-generation provider loss 现在仅 retire stale Gateway route 与 old binding receipt，然后 exact Runtime reapply 并 publish newly allocated Gateway endpoint；interrupted route removal retain replay authority 而不 stop 或 remove Runtime unit。Scoped Code Exec Task discovery 与 leased invocation 在 A3S CLI `main` commit `e77d318beba3cba7f193da8d83bb9ac5c46fc0f7` 与 CI run [32797862154](https://github.com/A3S-Lab/CLI/actions/runs/32797862154) qualified。Real provider-process kill qualification、non-Linux provider 与 cross-platform product-host recovery 仍 open |
| Scope-bounded OKF quota、retention、tombstone GC、SQLite compaction 与 usage diagnostics | 在 standalone Knowledge backend 中已实现 |
| Scope-local OKF integrity audit、verified database backup 与 rotation、derived FTS repair 与 authority-bound database/binding restore | Verified backup 现在使用 exact-scope、bounded oldest-first retention（canonical plan-digest confirmation、last-backup preservation、directory locking、stale-plan rejection 与 fail-closed candidate validation）。Restore real-process tested，包括 missing database 与 missing exact-subset binding recovery、conflict rejection、main/WAL/SHM retention、binding-file 与 filesystem/journal process-exit window、durable maintenance blocking、每个 window 的 path-free restore-status diagnostic 与 terminal read-only replay。Missing Registry/package/lifecycle/Grant authority、clean-machine、coordinated cross-family 与 whole-product recovery 仍 open |
| Coordinated whole-installation backup、retention 与 reviewed restore | Backup 与 retention 在 exclusive maintenance fence 下实现（deterministic path-free manifest、exact Registry/receipt authority digest、allowlisted control-state family、explicit global Artifact Store exclusion、scan/copy/rescan consistency、full payload verification、exact-plan retention 与 two-generation preservation）。Capability Gateway catalog 与 descriptor-snapshot record 现在进入 strict content-addressed `CapabilityPayloads` family；lock、staging 与 retention journal fail closed 为 nonterminal evidence，Artifact Reachability traverse same owner tree 而非 silently ignore nested drift。Same-version/OS/architecture restore 现在 require exact independently retained Registry、Artifact 与 Grant authority、explicit verified rollback archive、path-free digest confirmation、link/reparse-safe candidate staging、seven durable journal phase、15 subprocess-exit recovery boundary、terminal replay、read-only status 与 bounded crash-recoverable history。Production owner-native clean-target activation/retention、missing-authority 与 clean-machine recovery，以及 cross-platform operational disaster-recovery drill 仍 open |
| Runtime Service、HTTP MCP、managed Knowledge recovery/rollback 与 sandboxed UI composition 于每个 declared host | 进行中 |
| A3S Code CLI/TUI integration | Reviewed Runtime Task install、offline restart disable/re-enable、apply-time build drift rejection、watcher hot-plug、Host status-revision resumption 跨 killed-process offline recovery（one effect 与 path-free history）、scoped Code Exec agent discovery/invocation（frozen Task-catalog evidence）、context review 与 TUI `/packages` review 已测试。Shared Host Manager 现在还 qualify signed six-surface Tool/MCP/Flow/Skill/UI/OKF install、invocation evidence、exact-generation upgrade、uninstall、replay 与 User/Workspace scope fence；six-surface Code product-host E2E 与 release qualification 仍 open |
| Verified preview installer 与 release evidence | Linux/macOS 与 Windows installer enforce HTTPS、exact tag-identity Sigstore verification、release checksum、safe extraction、packaged OCR/Skill binding、versioned atomic activation、complete-tree reinstall validation、retained local evidence 与 managed command ownership。Deterministic archive serialization、per-platform SPDX SBOM、GitHub OIDC provenance/SBOM attestation 与 pinned Action/tool 已实现。Qualification run [33651777660](https://github.com/A3S-Lab/Use/actions/runs/33651777660) 从 exact `main` commit `4f6e4725205d06ab81f8ea98bfee85c7eb4b2bcd` 在五目标上通过 isolated archive execution 与 cache-free byte-for-byte rebuild；stale-core `v0.3.5` publication attempt 未创建 Release。Release workflow [33687297386](https://github.com/A3S-Lab/Use/actions/runs/33687297386) 为 tag `v0.3.7` at exact `main` commit `48a0b76f8a4a87a11d16627c7bd7567920852508` 通过全部 13 job 并发布 verified archive、typed crate（`a3s-use-core 0.2.6`、`a3s-use-extension 0.3.7`、`a3s-use 0.3.7`）、SBOM、attestation 与 installer。Release workflow [33720485826](https://github.com/A3S-Lab/Use/actions/runs/33720485826) 为 tag `v0.3.8` at exact `main` commit `6d3a7baf32ce998a2e487c40fbf78b4a6cda2579` 通过全部 13 job 并发布 verified archive、typed crate（`a3s-use-core 0.2.7`、`a3s-use-extension 0.3.8`、`a3s-use 0.3.8`）、SBOM、attestation 与 installer。Release workflow [33756618837](https://github.com/A3S-Lab/Use/actions/runs/33756618837) 为 tag `v0.3.9` at exact `main` commit `a5f3cc40bfb0a1021ca150d2ce4295409b74d220` 通过全部 13 job 并发布 19 verified release asset、typed crate（`a3s-use-core 0.2.7`、`a3s-use-extension 0.3.9`、`a3s-use 0.3.9`）、SBOM、attestation 与 installer。Release workflow [33791616307](https://github.com/A3S-Lab/Use/actions/runs/33791616307) 为 tag `v0.3.10` at exact `main` commit `c4c80a223bfff3698ca4b4598e7175c6e3303239` 通过全部 13 job 并发布 19 verified release asset、typed crate（`a3s-use-core 0.2.8`、`a3s-use-extension 0.3.10`、`a3s-use 0.3.10`）、SBOM、attestation 与 installer。Prior `v0.3.6`、`v0.3.7`、`v0.3.8` 与 `v0.3.9` release 仍是 historical evidence；externally operated full-archive witness 与 off-Release evidence retention 仍 open |
| Complete Linux/macOS/Windows real-process E2E 与 recovery matrix | Release blocker |
| Public Registry operation、external full-archive reproducibility witness、off-Release evidence retention、support runbook | Release blocker |

**Production-ready：否。
** 代码有 substantial tested foundation，
但上述 unfinished row 仍是 required release gate。
[ROADMAP.md](ROADMAP.md) 跟踪剩余 product work，
而不将 completed internal 转为 release claim。

## 平台支持

| 目标 | 当前门 | 产品状态 |
| --- | --- | --- |
| Linux x86_64 / arm64 | 完整 A3S Use workspace CI 加 release-container conformance | 开发预览 |
| macOS arm64 / x86_64 | 当前 A3S Use workspace build 与 test | 开发预览 |
| Windows x86_64 | 当前 A3S Use workspace test、native linked-state qualification（跨 Registry/cache、package graph/diagnostics、lifecycle/Runtime/Flow、backup/restore 与 OKF path）、scanner-lock blob publication/source-cleanup/package-commit/upgrade-receipt/lifecycle-removal recovery、signed Registry/graph/Grant/Flow/OKF CLI lifecycle 与 killed-process cutover replay | 预览；完整 runtime/recovery matrix 待定 |

Native CI run [32604181662](https://github.com/A3S-Lab/Use/actions/runs/32604181662) 从 exact `main` commit `40bc5593cbf58ca2da171d85ba578c2d6bd911c8` 在五目标上通过 current Use-owned workspace 与 real-process integration suite。
这仅 establish current Use-owned platform baseline；
product-host、reboot、broader antivirus contention 与 remaining recovery scenario 仍是 release blocker。

Trusted package 与 state path 使用 platform-aware metadata check，在 traversal 前 reject Unix symbolic link 与 Windows reparse point。Platform test coverage 不等于 production qualification。

## 仓库布局

`a3s-use-science` intentionally 不是本 repository、workspace、runtime、CI 或 release 的一部分。
Domain-specific Science code 仍 independently owned，日后仅可作为 signed package 通过与其他第三方 capability 相同的 Registry contract 消费。
名为 `a3s/science` 的 test package 是 synthetic Registry fixture，不 link Science crate。

```text
Use/
├── crates/core/             canonical contracts, resolver, plans, grants
├── crates/extension/        ACL packages, TUF Registry, receipts, Artifact Store
├── src/cognitive_package/   reviewed package-graph application service
├── src/plugin_manager/      shared typed service and standard manager MCP
├── src/plugin_lifecycle/    durable six-surface lifecycle and host boundaries
├── src/plugin_runtime/      Runtime provider selection and exact bindings
├── src/okf_knowledge/       standalone OKF Knowledge backend
├── website/                 GitHub Pages documentation site
└── docs/                    architecture, contracts, ADRs, and release design
```

## 开发

从本 repository 运行检查，而非 A3S monorepo root：

```bash
cargo fmt --all -- --check
cargo test --workspace --all-targets
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo check -p a3s-use --no-default-features
cargo check -p a3s-use --no-default-features --features extensions
```

构建并验证 documentation site：

```bash
cd website
npm ci
npm run format:check
npm run lint
npm run build
npm run check:site
```

Contribution rule 见 [AGENTS.md](AGENTS.md)。Public Rust type 在适用处应保持 typed 且 `Send + Sync`；I/O 使用 Tokio；ACL 是 default human-authored configuration format。

## 文档

- [产品路线图](ROADMAP.md)
- [Plugin 契约参考](docs/plugin-contracts.md)
- [Plugin 平台架构](docs/plugin-platform-architecture.md)
- [生命周期与安全](docs/plugin-platform-lifecycle-and-security.md)
- [Model Hardware Standard 集成 profile](docs/mhs-integration.md)
- [开发计划](docs/plugin-platform-development-plan.md)
- [已验证发布安装](docs/release-installation.md)
- [Release descriptor](docs/release-descriptors.md)
- [Agent Package Manager 第一性原理审计](docs/agent-package-manager-audit.md)
- [OKF Knowledge 操作](docs/okf-knowledge-operations.md)
- [Registry 缓存操作](docs/registry-cache-operations.md)
- [文档网站](https://a3s-lab.github.io/Use/)

## 许可证

Apache-2.0。参见 [LICENSE](LICENSE) 与 [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md)。
