<p align="center">
  <img
    src="assets/readme/hero.svg"
    width="100%"
    alt="A3S Use resolves one exact cognitive-package graph and publishes Tool, MCP, OKF, A3S Flow, Skill, and UI through one atomic cutover"
  />
</p>

<p align="center">
  <strong>Language / 语言:</strong>
  <a href="README.md">English</a> ·
  <a href="README.zh-CN.md">中文</a>
</p>

<p align="center">
  <strong>AI 本机包管理器，用于本机功能和版本化认知包。</strong>
</p>

<p align="center">
  <a href="https://a3s-lab.github.io/Use/">网站</a> ·
  <a href="#install-or-build">安装</a> ·
  <a href="#cognitive-package-format">封装格式</a> ·
  <a href="#replaceable-registries-and-exact-locks">注册表</a> ·
  <a href="#current-contract-baseline">合约</a> ·
  <a href="#implementation-status">状态</a> ·
  <a href="ROADMAP.md">路线图</a>
</p>

> [!警告]
> **开发预览 - 尚未做好生产准备。** 认知包
> 平台尚未发布受支持的产品版本。预发布清单，
> 收据、操作记录、目录元数据和主机协议不
> 兼容性目标：通过清理拒绝不支持的状态
> 重新安装说明。版本标签不会更改此发布状态。

## A3S 的用途是什么

A3S 使用解析、验证、安装、升级和删除确切的 SemVer
包图。认知包可以提供六个命名表面：
**工具、MCP、OKF、A3S 流程、技能和 UI**。包是生命周期单元；
其用户或工作空间安装是一致性单元。它的表面是
一起准备并通过一个不变的方式变得可见
能力快照切换。

它专为 Linux、macOS 和 Windows 上的 A3S 主机而设计。它并不试图
替换任意系统软件的 `apt`、Homebrew 或 WinGet。 A3S 使用拥有
包信任、不可变代、收据、依赖顺序、生命周期
期刊和能力证据。运行时、网关、流程、知识和 UI
主持人保留执行和演示的所有权。

当前的架构有五个不可协商的属性：

- **一个安装图：** 每个显式用户或工作区安装都有
  一个单调生成的`InstallationSnapshot`。它拥有统一的
  解析图加上每个包的启用和选定表面意图。
  根锁是派生视图；依赖项向前安装，退休运行
  相反，一个包 ID 不能在两个根下进行不同的解析
  在同一安装中。
- **一条经过审查的突变路径：** 规划是只读的；申请接受
  审核操作 ID、计划摘要和确认。没有直接的
  启用/禁用突变 API。
- **一个串行安装突变：**安装、升级、卸载、启用、
  禁用和精确恢复共享跨进程写入器栅栏。每个
  经审查的切换与预期的能力生成相关；一个失败的
  并发计划在提供者或包发布效果之前失败。
- **一个不可变的内容身份：**经过验证的原始目标和扩展包
  树仅通过全局 Artifact Store 中的摘要进行键入。注册表来源
  保留观察结果和部分下载；安装自己的选择和
  生命周期世代，绝不是相同内容的私人副本。
- **一个有界的注册表权限边界：** 安装的权威
  `registry.json`快照仅通过拥有的目录读取和发布
  链、无跟随/重新分析安全文件句柄、4 MiB 字节上限，以及
  原子临时文件替换。读者重新检查打开的文件并拒绝相同路径的更改，而不是解析无限制或重定向的文件。
- **当前协议基线：** 预发布格式被拒绝
  而不是解码、迁移或默认默认。

## 此存储库中的证明

实现和固定装置直接运用产品模型：

- [`plugin-v3-cognitive`](crates/extension/fixtures/packages/plugin-v3-cognitive/)
  是一个包含所有六种表面类型的内容寻址包。
- [`plugin-v3-mhs-bridge`](crates/extension/fixtures/packages/plugin-v3-mhs-bridge/)
  证明硬件适配器重用标准 MCP、Flow、Skill 和 UI
  图，在没有确切的托管网关绑定的情况下仍未发布，并且
  不需要 MHS 特定的封装表面。
- [`PluginPackageResolver`](crates/core/src/plugin/package_resolution.rs)
  解决有界 SemVer 闭包并拒绝循环、不兼容的版本、
  和跨注册机构的歧义。
- [`InstallationSnapshot`](crates/core/src/plugin/installation_snapshot.rs)
  拥有一个作用域所需的根、统一的锁图、包状态
  生成、启用和精确选定的表面发布意图。
- [`RegistrySourceStore`](crates/extension/src/registry_sources/mod.rs)仍然存在
  规范修订寻址 ACL 源配置，导入摘要绑定
  受信任的根，并通过源身份隔离 TUF 元数据和缓存。
- [`ArtifactStore`](crates/extension/src/artifact_store.rs) 商店扩大
  打包在一个分片全局 SHA-256 路径中，通过以下方式序列化并发提交
  摘要，拒绝链接/重分析点祖先，并且不进行安装
  或激活权限。
- [`CapabilityGatewayCatalogStore`](src/capability_catalog_store.rs) 拥有
  一次安装的精确的面向代理的目录负载。它发布
  不可变的规范记录，支持显式的保护集保留，以及
  保留有界恢复日志，因此中断的修剪可以通过以下方式恢复
  `recover_retention()` 无需发明生命周期权限。网关
  会话工厂的 `from_published` 和 `replace_published` 路径验证了这一点
  在公开实时端点之前进行精确的持久发布。不活跃的
  对照组合物另外将出版物身份结合到在一笔事务中应用功能切换和发布游标。其
  生命周期协调一起读取游标和拥有操作，
  其排出和保留路径需要准确的目录标识和
  在关闭实时端点之前控制发出的生成租约。的
  协调备份库存验证其规范记录
  一个功能有效负载系列下的签名/遗留描述符快照。
- [`RegistryNetworkPolicy`](crates/extension/src/remote/network.rs)让
  嵌入主机为不受信任的对象选择严格的公共互联网边界
  注册表端点。该模式需要 HTTPS，固定检查 DNS 答案，
  拒绝非公共地址空间和代理，禁用自动重定向，
  在每一跳重新检查有界目标重定向，并应用于 TUF 元数据，
  引导根、规划目标和包目标等。
- [`CognitivePackageManager`](src/cognitive_package/)绑定签名目录
  证据、精确锁定、审查计划、授权和崩溃重放。
- [`ExtensionRegistry`](crates/extension/src/registry.rs) 保留已发布的
  安装快照背后有界的、拥有的、跨平台的文件IO所以
  格式错误、过大、链接或同时替换的权限不能被
  被承认为有能力的一代。
- [`CognitivePackageHostManager`](src/cognitive_package/host_manager.rs)
  为一个精确的托管范围实现类型化主机协议 v6 端口
  栅栏。它将请求 ID 持久地绑定到用户拥有的计划和最终结果，
  同时委托注册管理机构决议、准入、生命周期、拨款和
  观察其他主机使用的相同`CognitivePackageManager`。
- [`bind_cognitive_package_provider_plan`](src/cognitive_package/provider_plan.rs)执行授权安全的两遍提供商协议：未绑定草稿，
  指定提供者预检、主机权限、规范授予语义，以及
  经过漂移检查的最终选择。
- [`PluginPackageGraphLifecycleCoordinator`](src/plugin_lifecycle/graph.rs)
  准备依赖闭包，执行一次持久的注册表切换，调用
  可选的主机拥有的网关激活边界，耗尽接受的呼叫，
  并让前几代人退休。激活钩子是重放安全的，
  将其不透明密钥绑定到拥有已发布的持久控制操作
  游标，并在任何上一代排水之前运行；非活动控制
  组合为实时会话提供控制租赁支持的适配器
  替换并拒绝复制或不相关会话的排出请求。
- [`RuntimeTaskDispatcher`](src/plugin_runtime/task_dispatch.rs)重新打开
  在审核时选择精确的 v4 任务绑定和提供者，而功能
  快照 v5 仅发布匹配的版本支持的任务，并具有完整的
  安装和生命周期标识。
- [`SqliteOkfKnowledgeAdapter`](src/okf_knowledge/sqlite/mod.rs)阶段，
  提升、搜索、读取和删除范围隔离的 OKF 投影
  精确的包生成引用，保留源 Markdown，有界
  收据核算存储、全局墓碑修剪、物理 SQLite
  删除后压缩、源/索引完整性审核、非覆盖
  已验证的备份、精确计划最旧优先的备份轮换、
  保权限FTS修复、权限绑定数据库+
  丢失绑定恢复。
- [`A3sFlowLifecycleHost`](src/flow_runtime/lifecycle.rs)代表流程预检到真实的 `a3s-flow` Native TypeScript 运行时并记录
  精确生成绑定。
- [`StandaloneCognitivePackageLifecycleFactory`](src/cognitive_package/hosts.rs)
  仅从显式绝对编译器路径组成该主机；失败了
  预检尚未发布，可以根据确切的持久证据进行重播。
- [`crates/core/fixtures/plugins`](crates/core/fixtures/plugins/)下的合同赛程
  冻结当前模式的规范 JSON 和 SHA-256 摘要。

CI 运行格式化、完整的 A3S 使用工作区测试、Clippy、
发布容器一致性和平台作业。现在的 Windows 预览门
执行完整的当前工作区套件，包括真实的
共享重解析点保护的目录连接回归。当地人
Windows 套件还证明注册表切换容量拒绝发生在之前
任何生命周期收据替换以及 Box 委托保留参数，
通过本机命令脚本输出和退出状态。可恢复注册表
部分文件在不遵循其最终路径的情况下打开，并保持由一个人拥有
手柄； Windows 门证明主动部分允许读者但拒绝
外部写入和删除，直到事务释放它。签署登记处，
依赖图、Grant、Flow-preflight/lifecycle 和独立 OKF 场景
也可以通过真正的 CLI 运行。现在它的终止进程覆盖率
包括在持久注册表图发布后终止的多节点安装
但在依赖日志和安装快照完成之前，
升级切换后删除依赖项清理，并在持久化后终止卸载
注册表隐藏但在包裹隐藏收据之前。安装会重播
离线精确切换，无需另一代或网络请求。的
卸载从同一计划重新启动，阻止已接受的呼叫生成
租赁，然后耗尽并退役范围内的发电权，无需另一个注册表生成；缺少包状态但仍没有确切的切换
关闭失败。
生命周期提交和清理重试 Windows 访问、共享和锁定冲突
每个阻断突变最多持续两秒。瞬态扫描仪处理结束
活动工件暂存目录、选定的升级收据、删除
收据，或嵌套废弃的暂存文件让相同的提交或权限
退休继续。持久活动暂存句柄在接收之前失败
或注册表快照突变，保留剩余树，并让我们提交
发布后立即重播。持久选择收据锁保留
有效的全局候选工件并回滚保留收据状态，同时
保留字节精确的先前接收和发布的生成；升级
释放后重放成功。一个完整全球的持久读者
工件不会阻止卸载，并且共享字节仍然可用。
在每个持久主机效果之后也会存在测试二进制子进程矩阵，但是
在收到每个规范安装、升级、启用、禁用和
卸载检查点；恢复重复使用精确的幂等密钥，无需
复制效果，并且终端重播不会进行主机调用。一秒钟
测试二进制子进程矩阵涵盖授权安装、升级和
卸载图形转换：它在原子发布或隐藏效果后退出
但在打包发布收据和格兰特切换证据之前，然后证明精确密钥恢复、一张图效果、完成包和授予
日志和终端重播，无需再次发布或隐藏。分开
托管范围管理器进程在五节点安装期间被外部终止，
升级，并在注册表发布/隐藏后卸载，同时存在一个依赖项
出版收据正在等待中，格兰特期刊仍在准备中。重新启动
在禁用重新授权的情况下运行，不执行网络请求，保留
精确候选格兰特，仅退休绑定的先前格兰特，完成包
并授予期刊，并且不会再次推进注册表生成。五
真正的 `CognitivePackageHostManager` 协议子项还涵盖
注册服务器停止后完成审核的应用集。安装，
升级、卸载在对应的五节点图处被杀死
发布/隐藏边界。根包绑定隐藏后，Disable 被杀死
授予切换承诺，同时接受的呼叫租约阻止消耗；启用是
在登记处公布后被杀，而其候选人格兰特仍在准备中。
重启会消耗持久审核计划和确认；安装并
升级也仅使用经过验证的计划缓存。恢复不
重新授权，汇聚确切的候选人/先前的拨款或启用
重新授予/撤销，完成排出和两个日志而不生成
通货膨胀，保持主机结果，并且最终仍然可重玩。这些
路径不会替换仍然开放的实际产品主机并完成跨平台故障注入门。格兰特
也存储自己
在所有 14 个持久检查点上运行一个测试二进制子进程矩阵
其规范的两个候选人/两个退休生命周期：向前准备，
切换/退役和切换前回滚均包括每个候选者
收据、事先撤销和候选人恢复。
请参阅【平台支持](#platform-support)】。

## 安装或构建

标记的档案仍然是开发预览。安装程序选择当前的
操作系统和架构，需要 Cosign，针对 `checksums.txt` 进行身份验证
确切的 A3S 使用标签工作流身份和 GitHub OIDC 颁发者，验证所选
在提取之前存档 SHA-256，拒绝不安全的存档条目，以及
以原子方式发布用户范围的命令。首先下载安装程序，这样就可以了
可以在执行前进行审查。

Linux 或 macOS：

```bash
curl --proto '=https' --tlsv1.2 -fsSLo /tmp/a3s-use-install.sh \
  https://raw.githubusercontent.com/A3S-Lab/Use/main/install.sh
sh /tmp/a3s-use-install.sh
```

带有 Windows PowerShell 5.1 或 PowerShell 7 的 Windows x86_64：

```powershell
$installer = Join-Path $env:TEMP 'a3s-use-install.ps1'
Invoke-WebRequest https://raw.githubusercontent.com/A3S-Lab/Use/main/install.ps1 -OutFile $installer
& $installer
```

`cosign`必须安装在`PATH`上；显式可信可执行文件可以是
在 Unix 上使用 `--cosign <path>` 或在 Windows 上使用 `-CosignPath <path>` 选择。

在 Unix 上传递 `--version <version>` 或在 Windows 上传递 `-Version <version>` 来固定
标签。 Unix 安装在 `$XDG_DATA_HOME/a3s-use` 下（或
`$HOME/.local/share/a3s-use`）和来自`$HOME/.local/bin`的链接。 Windows 使用
`%LOCALAPPDATA%\A3S\Use`，在下创建一个拥有的命令垫片
`%LOCALAPPDATA%\A3S\bin`，并将该 bin 目录添加到用户 `PATH` 除非
`-NoPathUpdate` 已设置。托管启动器绑定打包的 OCR 模型，
OCR 技能和浏览器技能，同时保留显式环境
覆盖。重新安装相同版本会重新验证完整安装的内容
树。缺少 Cosign、无效的 Sigstore 证据、校验和不匹配、被篡改
现有版本、不安全路径、链接/重新分析点、并发安装程序或
在不更改活动命令的情况下，非托管命令冲突会失败。的
已验证的校验和清单和 Sigstore 捆绑包保留在不可变的中
版本目录。参见
[已验证发布安装](docs/release-installation.md)为了信任
边界和自定义路径选项。

标记发布工作流程旨在发布确定性序列化的内容
档案、每个平台一个 SPDX JSON SBOM、GitHub OIDC 构建来源和 SBOM
证明，以及 `checksums.txt` 的无密钥 Sigstore 捆绑包。它固定每个
Action 加上 Rust、Python、Syft 和 Cosign 版本，派生存档
标签提交的时间戳，并在之前验证其校验和签名
出版。除非 Cosign 进行身份验证，否则安装程序将无法关闭
在下载存档之前，根据确切的标签身份进行捆绑。对于
每个目标，第二个干净的运行器没有编译的工件缓存重建
所有附带的本机可执行文件，并且必须在之前与主存档进行字节匹配
确定性`.reproducibility.json`证据可以被证明、校验和，
签署并在档案旁边公布。

`v0.3.7` Rust 兼容性版本保留了后`v0.3.3`
原子快照租赁、共享管理器、运行时服务重新绑定和标准
MCP管理器在携带无路径能力网关的同时签订合同
描述符/目录适配器。完整的快照具有有界的、规范的
通过干净目标暂存、激活和崩溃进行运行时计划存档
重播；工件可达性保留了已提交计划引用的 blob。它
将外观的精确 `a3s-flow 1.1.0` 注册表依赖项与
`a3s-code-core 8.0.3` 并发布`a3s-use-core 0.2.6`，
`a3s-use-extension 0.3.7`和`a3s-use 0.3.7`。立面继续沿用
与 A3S 搜索相同发布的 Browser 0.3.2 提供程序，因此打包的消费者可以
解析一个名义上的浏览器/核心/流程能力图。这是一个兼容性
发布并且不会改变开发预览状态。网关
适配器保持合同级增量，直到生命周期租用/耗尽，
身份验证、CLI 连接和独立客户端资格已完成。

标记的`v0.3.2`工作流程暴露了四个上的本机链接器元数据漂移
五个目标，因此没有创建 GitHub 版本。非出版类
【资质运行33651777660](https://github.com/A3S-Lab/Use/actions/runs/33651777660)
冻结 `main` 提交 `4f6e4725205d06ab81f8ea98bfee85c7eb4b2bcd` 并通过
完整的五平台存档、独立安装路径扫描、SBOM 和
证明和无缓存的逐字节重建矩阵；它仍然是历史的
证据并且从不公布资产。早期的 `v0.3.5` 发布尝试
没有创建 GitHub 版本，因为公共 `a3s-use-core` 箱子已
还是`0.2.4`。发布工作流程
[33675697857](https://github.com/A3S-Lab/Use/actions/runs/33675697857)然后构建
标签 `v0.3.6` 来自确切的 `main` 提交
`54758910f2f4ad9498137410e0a2207d412e99a1`，通过了所有初级和独立
五目标工作，并发布发展预览
[v0.3.6 发布](https://github.com/A3S-Lab/Use/releases/tag/v0.3.6)
`a3s-use-core 0.2.5`、`a3s-use-extension 0.3.6` 和 `a3s-use 0.3.6` 封装。
发布工作流程
[33687297386](https://github.com/A3S-Lab/Use/actions/runs/33687297386)然后构建
标签 `v0.3.7` 来自确切的 `main` 提交
`48a0b76f8a4a87a11d16627c7bd7567920852508`，通过了所有初级和独立
五目标工作，并发布发展预览
[v0.3.7 发布](https://github.com/A3S-Lab/Use/releases/tag/v0.3.7)
`a3s-use-core 0.2.6`、`a3s-use-extension 0.3.7` 和 `a3s-use 0.3.7` 封装。
发布工作流程
[33720485826](https://github.com/A3S-Lab/Use/actions/runs/33720485826)然后构建
标签 `v0.3.8` 来自确切的 `main` 提交
`6d3a7baf32ce998a2e487c40fbf78b4a6cda2579`，通过完整验证，
五目标主要构建和独立的无缓存重建门，以及
发布了开发预览
[v0.3.8 发布](https://github.com/A3S-Lab/Use/releases/tag/v0.3.8)
`a3s-use-core 0.2.7`、`a3s-use-extension 0.3.8` 和 `a3s-use 0.3.8` 封装。
发布工作流程
[33756618837](https://github.com/A3S-Lab/Use/actions/runs/33756618837)然后构建
标签 `v0.3.9` 来自确切的 `main` 提交
`a5f3cc40bfb0a1021ca150d2ce4295409b74d220`，通过完整验证，五个目标主要构建，以及五个独立的无缓存重建，以及
在 中发布了 19 个经过验证的发布资产
[v0.3.9发布](https://github.com/A3S-Lab/Use/releases/tag/v0.3.9)，包括
档案、安装程序、校验和/Sigstore、SBOM 和再现性证据，
以及 `a3s-use-core 0.2.7`、`a3s-use-extension 0.3.9` 和 `a3s-use 0.3.9`
包。
发布工作流程
[33791616307](https://github.com/A3S-Lab/Use/actions/runs/33791616307)然后构建
标签 `v0.3.10` 来自确切的 `main` 提交
`c4c80a223bfff3698ca4b4598e7175c6e3303239`，通过完整验证，
五个目标主要构建，以及五个独立的无缓存重建，以及
在 中发布了 19 个经过验证的发布资产
[v0.3.10 发布](https://github.com/A3S-Lab/Use/releases/tag/v0.3.10)，包括
档案、安装程序、校验和/Sigstore、SBOM 和再现性证据，
以及 `a3s-use-core 0.2.8`、`a3s-use-extension 0.3.10` 和 `a3s-use 0.3.10`
包。
发布工作流程
[33830280138](https://github.com/A3S-Lab/Use/actions/runs/33830280138)然后构建
标签 `v0.3.11` 来自确切的 `main` 提交
`c25028ae0245ba1d28f7e2837e2a87f7e9f6fe40`，已通过验证，五目标
主要构建，以及五个独立的无缓存重建，并发布了 19
已验证的释放资产
[v0.3.11发布](https://github.com/A3S-Lab/Use/releases/tag/v0.3.11)，包括
档案、安装程序、校验和/Sigstore、SBOM 和再现性证据，
以及 `a3s-use-core 0.2.9`、`a3s-use-extension 0.3.11` 和 `a3s-use 0.3.11`
包。
外部操作的全档案证人，证据保留在 GitHub 之外
发布，其余产品大门仍然打开，所以这不会
更改上面的预览状态。操作员还可以验证是否成功
GitHub 认证遵循 [已验证的版本安装](docs/release-installation.md#additional-independent-verification)。

### 构建并验证

需要 Rust 1.85 或更高版本。直到产品发布门完成，
从源代码构建：

```bash
git clone https://github.com/A3S-Lab/Use.git
cd Use
cargo build --workspace --bins --locked
./target/debug/a3s-use doctor \
  --scope-kind user --scope-id user/alice --json
./target/debug/a3s-use capability snapshot \
  --scope-kind user --scope-id user/alice --json
```

Rust 嵌入主机可以将相同的权威扩展注册表绑定到
类型化的能力桥和引脚一完整的发布一代：

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

光标绑定安装快照生成和摘要功能
修订版、注册表修订版和排序的确切包生成。收购
按规范顺序获取每个包生成租约并重新检查两者
持有完整批次后不可变的权限。
隐藏的、陈旧的、混合的、竞争的或消化不匹配的一代不会返回任何结果。
租赁；没有不可变生命周期证据的已启用旧包绑定失败
关闭。非克隆 RAII 租约为`Send + Sync`，因此 A3S Code 可以将其保留在
a 运行范围，同时使用生命周期退休等待已接受的工作耗尽。
删除它只会释放同步生成锁；异步清理
仍然由 Use 生命周期协调器明确拥有。

功能监视现在订阅原子扩展注册表出版物
而不是按照固定的时间间隔重建完整的投影。当地人
首选文件系统后端，同时运行有界目标元数据探测器
与它一起捕获平台后端可以合并的原子替换
或省略；当本机注册时，使用仅元数据轮询后端
不可用。事件经过目标过滤并合并为一个有界信号；
经过验证的`registry.json`仍然是权威。 `CapabilityRegistry`
在真实的订阅设置后重建并散列完整的投影
一代人前进，并一度在暂停时结束了最后的比赛。这删除了
从正常等待路径重复包扫描和资产散列，无需
创建第二个可变生成游标。
在生命周期中保留完整的面向代理的描述符目录
能力指数仍然是一个单独的产品门。

`capability snapshot --json` 模式 v5 仍然是 CLI 的外层。它
公开安装快照的生成和摘要，同时完整的
进程内游标故意不附加到独立释放的游标上
架构。托管 MCP、技能身份和 UI
依赖字段是显式的。每个扩展 MCP 表面都保持其规范
ID和多重性、抗冲突主机服务器名称、激活、
包/清单/生成身份、经过审查的文件证据摘要以及一个
特定于运输的发射预测。 Stdio 投影仅包含
包相关的可执行文件和有界参数。可流式传输的 HTTP 投影
仅包含包相关版本、不透明端点引用/路径，以及
准确的运行时/网关准备情况摘要；解析的 URL 和凭据永远不会
输入快照。每一个 UI 贡献都承载着
`a3s.use.ui-dependency-evidence.v1` 所以空的依赖列表是可区分的
来自未发布依赖性证据的旧主机。

独立的 CLI 目前公开了包图生命周期、诊断、
能力观察、内置浏览器/OCR 路径、引用 OKF 搜索以及
精确范围知识存储操作：

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

`mcp serve manager` 在标准输出上使用标准 MCP，因此不得
与`--json`结合。 `manager`、`package-manager` 和
`use/package-manager` 目标名称是等效的。它由相同类型的
CLI 和 TUI 使用的`PluginManagerService`；它不会创建第二个
目录、计划、确认或突变路径。

独立注册表支持的 `install`、`upgrade` 和 `uninstall` 现在正在计划和
通过共享`PluginManagerService`申请。他们现有的组件和
`packageGraph` 字段仍然可用，而 JSON 输出还包括
`pluginManager` 包含确切操作 ID、计划摘要、已审核的对象
主机计划结果，终端主机申请结果。重复不变
操作返回持久重放结果。离线规划和恢复停留
零网络，并且提供的包锁摘要在任何目标之前被拒绝
下载。兼容性命令仅自动应用，无需许可 `Allow`
计划。一对一的`plugin`命令公开相同的四个读取操作，
五个只读计划操作、仅摘要应用、精确操作
观察/观察，以及明确的取消边界作为管理器工具集 v5。
每个成功的 JSON `data` 值都是精确类型化的服务结果，
包括完整的主机计划、包锁、来源和权限证据，
操作ID、计划摘要、确认决策、终端应用结果。
规划永远不会改变包状态。 `plugin apply-plan` 仅重新打开确切的
耐用的 `(operation ID, plan digest)` 配对，需要 `--yes`；一个普通的 CLI
呼叫并不意味着用户确认，`Ask`计划收到确认
仅在该明确的边界处。精确应用和重播使用经过验证的计划
缓存无需注册表访问。 A3S 代码 CLI、TUI `/packages` 和标准
manager-v5 MCP 现在可以在没有演示拥有的计划的情况下组成相同的服务，确认，或突变路径。每个独立管理器命令都需要一个
显式用户或工作区安装。所选的`InstallationId`拥有
管理器和所有可变状态；用户和工作区安装具有相同的
文本 ID 保持不同的权限域。

运行时、流程、知识和生命周期证据存储准确捕捉到这一点
`InstallationId` 当它们被构造时。收据、查询、恢复项目、
或另一个安装的生命周期意图失败
`use.installation.identity_mismatch` 在Use导出路径之前，获取一个
存储锁、创建数据库或写入证据。单独安装使用
独立的存储，而不可变的注册表和工件输入仍然可共享。

范围布局和全局 Artifact Store 是故意预发布干净的
切换，而不是迁移。如果 `use.installation.legacy_state_unsupported` 或
`use.artifact_store.legacy_state_unsupported` 被报告，停止旧的使用主机，
保留事件审查的先前根源，并仅删除已证明的遗留问题
使用显式范围标志重新安装之前的条目。这些条目包括
旧的全局`data/extensions`，安装范围
`data/installations/<kind>/<key>/extensions`，以及旧的国家级`extensions`，
`registry.json`，生成，授予，绑定，生命周期，知识，图，
支持、主机管理器、路由锁定/生成租赁和突变锁定路径。保护全球
`registries.acl`，注册表信任根，TUF 元数据/目标，以及
`data/artifacts`；它们是装置共享的输入。

扩展内容位于
`data/artifacts/expanded-packages/sha256/<prefix>/<digest>/content`。不同
安装可能会指向同一棵树，同时保留独立的
收据、生成、启用、赠款、绑定和租赁。使用重新哈希
发布和使用之前的内容。全局字节只能通过
明确确认的 Artifact Store 垃圾收集计划；源头清理和
范围卸载仍然不会删除它们。一个
跨流程共享/独占边界现在可以防止未来的库存和
来自赛车原始目标观察、生命周期收据的收集，
应用生命周期日志、安装快照或待处理图表
操作。源观察和可恢复部分仍然是注册表源
范围；它们的验证字节使用全局 Blob 层。该库公开了一个
精确排他性下有界、确定性、无路径的物理库存
店员。它分别报告规范内容和放弃的分段，
在未知布局、链接/重分析点、特殊文件或情况下关闭失败
遍历限制。派生出一个单独的无路径注册表参考清单
来自所有保留的源数据存储的每个规范 blob 观察，
包括更换的来源，在同一守卫之下。全局无路径
`a3s.use.artifact-reference-inventory.v1` 视图现在聚合了这些观察结果
每个安装快照，当前和保留的收据，未取消
包图操作，应用或回滚生命周期日志，以及
不可变的运行时计划有效负载。运行时计划记录在保存时被解码安装维护和计划存储锁，因此他们引用了 Blob
清理过程中工件仍可访问。生产出版物通过
`ExtensionPaths`绑定计划商店之前获得全球参考入场券
安装围栏；为隔离离线/测试状态创建的商店不会
具有全局 Artifact Store 边界。它验证安装身份并
源布局，拒绝冲突的物理期望，并保留
即使内容缺失，也可以参考。所加入的
`a3s.use.artifact-reachability-inventory.v1` 视图捕获逻辑证据
以及一张受保护的收集通行证中的实物库存。新出版物是
冷冻；参考退休只能留下保守的额外业主。一排
每个工件保留所有者、物理测量、期望状态和
检查全局存储使用情况不同。

Artifact Store 现在拥有可选的持久硬配额政策：
`data/artifacts/storage-quota.acl`。 `ArtifactStore::storage_quota`,
`set_storage_quota`和`clear_storage_quota`通过公开规范ACL状态
修订比较和交换。该策略限制逻辑常规文件长度和
摘要容器，而不是分配的文件系统块。出版物总是进入
首先是参考准入，然后是全局存储边界，然后是精确的
摘要锁。在没有策略的情况下，发布者共享存储边界。与一个
政策，最终的 Blob 或扩展包出版物独家拥有它，
扫描当前内容加上废弃的暂存，项目准确准备的写入，
并通过暂存清理和原子提交保留锁。独特的
因此，进程不能同时使用相同的剩余容量。如果一个
运营商收紧了低于当前使用情况的策略，精确重播和清理
不恶化任何超过尺寸仍然是可能的。畸形的政策
在不抑制物理库存证据的情况下关闭写入失败。

`ArtifactStore::audit_digests` 现在执行显式全存储完整性
通过确切的商店收集警卫。其确定性，
无路径`a3s.use.artifact-store-digest-audit.v1`报告顺序重新散列
使用原始 SHA-256 完成原始 Blob 并使用相同的扩展包
入院时使用的规范包装指纹。它报告`verified`，
`mismatch`，以及未散列的 `incomplete` 结果加上检查的字节/文件总数。
通行证在返回之前重复了有限的实物库存，因此承认
出版物被冻结以获取完整的操作和可观察的布局，或者
测量漂移失败关闭。摘要不匹配仍然是证据；审计
绝不删除、覆盖、隔离或重新水化内容。

效果所有者现在拥有一个无路径的验证读取边界，而不是处理
`expanded_package_path` 作为权限。 `ArtifactStore::acquire_verified_package`
接受一份完整的经过验证的目录记录，获取全球可达性并
每个工件的突变锁定在共享模式，拒绝中断的收集和
逻辑隔离，并重新验证完整的包指纹、清单
摘要、准确的字节/文件计数、清单到目录表面图以及每个
声明的表面文件。不可克隆的租约仅公开目录身份和
解析后的清单；它的 `Debug` 形式不包含本地路径。清单读取是
在 ACL 解析之前有界，丢失的锁永远不会由读取创建，并且
`verify_unchanged` 重复完整验证以检测不协调
适配器记录成功之前的本地篡改。

逻辑损坏隔离是一项单独的精确计划操作。
`ArtifactStore::plan_quarantine` 只接受新的一个完全不匹配
在相同的确切收集守卫下进行审计并返回规范的、无路径的
证据。 `apply_quarantine`重新审核字节，要求审核准确
计划摘要，并原子地发布有界规范`quarantine.json`
记录而不移动或覆盖`content`。同一记录的重播是
幂等的。失败的恢复保留其有界的临时故障关闭哨兵，
因此普通访问不会在重试之间重新打开。实物库存
验证活动和中断的隔离元数据，但将其排除在外
内容和分期配额测量。新斑点
打开、观察和提交以及扩展包验证和提交
一旦标记存在，关闭失败。该标记保留取证字节和
阻止未来的普通使用；它不会撤销已经打开的句柄，而是重写
承认生成、授权再水化或授权删除。

验证的补水是一个单独的参考感知突变，由
`ArtifactStoreMaintenance`。规划和每个非终结应用都获得
精确的全球收集保护并重新扫描每个登记处观察，
安装快照、当前或保留的收据、待处理的包图以及
非终结生命周期操作；目标的持久引用必须为零
更换前。独立提供的候选人必须居住在
Artifact 存储并匹配预期的 Blob SHA-256 或规范扩展包
指纹。规划仅发出无路径证据。初次申请需要其
准确的规范摘要，重新验证候选者和隔离区绑定，
持久地发布准备好的证据，并保持普通访问失败关闭
同时它会暂存和切换规范内容。匹配的完成记录
打开访问。精确的终端重放是只读的：它验证完成情况，
隔离绑定和规范替换，无需重新打开外部
候选人或要求后来的业主再次退休。准备工作中断或
内容切换从有界状态恢复，移动或冲突的记录失败
关闭，硬配额入场占暂时恢复高峰。
应用消耗已审查的损坏取证字节；需要的运营商
证据保留时间较长，必须在店外存档
确认。现有开放句柄未撤销，但未承认包
Generation 可能会在替换期间引用目标。

确认 Artifact Store 垃圾收集是单独的引用感知
由`ArtifactStoreMaintenance`协调的突变。它的策略是非空的，
精确 `(kind, digest)` 目标的有界、规范白名单；没有
计时器、年龄阈值、配额触发的扫描或隐式“全部未引用”
模式。规划掌控全球收藏卫士，证明零耐用所有者
跨每个注册表、安装、接收、快照和非终端
操作，并结合精确的物理测量加上普通的、隔离的、
或将已完成的补液生命周期证据纳入无路径计划中。申请
重复零引用证明并仅接受经过审查的规范计划
消化。在任何命名空间突变之前，它会发布一个持久的全局准备好的
记录。然后，每个经过审查的摘要容器都会在其内部自动重命名
分片到确定性墓碑并通过有界的、无链接的方式删除
残差树检查。准备好的或临时的状态会阻止新的引用
重新启动后进入，直到恢复相同的计划。持久的完成
记录使精确重播变为只读，因此旧的确认无法删除
后来重新创建或新引用的相同摘要；每个以后的计划
链接到先前的完成摘要。检疫、补液、审核、配额
压力和身体无法到达只是证据，绝不是独立的
授权删除。

联合配额评估仅作为证据；它不授权
删除。硬准入是故意序列化的，而不是实施为
并行的持久预订分类账。 `complete`仍然只是一个物理的
出版状态；显式摘要审计产生单独的完整性
结果。精确计划逻辑隔离和零参考验证补液
与明确确认的垃圾收集保持分离；没有人授予
另一个人的权威。

默认知识策略将每个完整的用户或工作空间范围限制为
512 MiB 的收据核算扩展内容、256 个保留预测、32 个
每个表面代数，以及 256 个移除墓碑。分期检查整体
原子范围；收据拥有的清除释放配额，修剪旧墓碑，以及
压缩 SQLite 及其 WAL。 `knowledge usage --json` 报告确切的范围，
当前计数、配额、分配的数据库字节和可回收字节。这些
独立控件还审核 SQLite、收据、范围、外键和 FTS
一致性。备份写入一个版本化、SHA-256 绑定的 SQLite 快照，无需
覆盖现有文件；验证重新打开并审核嵌入的
数据库离线。 `knowledge backup-retention` 验证每个托管
`*.a3s-okf-backup` 候选者位于一个拥有的目录中，隔离确切的范围，
并返回一个最旧优先的有界计划。它不会删除任何内容，直到 `--yes` 并且
提供未更改的规范`planDigest`，永远不会删除最后一个
验证范围备份，并将部分删除报告为结果未知。
搜索索引修复需要 `--yes` 并且仅重建 FTS
从已验证的文档派生的行。它从不重写包
收据、预测状态或授权证据。权限绑定恢复
将无路径计划审查与仅摘要确认申请分开，验证
完整的注册表/包/生命周期/授予权限和精确子集绑定
库存，绑定实时主/WAL/SHM 证据，仅恢复丢失的绑定文件，保留以前的文件，并在之后恢复六状态持久日志
进程退出。冲突或较新的具有约束力的证据未能结案。 `知识
Restore-status --json` 读取所选安装的活动标记并
有界无路径历史，无
备份路径或计划摘要；它报告当前阶段、准确摘要、保留
目录计数、未记录的标记切换目录和剩余容量
无需更改恢复或数据库证据。

备份是经过完整性检查的范围数据库快照，而不是签名的信任
工件或整个产品恢复。独立恢复可能会重新创建绑定
仅当当前集是备份和注册表的精确子集时才文件
收据、不可变的包根、生命周期日志和赠款仍然存在
准确。它无法重建那些独立的权威。更广泛的权威
恢复、清洁机器恢复、跨平台操作演练，以及
整个产品的回滚证据保留仍然需要一个程序。
每个安装范围的操作都需要显式的 `--scope-kind` 和
`--scope-id`； CLI 永远不会猜测当前用户或工作空间身份。参见
[OKF知识操作](docs/okf-knowledge-operations.md)。

对于静态整体安装库存，`state backup` 采用
安装的专属维护围栏和快照注册表，
安装快照、保留生成、授予、绑定、
生命周期/包操作、知识、支持和主机管理器控制
状态。扩展包字节是全局不可变输入，不会被复制。
其
`a3s.use.state-backup.v2` 清单绑定了确切的安装并包含
仅可移植的相对路径，
每个文件长度/SHA-256/模式证据、家庭会计、注册表
生成/摘要，以及排序的安装收据摘要。创作扫描件、复印件
使用精确的散列，然后在非覆盖发布之前重新扫描。锁是
排除；主动恢复、挂起的切换/操作、链接/重新分析点、
特殊文件、未知状态系列、安装数据有效负载或不可移植
路径无法关闭。 `state verify-backup` 验证规范
清单字节、完整存档长度和每个离线负载摘要
没有提取或本地使用状态。存档包含原始状态并且必须
作为敏感数据进行保护。 `state backup-retention` 需要一个单独的
外部目录锁，完全验证每个托管存档，并返回
绑定确切文件名、修改时间的无路径最旧优先计划，
长度、清单摘要、库存摘要和登记证据。已确认
apply 仅接受未更改的规范`planDigest`，同步每个
删除，通过精确安装过滤档案，并始终保留至少最新的两个经过验证的档案。全球注册来源/信托/TUF 国家，
Artifact Store 和可导出的 Flow 编译的工件故意放在外部
这个备份。
`state plan-restore` 仅构建无路径添加/替换/删除/保留审核
当备份与当前使用的版本、操作系统、体系结构完全匹配时
独立保留登记/收据/授予权限。确认`状态
Restore` 首先创建或验证显式的外部回滚存档，
仅阶段性发表候选文章，并推进持久的七阶段期刊
其 15 个进程出口边界幂等收敛。活动标记是
在实时突变、候选链接/重分析点和标记之前发布或
日志替换失败关闭，完成的历史记录仅限于 64 条记录，
`state restore-status` 是无路径且只读的。档案仍保留
完整性证据，而不是签名或缺失权限恢复机制；
清洁机器恢复和灾难恢复操作演习仍然进行。参见
[协调状态备份操作](docs/state-backup-operations.md)。

`extension inspect --json` 包括最新和之前的持久生命周期
明确选择的安装的操作。版本化诊断投影
报告操作、状态、生成、工件摘要、检查点进度、
有界错误代码、计时和回滚证据。它故意省略
检查点幂等性密钥、凭证、令牌、秘密值和
包编写的错误文本。这是诊断的检查点证据，而不是
遥测服务或备份/恢复机制。
一项经过审查的图操作可以创建连续的候选者和退休者
同一包的阶段意图。这些记录有意共享一个
`operationId`；消费者通过`intentDigest`、动作来区分确切的阶段，
生成和工件摘要。

`extension diagnose --json` 读取任一精确保留的安装、升级、
或卸载图表、一项主动承认的启用/禁用操作或最新的
主持人审核的启用/禁用计划尚未被所选项目接纳
没有网络 I/O、协调、恢复或的用户或工作空间范围
写道。其
`a3s.use.plugin-operation-diagnostic.v1` 投影约束已审查的计划和
锁定摘要、无路径注册表名称和 TUF 角色版本、当前注册表
生成和切换证据、提供商身份/准备情况、资助日志
阶段、生命周期发布/耗尽/回滚状态和稳定恢复
指导。图形诊断涵盖保留、计划、接纳和取消
仅审查待定计划时的操作和安装前工作
存在。在启用/禁用准入之前，摘要绑定观察索引
选择`(plannedAtMs, requestId)`最新的精确主机计划和项目
`planned` 或 `cancelled`，选定的提供者并等待授予状态，
预期生命周期单位数量以及当前注册管理机构切换证据。的
索引保留托管主机范围仅用于解决其不可变请求；主持人
ID、权限/围栏值、请求 ID 和私有路径永远不会进入
公共投影。主动使用拥有的启用证据优先，并且
持久的主机结果或完成的使用操作会抑制过时的计划。
URL、路径、幂等密钥、凭证、令牌、秘密名称和值，
包内容和任意包创作的文本均被排除。对于保留的安装或升级图表，投影还报告总数
预期和当前保留的存档字节加上每个确切目标的
`missing`、`partial`或`complete`缓存状态；合计为`missing`，
`in-progress`，或`complete`。在审查图表存在之前，持久使用
记录准确的非权威包锁和选定的归档集
进程持有的包锁。 `extension diagnose` 然后返回
`a3s.use.plugin-download-attempt-diagnostic.v1` 具有相同的字节证据。
该记录在下载失败或进程退出后仍然存在，以后的尝试可以
仅在进程锁释放后才替换，并且仅将其删除
待审核的待处理图持久后。两项预测还报告了
由确切保留者选择的单独签名的可执行计划目标
通过`planningBytes`、`planningRetainedBytes`、聚合进行包锁定
`planning`，以及每个包`planningTargets`。每个目标仅公开包
ID、注册表名称、目标摘要、预期/保留字节以及
`missing`/`partial`/`complete`状态。静态包报告`not-required`。

在存在确切的包锁定之前，Use 还将注册表/TUF 工作记录为
`a3s.use.plugin-resolution-attempt.v1`。记录在刷新之前开始或
缓存元数据访问并跟踪请求的版本/通道以及每个根
或依赖项注册表为待定、验证、已验证或失败。它暴露了
仅无路径注册表名称、源身份/信任根摘要、经过验证的 TUF
角色版本、有界目标计数、稳定错误代码和终端
包锁摘要/计数。被杀死或失败的解析器仍然可以诊断；
成功解决会在删除此内容之前写入下载尝试
证据。当图表和下载尝试都不存在时，
`extension diagnose` 返回
`a3s.use.plugin-resolution-attempt-diagnostic.v1` 具有相位 `pre-lock` 且
访问`refreshed`或`cached`。它从不公开注册表 URL、路径、原始信息
传输错误、凭证或元数据字节。

`extension diagnose --history --json`回归
`a3s.use.plugin-operation-history-diagnostic.v1` 对于相同的显式范围。
它在精确的 8 MiB 存储范围内保留了最新的 16 个退役操作，
包括他们完整的无路径操作快照和单独的
验证`completed`/`rolled-back`操作或`cancelled`图形计划结果。历史是
在待处理图或活动启用恢复权限之前写入
删除；相同`(operationId, planDigest)`事件的重播是
幂等的。文本图操作 ID 可以在一次之后合法地重复出现
完全重新安装，因此计划摘要仍然是事件标识的一部分。
卸载后历史记录仍然可用。未知领域、身份/结果
冲突、链接或重分析点以及超大记录无法关闭而无需
回显保留的字节或路径。

图表和下载预测源自历史注册表数据存储
保留签名来源，不发出网络请求或写入，不公开路径，
并且不获取目标缓存锁。完整的归档或规划目标
是一个规范源观察加上一个拥有的精确长度的全局 blob；的
诊断不会重复它，并且它或部分都不会应用，规划，
或恢复权限。分辨率诊断同样是只读和零网络的
不要等待或获取包锁。实际过程测试被证明被杀死
规划目标观察和精确范围恢复，经主持人审查
未经许可、授权或未经许可而计划/取消的支持计划
网络访问，以及完成使用/未完成主机期间的抑制
结果窗口。

### 可替换的注册表源

独立 CLI 将一组有界的命名注册表源保留在
规范的 A3S ACL。第一个启用的源将成为默认源。每个启用
源被提供给依赖解析，而 `--registry-name` 选择
一项操作的根源。跨启用的重复包标识
消息来源因含糊不清而失败。

在包解析之前配置信任：

```bash
a3s-use registry source add packages \
  --url https://packages.example.org/a3s/ \
  --trust-root sha256:<64-hex-digits> \
  --json
```

GitHub 存储库可用作类似于 Homebrew-tap 的创作和静态
不将 Git 历史记录作为信任根的分发源：

```bash
a3s-use registry source add official \
  --github A3S-Lab/Use-Registry \
  --trust-root sha256:<64-hex-digits> \
  --json
```

速记解析为
`https://raw.githubusercontent.com/<owner>/<repository>/main/registry/`。
`--github-ref` 和 `--github-path` 可以选择规范标签/分支名称并
存储库子树。它们只是地址输入：调用者固定的 TUF 根，
签名的catalog-v3元数据、存档哈希、审查的计划和授予仍然是
安装和激活权限。 A3S 使用从不克隆或执行
存储库签出。

`--trusted-root /absolute/path/root.json` 另外导入一个精确的
将摘要匹配根放入托管的内容寻址信任根存储中。
源列表输出包括完整的配置修订。更换
当局要求审查修订并明确确认：

```bash
a3s-use registry source list --json

a3s-use registry source replace packages \
  --url https://mirror.example.org/a3s/ \
  --trust-root sha256:<64-hex-digits> \
  --expected-revision sha256:<reviewed-configuration-revision> \
  --yes \
  --json
```

替换、禁用或删除源绝不会重写已安装的收据
并且永远不会删除其身份绑定的 TUF 元数据、观察结果、部分数据或
全局斑点。重新启用或恢复确切的名称、URL 和 bootstrap-root
摘要重用了确切的源状态。更改后的源身份会收到
独立的数据存储，防止旧的元数据或观察结果跨越
信任边界。

从配置的注册表进行开发安装示例：

```bash
a3s-use install acme/research \
  --scope-kind workspace \
  --scope-id workspace/acme-project \
  --registry-name packages \
  --version 2.0.0 \
  --json
```

当一个锁被单独审查时，绑定应用到它：

```bash
a3s-use install acme/research \
  --scope-kind workspace \
  --scope-id workspace/acme-project \
  --registry-name packages \
  --package-lock-digest sha256:<64-hex-digits> \
  --json
```

不匹配的锁摘要在存档下载之前失败。示例包和
上述注册表名称仅供参考；这个存储库不做广告
公共生产登记处。

在线安装会验证当前的 TUF 元数据并存储每个选定的元数据
在注册表数据存储中存档并签名`planning-v1.json`目标
内容寻址缓存。删除该精确图形后，可以
在没有网络访问的情况下再次安装：

```bash
a3s-use install acme/research \
  --scope-kind workspace \
  --scope-id workspace/acme-project \
  --registry-name packages \
  --version 2.0.0 \
  --offline \
  --json
```

同一标志仅当主机已经刷新时才支持升级
候选人的 TUF 元数据并验证每个选定的目标是否相同
缓存：

```bash
a3s-use upgrade acme/research \
  --scope-kind workspace \
  --scope-id workspace/acme-project \
  --registry-name packages \
  --version 2.1.0 \
  --offline \
  --json
```

离线模式是明确的并且失败关闭。它加载相同的持久注册表
源码修订；重新验证缓存的 TUF 签名、过期时间、源身份、
目标长度和 SHA-256；并返回 `registryAccess: "cached"` 加
JSON 格式的`registrySourceRevision`。正常上线操作返回
`registryAccess: "refreshed"`。源丢失、禁用、过期或被篡改
或者缓存证据是错误的。在线命令永远不会回退到缓存
网络或刷新失败后的目标。

### 验证目标缓存操作

每个注册表都有一个独立的默认逻辑工作集限制 4 GiB
4,096 个组合目标观测值和可恢复部分数据，大小为 256 MiB
源部分/暂存可用空间保留。中断的 HTTP 下载会保留
摘要绑定 `.target-<sha256>.part` 并仅从精确签名的范围重试
回应。完全验证的字节通过复制和重新散列
事务拥有的句柄变为
`<data-root>/artifacts/blobs/sha256/<shard>/<digest>/content`，同步，并且
发布时不替换现有内容。只有这样，注册表才会
源发布规范的`<digest>.json`观察元数据并删除其
部分的。缓存的暂存重新打开并重新散列全局 blob；腐败失败
关闭并且永远不会被悄悄替换。

Windows 本机测试模型扫描仪争用跨 Blob 发布和
源头清理。如果最终部分删除保持锁定状态，则持久 blob 和
观察仍然可用，重试会删除多余的部分，而无需
网络传输。源修剪删除陈旧的写入，然后删除不活动的部分，
然后是最古老的观察。它释放了逻辑源策略容量，但是
从不删除全局 blob、已安装的工件、收据、生成或
期刊。全球参考现在与物理证据和有界的结合起来
跨来源、安装和操作的配额评估。可选全局
硬配额准入和只读摘要审核涵盖两个出版层；
精确计划逻辑隔离区会阻止新观察到的损坏内容，同时
保留其字节，并验证补水需要一个独立的候选者
加上一个新的零参考证明。全局删除现在还需要
有界显式目标策略及其精确确认的 GC 计划摘要。

在不发出注册表请求的情况下检查缓存使用情况：

```bash
a3s-use registry cache usage \
  --registry-name packages \
  --json
```

修剪可以丢弃可恢复的进度和源观察，因此
独立 CLI 需要明确确认：

```bash
a3s-use registry cache prune \
  --registry-name packages \
  --cache-max-bytes 2147483648 \
  --cache-max-entries 2048 \
  --cache-min-free-bytes 536870912 \
  --yes \
  --json
```

持久策略配置在`registry source add`或`replace`上。
确认的修剪可以使用更严格的单命令覆盖；它不会重写
源配置。嵌入主机使用相同类型
`VerifiedTargetCachePolicy`。缓存使用和剪枝是零网络的
操作并在之前验证任何保留的目录缓存源身份
检查或删除源状态。 Schema v3 将 `targetBytes` 报告为逻辑
引用的 blob 字节，而不是修剪回收的物理字节。这个源缓存
GC 永远不会更改全局原始或扩展工件、收据、功能
世代或生命周期期刊。请参阅[注册表缓存
操作](docs/registry-cache-operations.md)。

## 认知包格式

认知包是一个类似 npm 的不可变分发单元，其中包含一个
`<publisher>/<name>` 身份、一个 SemVer 版本、所需的 ACL 清单、
必需的包文档、可选的包依赖项以及零或
更多命名的表面贡献。

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

仅清单和 `README.md` 名称是固定的。贡献路径为
清单拥有。清单是 A3S ACL (`.acl`)，必须使用以下命令进行解析
[`a3s-acl`](https://github.com/A3S-Lab/ACL); ACL 不是 HCL。

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

`route` 属性是可选的，仅作为面向人的 CLI 保留
别名。它不需要是唯一的并且从不拥有安装状态，
接受调用租约、游标包标识或工具/MCP 主机名。自动化
应按 `<publisher>/<name>` 处理包，并按规范类型处理表面
和表面 ID；不明确的别名查找失败关闭。

|表面|套餐贡献 |准备就绪所有者|
| ---| ---| ---|
|工具|包本地本机任务或摘要固定任务/服务版本 |签名的规划启动器加上本机提供程序，或明确选择的运行时 |
| MCP|本地包 stdio 服务器或摘要固定 HTTP 版本 |签名的 stdio 启动器加上本机提供程序，或运行时/网关准备就绪 |
| OKF |开放知识格式概念图|知识宿主阶段、提升、观察、引用检索|
| A3S流程|具有明确表面边缘的 TypeScript 工作流程源 | `a3s-flow` 预检和精确编译绑定 |
|技能|规范表面 ID 加上内容绑定 `SKILL.md` 和支持文件 |所需依赖准备好后进行静态投影；主机将清单 ID 与从文档解析的表示元数据区分开来。
|用户界面|完整性绑定静态入口点 |生命周期验证条目和准确的资产摘要，使用版本化完整性标记来投影规范排序的技能/工具/MCP/流程依赖集，仅发布完整的依赖证据，并在删除时清除收据拥有的预测。沙盒、渲染、状态和后端绑定仍由主机拥有 |

表面可以选择进行投影，但它们不是独立的
在其自己的软件包生成之外安装、升级或删除。

## 一个 A3S Flow 生命周期

A3S Use 不定义第二个工作流引擎。

- 包清单声明了 `flow` 表面、源摘要、导出和
  工具/MCP/OKF 依赖性。
- `a3s-flow`拥有编译和执行语义。
- 主机可以使用`flow.json`作为可视化设计或部署文档，但它
  不是另一个包裹收据、依赖性解析器或生命周期日志。
- A3S Code是本地主机，A3S OS可能是远程执行目标；两者
  必须解析相同的包拥有的流标识。

当嵌入主机未注入时，所需的流发布无法关闭
声明的 Flow 运行时。没有源存在或 `PATH` 后备。的
独立 CLI 选择使用相同的经过审查的绝对编译器路径
跨进程重新启动安装、升级和卸载：

```bash
A3S_FLOW_NATIVE_TS_COMPILER=/opt/a3s/bin/a3s-flow-native-compiler \
  a3s-use install acme/workflows \
    --scope-kind workspace \
    --scope-id workspace/acme-project \
    --registry-name packages \
    --json
```

`CognitivePackageManager::new` 保持无提供者和确定性；
`CognitivePackageManager::from_env` 是显式独立组合。
丢失或失败的编译器会留下已安装禁用的候选收据，
但不可变的功能快照仍保持其上一代的样子
并且不投影暂存包状态。生命周期诊断保留
有界失败证据。修复后的重试恢复相同的承认计划并且
准确的包生成，然后发布一项经过审查的功能切换
而不是猜测或暴露部分状态。

## 可替换的注册表和精确的锁

注册表 URL 和信任根是主机输入，永远不会编译到解析器中。
主机可以选择镜像、私有注册表或另一个明确信任的 TUF
源代码无需更改包逻辑。每个依赖项都可以从
不同的启用源，但同一个包出现在多个启用中
来源因含糊不清而被拒绝。

目前的注册规则：

- 托管主机首先从提供的内容中获取准确的摘要/版本/大小证据
  通过无状态`inspect_bootstrap_root`的字节，然后固定那些相同的
  字节通过`TrustedRegistry::pin_trusted_root`。两个 API 共享一个
  公共 one-MiB 绑定和解码器；固定还强制执行配置的
  摘要、常规文件检查、元数据锁定和之前的不可变重播
  普通刷新执行完整的TUF链、过期和回滚
  验证。
- TUF 目标`custom.a3s` 元数据包含一条完整的catalog-v3 记录。
- 每个可执行目录都带有一个单独签名的`planning-v1.json`
  目标。它将本地包工具/stdio MCP 启动器与
  下载存档之前发布支持的运行时工作负载。
- 混合包被计划为一个精确的提供程序集：本机工具任务和
  stdio MCP 保留在内置启动器上，而版本支持的工具任务，
  工具服务和 HTTP MCP 需要来自类型化的显式主机分配
  `RuntimeClientRegistry`。缺少拨款、代际、任务或
  提供者失败而没有后备。
- 提供者选择是两次通过的。能力预检暴露真实情况
  提供商执行主机策略；最终通过绑定规范格兰特
  语义，并且必须保留相同的提供者 ID、构建、规范化
  能力和执行力。最终的政策决定也必须保留
  不变。
- 安装的 schema-v6 收据保留确切的安装 ID，可选
  非拥有的 CLI 别名，以及签名的规划包每个可执行包。因此，可以在之后再次审查启用情况
  无需咨询可变注册表即可重新启动，而目录、清单和
  已安装的包字节仍会重新验证。
- 应用时主机适配器从不可变的重新派生授予提案
  审查计划和持久快照，重建准确的运行时选择，
  并要求提供者证据逐字节匹配。共享A3S CLI，
  TUI 和托管主机支持路径保留重建输入
  而不是进程本地客户端。
- 退休永远不会选择新的激活提供商。禁用、卸载和
  上一代升级清理重新打开由确切记录的提供程序
  运行时绑定收据；提供者 ID、构建和标准化功能
  在服务被耗尽和删除之前重新检查。
- 版本支持的运行时任务绑定使用当前的独立收据：
  无参数审查的运行时模板，Grant/descriptor/provider
  证据、捕获契约和精确的生命周期生成过程
  重新启动而不依赖于短暂的操作记录。每次调用
  仅派生其唯一的单位 ID 和有界 argv，重新打开收据拥有的
  提供商，并通过输出持有确切的发布发电租约
  捕获和清理。隐藏的或被取代的一代拒绝新的呼吁。
- 运行时任务发布和调度还会交叉检查持久绑定针对已安装软件包保留的规划证据。受注册机构信任
  软件包必须保留目录绑定的签名规划包和确切的信息
  释放描述符摘要；一个自洽但可替代的描述符，
  包生成，或者丢失的证据在之前被遗漏或拒绝
  提供商连接。
- 目录记录、存档、扩展包和清单均具有准确的
  摘要/大小证据。
- 存档准入将每个计划启动器重新绑定到确切的摘要绑定
  `.acl` 清单和释放描述符；表面种类、激活、可执行、
  argv、命令、超时和传输漂移无法关闭。
- 准备好的下载和安装的注册表/TUF收据必须保留完整的
  经验证的目录记录及其出处。
- 在线准备将源观察保持在
  `<registry-datastore>/verified-targets/sha256/<digest>.json` 并提交
  向全球核实档案、规划目标和演示媒体
  分片 blob 层。缓存读取拒绝链接和非常规文件，重新散列
  blob 通过保留的句柄，并在接纳之前验证签名的长度。
- 显式缓存的解析重新验证最后一个可信的、未过期的 TUF
  元数据和准确的注册表名称、URL 和信任根。它永远不会刷新
  网络，并且永远不会削弱源或包锁的来源。
- 类型化的每个注册表策略限制逻辑引用字节、观察、
  和部分并保留源/暂存磁盘空间。消化结合部分内容可以在进程中断后幸存下来，只能通过精确的 HTTP 恢复
  范围响应，并且在完整签名长度和 SHA-256 之前不会上演
  验证。自动和确认的源清理会删除陈旧的写入，
  然后是同一缓存锁下最旧的部分和观察。它从来没有
  将源引用删除视为全局 blob 删除。
- 实时进程恢复覆盖范围也会在验证期间终止安装
  存档提取，证明没有收据，安装快照，待处理
  操作，或包根已发布，并完成显式
  从重新验证的缓存进行零网络重试。
- 以下实际进程包复制中断保留其准确的
  待定计划和应用日志，但没有收据、安装快照或
  包发布。离线重播回收物理`.artifact-staging-*`残渣
  并仅发布一次经过审查的生成。
- 实时进程卸载中断重播准确的生命周期标识，
  完成范围收据和权限退休，保留全局工件
  字节，并且不会再次推进注册表生成。
- 原子注册表图后实时多节点安装中断
  出版物保留了一个完整的可见闭合及其持久的切换，但
  没有安装快照。离线重播完成每个包日志，
  写入准确的快照，取消切换，并保留注册表生成
  1 无网络请求。- 观察者无需等待作者就可以阅读不可变的出版物。如果一个
  一次性崩溃协调短暂拥有注册表锁、生命周期
  写入者异步等待最多两秒；真正并发
  `use.extension.busy` 突变仍然失败。
- 已安装的收据仍与其源名称、URL、根摘要绑定，
  发布通道、目标和 TUF 角色版本。
- 替换源配置永远不会重写已安装的收据来源；
  升级之前需要恢复确切的源或重新安装。

规范包锁冻结选定的版本、依赖边缘、主机
目标、`requires_use`、存档和包摘要、注册表标识和 TUF
每个节点的出处。解决方案失败，循环关闭，不兼容
约束、缺少提供者、源模糊性和配置的搜索范围。

## 审查生命周期

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

安装、升级、卸载、启用和禁用是持久操作。申请
重新验证确切的包锁、目录证据、主机功能，
政策权限、范围、确认以及突变前的现状。
升级计划绑定优先锁和候选锁，并对每个节点进行分类
如 `Add`、`Replace`、`Remove` 或 `Retain`。

管理激活和退休有意使用不同的证据。安
启用或候选安装/升级使用主机拥有的两遍提供商选择。
禁用、卸载或上一代升级清理不包含候选项
选择并取消确切的收据拥有的绑定。如果停止的绑定是
使用新的授权语义重新启用，旧绑定之前已停用
同封装代反弹；冲突的不可变收据是
从未被覆盖到位。

管理器 MCP 工具集将只读计划与突变分开公开：

```text
plugin_plan_install     plugin_plan_upgrade     plugin_plan_uninstall
plugin_plan_enable      plugin_plan_disable     plugin_apply_plan
plugin_observe_operation plugin_watch_operation plugin_cancel_operation
```

`plugin_apply_plan`是唯一的管理器包状态突变入口点；
`plugin_cancel_operation` 是一个单独的预准入控制平面突变
无法发布包生成。 `NoChange`启用结果是
末端并且没有合成突变身份。崩溃
恢复恢复精确存储的计划和授权；重读已完成的
操作返回其持久结果，而不会重复产生副作用。
应用和回滚记录均保留独占操作所有权；
在到达终端之前，不同的意图不能取代任何一个意图
记录。检查读取同一记录下的最新和以前的记录
包范围的日志锁。

`PluginManagerService` 现在是共享类型应用程序边界
`CognitivePackageHostManager`。它拥有确定性的请求身份，
注册表绑定搜索游标、稳定的安装状态分页、SemVer
安装/升级选择，所有五个规划路径，持久计划重新开放，
和仅摘要适用。 `PluginManagerMcpServer`暴露了确切的13个v5
通过标准MCP初始化`tools/list`和`tools/call`的工具；它的
模式和注释是从冻结的工具集中生成的。 MCP 申请并
取消请求注入的可信主机确认提供商现有的
确切的证据，并且永远不要将代理工具调用视为用户确认。独立的 CLI
注册表支持的安装、升级和卸载突变使用此服务并且
公开经过精确审查的主办方计划/结果及其发布的输出
字段。它的`plugin`表面将所有十三个管理器操作映射到同一个
服务，保持每个计划只读，公开精确的操作观察/观察，
并需要准确的操作 ID、计划摘要和显式 `--yes`
申请或取消。代码 TUI `/packages` 和代码端
Manager MCP 现在使用相同的服务。人类 CLI 和 TUI 审查派生出一个
来自不可变管理器信封的确定性只读投影以及
显示确切的计划身份、候选/先前包图、来源、
过渡、完整的许可上限、提供者/影响/状态证据，以及
确认边界而不改变机器 JSON。 TUI 滚动完整在确切申请之前进行审查。此资格登陆 A3S CLI `main` 提交
`bef7c913cbefba62638b37f91ce9263f4db2ffbb`；持续集成运行
[32786647662](https://github.com/A3S-Lab/CLI/actions/runs/32786647662)
通过了所有五个主要的 Linux、macOS 和 Windows 作业。六面体
产品主机 E2E 仍然是一个发布入口。

主机协议 v6 绑定显式用户或工作空间范围类型和项目
确切的运行状态仅来自持久的证据。相同的文本范围 ID
不同类型不能共享栅栏、计划、请求重放记录或主机
操作。该协议报告事实阶段和有界检查点计数
而不是发明的
百分比，将每个状态修订绑定到完整状态，支持
基于修订的长轮询，并且仅接受显式用户取消
在持久图表或启用许可之前。出版取消
太晚了；只有持久的主机结果报告`Completed`。

A1 两次安装资格矩阵驱动相同签名的 OKF 包
通过安装、主机重启、精确能力快照、租用查询、升级、
并发用户和工作空间安装中的卸载和终端重放
具有相同的文本 ID。每个突变都会留下其他安装的光标
不变且保留的租约可调用，而不可变的包字节是
通过共享 Artifact Store 进行重复数据删除。

生产托管主机适配器仅存储协议请求/操作
绑定和终端投影。它不会创建第二个包，
授权或恢复状态机。过期的计划仍然无法使用
除非用户拥有的确切证据证明它已经被承认或
在原来的审查窗口内完成；仅仅是有计划的行动必须
重新计划和审查。

工作空间补助金被组成相同的图形传奇。候选人补助金是
在包准备之前保留，记录准确的注册表切换，
在先前的授权被撤销以及预切换失败之前，已接受的呼叫已耗尽
将一揽子计划和格兰特候选人重新组合在一起。

## 架构

<p align="center">
  <img
    src="assets/readme/architecture.svg"
    width="100%"
    alt="Trusted sources enter one reviewed Plugin Manager and A3S Use graph lifecycle before an atomic capability snapshot reaches A3S hosts"
  />
</p>

|边界|拥有 |不拥有 |
| ---| ---| ---|
|主机插件管理器 |注册表配置、信任根、策略、用户确认、审核计划/申请 |包字节或提供程序调度内部结构 |
| A3S使用|验证、精确锁定、不可变代、收据、拨款、生命周期日志、切换证据 |通用调度或UI渲染|
|运行时/网关 |工具和 MCP 提供程序执行、运行状况和消耗 |包解析或信任策略 |
| A3S流程|工作流编译、执行、重放和观察 |并行包生命周期 |
|知识主持人| OKF 验证、索引、提升、引用搜索 |流程执行 |
| A3S 代码/操作系统 |产品用户体验、工作区/会话范围、渲染、注入提供程序 |第二个包管理器 |

参见【插件平台架构](docs/plugin-platform-architecture.md)，
[生命周期和安全性](docs/plugin-platform-lifecycle-and-security.md)，
[ADR-002](docs/adr-002-cognitive-package-lifecycle-saga.md)，以及
[控制存储事务边界](docs/adr-003-control-store-transaction-boundary.md)。
经机器检验
[协调割接库存](docs/control-store-cutover.md)对每个
当前状态叶子、外部所有者、操作文件和消费者必须
一起切换；它明确地使生产激活保持不活动状态并且
禁止双重写入或传统后备读取。
私有 A2 Control Store 内核现在符合其干净状态 schema-v11
聚合。每个操作都存储规范的完整审查计划信封
和版本化授权证据，然后派生并重新验证其操作
ID、计划和授权摘要、操作、根包、安装范围、
并在重新启动后和期间针对关系投影生成游标
离线出口验证。授权证据 v2 仅保留准确的
先前的拨款快照、审查的变更集和确认事实；已解决
赠款及其接收修订是派生输出，而不是调用者权限。
安装生成、所需的包状态生成、不可变
包生命周期生成和拨款收据修订仍然不同。
在提交之前，内核会重建完整的目标快照，两者
包生成轴，以及完整的目标授予库存
审查了计划、确切的上一代、有限的承诺历史，并审查了
授予证据。所有五个操作、用户和工作空间安装、多根目录共享依赖项，卸载/重新安装因此拒绝调用者选择
包或授予身份。离线导出和恢复验证程序运行
再次相同的投影。
该投影还重建了完整审查的运行时提供程序
每个启用的工具和 MCP 曲面的选择。保留了不相关的
选择，删除禁用或删除的表面，存储完整的提供者
构建/能力/语义/执行证据，并得出每个选择
从经过审查的计划证据的版本化规范描述符中摘要。
Flow、OKF、Skill 和 UI 保持类型化主机
效果而不是被分配虚构的运行时提供者。一个单独的
候选能力摘要源自目标快照、包
生命周期身份、拨款修订和提供者选择。它描述了
仅承诺所需的能力身份；终点、就绪、编译
工件和知识应用观察结果仍然是提交后的证据。
相同的投影得出完整的有界工作序列，而该序列不能
加入本地事务：表面准备、能力割接、
接受呼叫排水，以及表面停止或去除。依赖面准备
受抚养人之前；退休则颠倒了这一顺序；升级为新做好准备
切换前的化身，并在移除前耗尽旧的化身。
每个效果都命名一个类型化的能力索引、调用租赁、运行时、流程、知识、技能或 UI 所有者。工具和 MCP 效果经过精确审查
提供商 ID 和选择摘要；静态主机永远不会收到虚构的
运行时选择。可选的选定表面可能会在切换前降级，但是
他们所需的依赖性关闭和每项退休效应仍然是必需的。
软件包选择、生命周期身份、资助和经过审查的提供商选择
已经在聚合中提交，因此它们不会被复制为伪外部
影响。规范有效负载字节，其域分隔的幂等密钥，
摘要和关系投影一起提交并在之后再次验证
重新启动并通过离线导出验证。应用结果仍然是规范的，
所有者特定的证据，而不是任意的成功摘要：能力
索引收据现在绑定准确的不可变的面向代理的目录
摘要/生成/修订、调用租赁收据、精确的运行时选择以及可移植
任务或不透明`gateway:`服务绑定/就绪证据，流程工件
摘要、知识投影摘要和不可变的技能/UI 内容摘要。
每个应用程序都会重新绑定确切的幂等性密钥和意图。延期，
被拒绝，未知结果仅保留诊断证据。推迟的是
保留给证明其不接受任何影响的所有者；它持续有界
不早于使用相同密钥自动重试的时间。记录应用的
能力切换观察淘汰了先前的出版物，发布了精确的候选者，并通过该目录绑定移动功能光标
在同一笔交易中，
在排水、报废或操作完成之前。所需的后切换
因此，失败仍然处于协调待定状态，并且必须重用其原始状态
身份；它无法回滚已经可见的一代。完成不能
早于任何提供者观察。内核还限定类型化生成
转换、完整的授予和审查的提供者选择证据、幂等
发件箱协调、有限执行、损坏检查和确定性
可离线验证的导出以及分阶段恢复。现在它的调度程序不活动
持有一个安装范围内的共享维护围栏，从索赔到
后来的观察，最多声称一个承诺的影响，释放该声明
输入所有者之前的交易和有界执行者、路线能力
索引、调用租赁、
运行时、流程、知识、技能和 UI 通过单独的类型端口工作，然后
记录特定于所有者的应用、推迟、拒绝或未知证据
以后的交易。延迟效果在其持久之前无法收回
不早于时间，然后使用相同的密钥自动重试。供应商
超时必须在其声明租约内留下固定的观察预算；超时
是持久的未知证据。超时或取消仅分离等待，
不是可能接受的效果任务：该任务保留相同的共享栅栏直到它真正完成。进程退出仍然需要显式的相同密钥
和解。测试证明
commit-before-effect、提供程序 I/O 期间存储重新输入、精确密钥恢复
在未观察到的进程退出、挂起提供者边界和所有七个所有者之后
路线。并发的整个安装恢复无法获取其独占的
维护围栏，直到提供者观察持久并且任何分离
进程中效果未来已完成。索赔要求
交易现在还衍生出所有者形状的承诺
上下文：包端口仅接收确切的包选择、生命周期、
主机、快照身份和授予； Runtime 也接受了全面审查
供应商选择；能力指数接收候选生成加上
保留的每个启用的选定表面的最新终端准备
多根历史。可选拒绝是显式降级，同时缺失
授予覆盖范围、非终止或拆卸状态以及生成漂移失败关闭
在所有者 I/O 之前。多根测试还修复了生成插入，因此所有
包节点位于其直接外键依赖边之前
相同的交易。调度程序不是按生产生命周期构建的
代码，并且不会在当前 JSON 存储之外创建第二个权限。
第一个具体的提交后所有者适配器现在符合不可变技能和
针对该边界的 UI 准备。它重新派生键入的所有者并来自可移植请求的幂等性密钥，仅获取确切的包
通过经过验证的工件租赁，读取一个命名表面而不暴露
包根，并在返回之前重新验证完整的包
稳定的无路径收据。索赔尝试和截止日期不会改变这一点
收据。工件争用是持久的同密钥延迟；篡改，
内容缺失或权威替代属于无效拒绝；
只读适配器从不报告不明确的接受情况。静态停止和
删除是与路径无关的投影收据，因此仍然可重播
神器收集后。第二个混凝土适配器现已符合 OKF 资格
知识反对相同的承诺边界。第一次准备工作消耗了
无路径、完全验证的 OKF 字节有效负载；阶段收据拥有的 SQLite/FTS5
状态；保留晋升前的阶段性证据；坚持推广证据
在应用报告之前；并返回准确的观察结果和能力
投影摘要。保留的促销收据会重播，无需重新打开
神器商店。预效应争用安全推迟、权限或字节漂移
拒绝以及任何不明确的阶段、提升、删除或接收持久性
对于显式相同密钥协调，边界仍然未知。停止是一个
路径无关的检查点和删除仅使用保留的投影
收据。现在，成分测试通过以下方式证明了承诺的控制声明：真正的知识适配器并返回到持久的控制应用程序
观察。工件准入是单独幂等的并重新验证
准备好源，同时创建无安装生命周期收据；来电者必须
通过单独的机构保留其全球参考准入警卫
提交。第三个具体的能力平面适配器现在拥有这两个能力
索引发布和调用消耗。验证承诺权限后，
它调用主机拥有的纯投影仪，拒绝启用外部的描述符，并且
成功准备包化身，持久发布确切的代理
目录，并具体化一个规范的内容寻址索引文档，该文档
对该出版物具有约束力。没有第二个 SQLite 数据库或可变 `current` 文件
创建的。应用的割接观察仍然是唯一的出版物
事务并通过目录推进控制光标
摘要/生成/修订。调用准入重新开放并重新讨论这些
精确字节，验证索引，读取共享周围的控制发布
锁定每个精确的包生命周期化身，如果
切换赛跑。 Drain首先证明旧的化身不再
发布，然后安全地推迟，同时任何接受的调用保留其共享锁；
释放后应用相同的效果键。目录和索引出版物是
不可替换、不可跟随、可崩溃重播且无路径。指数得出不包括在备份中的运行状态；现在协调的国家库存
注册并在语义上验证目录和描述符快照记录
作为一个 `CapabilityPayloads` 系列，同时锁定、暂存、日志和租赁
文件仍被排除。 `ControlCapabilityPayloadRestoreCoordinator` 现在绑定
一个专属维护围栏下的目录和描述符计划，
预检两个干净目标，并重试固定顺序激活，无需
打击已经出版的所有者。 `ControlCapabilityPayloadRetentionCoordinator`
现在将两个所有者保留计划绑定在同一个专属围栏下，
预检库存（包括确切的待处理日记）和重播
固定顺序删除。非活性组合物现在保留了相同的
能力平面，并且可以在经过一段时间后重新打开持久发布的控制光标
重新启动而不接受调用者选择的光标。重新开放重新验证
准确的索引和目录，重新获取每个包生成租约，并返回
如果并发切换获胜，则过时。生产控制所有者注册，
从返回的租约、租约消耗和
生命周期保留权限仍然是独立的。一个真实的
作文测试加入知识、技能、目录/索引出版、精确
有效负载准入、过时准入和耗尽。
非活动组合现在接受规范的认知包计划
信封、授权证据和可选的计划拨款过渡一次性完成
生命周期准入缝。它派生出先前的安装和能力来自不可变计划的游标而不是接受调用者选择的值。
其组合组合入口点保留了一个安装范围内的围栏，同时
它注册准确的审查操作，发布运行时计划有效负载，
并在任何提供商生效之前承诺预计的发电量。生产
仍然必须通过这条接缝路由实时生命周期并组成
调度员。非活动内核现在也有一个
提交权限流所有者：它读取有界流
源作为无路径验证的 Artifact Store 有效负载，发布持久的
所有者控制的工作区中的无破坏内容寻址副本，并调用
仅类型化的 `a3s-flow` Native TypeScript 预检。包路径永远不会交叉
该边界；编译器/缓存路径是可操作的主机配置，而不是
比期望的国家权威。源替换和失败的预检拒绝
没有控制观察，而 Artifact Store 争用安全地推迟。
停止/删除是与路径无关的收据。该资格仍处于无效状态
直至生产调度员组成被切换。坚定的权威
运行时所有者现在符合发布支持工具的相同边界条件
任务、工具服务和可流式 HTTP MCP。首先准备仅消耗
无路径验证工具/MCP 发布有效负载和显式运行时选择
其提供者和完整语义摘要与提交的控制权限相匹配。
任务在不启动单元的情况下保持独立的绑定。服务第一坚持`requested`，然后保留准确的运行时和类型网关准备情况
证据，并在删除恢复权限之前提交最终绑定。
准确的最终收据重放，无需访问 Artifact；保留的终端
配置记录无需另一个运行时应用即可协调；停止/删除使用
仅收据拥有的提供商、网关和生成证据。预效果
争用被推迟，无效的权限或不可变的字节被拒绝，并且
运行时/网关效应后所有持久性或协议模糊性仍然存在
未知。运行时包现在还公开了有界规范
`RuntimeSurfacePlan` 有效负载和 `CommittedRuntimeSurfaceResolver`，其中
重新启动后重建完整计划并重新检查提供商证据。这个
所有者仍然仅限资格：生产成分必须提供
持久主机源和原子调度程序而不是保留进程本地
选择作为权威。
然后，非活动控制成分证明仅接受已注册的
操作身份和主机生成的不可变计划有效负载。它投射了所有
Control 内的可变转换字段，验证准确的运行时发布
授予权限，并在生成提交之前命令计划发布
在一个共享安装围栏下。这缩小了割接边界，而无需
使私有内核或遗留消费者处于生产活跃状态。
现在的内核
也符合无路径的条件
外部有效负载注册和
快照证据边界。其六个冻结的所有者身份和固定备份根据 ACL 切换清单检查策略。全球神器
商店被明确排除在外，而其他五位业主必须出示一份
完整、规范订购的收据集与确切的安装绑定，
控制生成、注册表摘要、所有者模式、库存/清单摘要、
和有界文件/字节记帐。解码的证据在其被重新验证之前
描述符摘要可以被接受。私有快照会话现在冻结一个
规范控制导出及其摘要，同时保留相同的专有性
跨所有者 I/O 的维护栅栏，不保留 SQLite 事务或
商店执行人许可证。知识所有者适配器对范围本地进行快照
OKF SQLite/FTS5 知识数据库转换为非覆盖有界存档，
导出规范的绑定/选择库存摘要，并重新验证
离线存档。现场收据开具和线下受理均需要
由快照绑定命名的相同规范控制导出字节。每个
保留的知识化身必须源自其精确的控制准备
意图和承诺的 OKF 捆绑包；使用的制剂必须与保留的制剂相匹配
知识观察和能力预测摘要。这个连接反对
写入目标存档之前的临时 SQLite 快照，因此
语义不匹配不会留下存档或收据。以前被删除或丢失
应用的有效负载需要相同的生命周期记录的删除效果，而延迟结果仍然是安全无影响的调度证据，同时声称或
未知的结果仍然是调和的证据； none 是新的期望状态
权威。不存在的知识数据库会产生显式的
零文件清单，无需创建实时目录；舱单和收据
不包含主机路径。离线验证的知识快照现在可以流式传输
将其精确数据库写入调用者拥有的、州根本地候选人中，无需
触摸实时有效负载。清洁目标激活需要精确的
安装专属维护卫士，重新审核候选者及其
绑定/选择库存，拒绝无主、现有或不明确的有效负载
状态，并通过一个原子重命名来发布。精确完成的部分是
可重播。在保留相同的阶段性尝试和独家守护的同时，
发布后但在返回规范的无路径结果之前重试
协调准确的实时数据库。缺少有效负载激活不会产生任何影响
知识状态。第二个类型的适配器现在快照
规划和诊断观察所有者。它仅存档经过所有者验证的
终端诊断历史和终端解决尝试；活跃的
分辨率和下载尝试加上操作锁定永远不会恢复为
权威。确切的活跃库存数量和摘要仍然与
明显。安全有界遍历拒绝链接、移动或外部记录，
未知的布局、重复的包标识和文件/字节溢出。档案创建是无破坏的，在发布之前重新扫描实时状态，并发出
可以离线验证的无路径控制导出绑定收据。安
离线验证的观察快照现在可以将其精确的存档复制到
state-root-local 暂存目录，无需触及实时所有者路径。第一
激活需要一个干净的终端/活动记录清单和准确的
独占维护守卫，然后自动将存档候选更改为
发布任何记录之前的 `activating` 标记。摘要命名确定性
部分使中断的每条记录发布可重播；只有一个精确的
激活开始后接受快照子集。候选人、目标、
链接、活动记录和存档漂移无法关闭，锁仍被排除在外，并且
规范结果不包含主机路径。两个适配器均保持不活动状态
资格代码并且未连接到当前备份或恢复扫描仪。
主机协议投影现在是第三个合格的快照，并且
干净目标恢复适配器。它的所有者本机扫描仪档案仅是不可变的
请求计划记录、可选的最终结果和一个规范
每个确切操作绑定的取消。操作别名和
最新启用的诊断索引仍然是派生的：它们必须完整并且
同意他们的来源请求，但绝不进入档案。有界
无跟随遍历、第二次实时扫描、无破坏发布和精确离线解码拒绝链接、移动、丢失、陈旧或孤立的记录
存档替换。出版前、主办计划、完成/取消
证据、包装身份、所需状态、选定的表面，以及
包/功能生成必须可从精确绑定的控制中导出
出口；主机收据和健康证据仍处于观察状态，无法选择
期望的状态。清单和收据是无路径的，代表缺席
明确地，并保留无更改请求而不进行操作。
离线验证的快照现在可以暂存一个私有存档副本并构建一个
完成目标状态根下的`plugin-host-manager`候选。它
恢复精确的语义源字节，仅重建规范的精确的
操作和最新启用索引，并故意省略旧别名
并锁定文件。激活需要精确目标的独家维护
守卫和不在场的活所有者根，重新验证确切的树和
所有者本机语义扫描，记录快照绑定的持久激活标记，
并通过一个原子的无破坏目录移动来发布整个所有者根目录。
存档、记录和激活标记部分可确定性恢复；
发布后/结果前重播仅接受完全相同的快照。
候选、live-root、链接、存档和标记漂移失败、关闭、缺席
不创建所有者根目录，并且结果不包含主机路径。这个适配器资格代码仍处于非活动状态。恢复协调员现在是第四位
合格的快照所有者。它的所有者本地期刊扫描仪仅存档准确的、
对绑定安装进行规范编码的已完成恢复操作。
活动标记及其确切操作被排除在有效负载权限之外，
但它们的有限计数和摘要库存仍然受舱单限制；仅标记
切换是在不发明历史的情况下进行的。孤立的非终结记录，
修剪或临时状态、未知条目、链接、外部安装以及
路径/记录重新绑定失败关闭。在无破坏存档之前进行第二次扫描
发布，无路径收据和流式离线验证器绑定
结果精确控制导出。空的或仅活动的历史记录不会创建
存档。离线验证的快照现在可以构建不可变的候选者
在目标安装状态根目录下。因为当前恢复拥有
同一份日记，激活故意不是干净目标合并：
需要精确的专属维护防护和主动标记，标记
和当前操作被保留，并且仅替换终端历史记录。
持久的激活描述符绑定快照，稳定的活动身份，
以及准确的之前/目标库存。现有终端目录已移动
在候选人记录发布之前保留暂存墓碑，无需
更换。 Replay 容忍主动操作前进，同时拒绝标记漂移、链接、未知状态、候选者或墓碑篡改，以及
无法解释的实时变化。支持仅标记切换和缺席历史记录。
传统的整体安装标记保留了活跃操作的未来
终端槽，因此 64 条记录的源确定性地丢弃相同的本机
期刊将删除的最旧记录。打字的全套标记没有
保留操作，因此保留所有 64 条源记录。规范的
结果是路径自由且受快照限制的。此资格仍处于非活动状态
代码。运行时计划所有者现在快照不可变的安装范围计划
记录，验证其完整密钥和规范信封，并恢复它们
主机投影激活之前；引用的运行时工件摘要也是
包含在安装工件可达性证据中。私人完整-
设置快照协调器现在捕获规范控制导出和所有五个
注册所有者快照在一个确切的
维护围栏和时间戳。它绑定固定所有者集、收据、
摘要、模式和字节记帐在一个无路径的规范清单中，
将它们流式传输到每个使用数据和状态之外的单个无破坏存档中
root，并重用每个所有者本机验证程序来审核整个暂存文件
发布前离线。缺席的业主贡献收据但没有发明
有效负载字节；全局 Artifact Store 保留在安装备份之外。存档标头、清单、长度、有效负载摘要、尾随字节、链接、漂移、
重新绑定和覆盖失败都失败关闭。这个全套作家是
也是无效的资格代码。离线验证的完整快照可以
现在将控制数据库和所有五个候选所有者放置在一个固定的
`.control-installation-restore` 目录，同时保留确切目标的
专属维护围栏。一种规范的无路径尝试描述符绑定
快照、安装、所有者注册表、知识存储策略，以及
在候选 I/O 开始之前固定组件集。控制候选人必须
往返于精确的规范导出、检查点到一个 SQLite 文件，以及
匹配其持久字节摘要；每个外部候选人都会被建立并重新检查
由其所有者本地适配器在同一保护下进行。出席和缺席的业主，
已完成的重试和中断的控制分级是确定性的，而
非空目标、未知或链接条目、快照/策略重新绑定以及
已完成的候选漂移失败，无需触及实时权威路径。
全套协调员现在有资格整个跨所有者激活
协议。在持久意向之前，每一位在场或不在场的业主候选人都是
针对干净的目标重新验证。不可变的尝试描述符仍然存在
恢复身份； `activation.json` 是唯一的可变日志，并且
键入的全局 `.maintenance.restore.json` 标记会尝试绑定到一个
不可变的激活操作。固定所有者顺序是 Control Store、Runtime计划、主机预测、知识、观察，然后恢复协调员。
每个步骤都使用
相同的日志标记效应检查点规则，并且每个检查点都绑定
规范的无路径所有者由长度和域分隔的摘要得出。
恢复协调器接收准确的预期标记字节、长度和
在它改变历史之前消化它。只有第六个持久检查站允许
全球标记退休。重新开放重新获得确切的专属守卫，
重新绑定相同的已验证快照、尝试、所有者注册表和知识
策略，并重建或验证每个所有者的确切候选/实时状态
边界。日志和标记部分，每个所有者在检查点之前效果，
标记删除之前的最后一个检查点，之后进程立即退出
标记删除，并在每个固定顺序分期退出后退出全部收敛
确定性地。 21 边界子流程矩阵运用这些顶层
退出。仅在完整的六个检查点的情况下才接受缺失的标记
日记；不明确的标记、无序的活根、快照重新绑定、链接
路径或证据漂移未能关闭。完成的重播不执行所有者效应；
它只能恢复六个无链路暂存树的有界退休。的
幸存的规范 `attempt.json` 和完整的 `activation.json` 形成了精确的
安装绑定的终端收据。旧版备份和工件可达性
仅排除该两文件收据；不完整、扩展、链接或篡改证据失败关闭。生产补助金转换、运行时/流程调度程序
组成、备份/恢复命令
布线、不可分割的消费者切换以及遗留可变存储的删除
保持开放。
研究预览
[MHS集成配置文件](docs/mhs-integration.md)定义了硬件适配器
边界，无需添加另一个包表面或协议分支。

## 当前合同基准

仅接受以下认知包协议行：

|合同|当前架构 |
| ---| ---|
|包裹清单 |架构版本 `3` |
|注册表源码配置| ACL 架构版本 `1` |
|签名目录记录 | `a3s.use.plugin-catalog.v3` |
|安装收据|架构版本 `6` |
|安装快照 | `a3s.use.installation-snapshot.v2` |
|运营计划| `a3s.use.plugin-operation-plan.v4` |
|主机能力| `a3s.use.plugin-host-capabilities.v6`（协议`6`）|
|主机托管范围| `a3s.use.plugin-managed-scope.v2` |
|主机运行观察| `a3s.use.plugin-host-operation-observation-request/result.v1` |
|主机操作观看| `a3s.use.plugin-host-operation-watch-request.v1` |
|主办方取消 | `a3s.use.plugin-host-cancel-request/result.v1` |
|经理 MCP 工具集 | `a3s.use.plugin-manager-tools.v5`（v4 迁移合约仍然可读）|
|待处理包裹图 | `a3s.use.pending-package-graph-operation.v4` |
|锁定前解决方案尝试 | `a3s.use.plugin-resolution-attempt.v1` |
|预先计划下载尝试| `a3s.use.plugin-download-attempt.v1` |
|生命周期诊断 | `a3s.use.plugin-lifecycle-diagnostic.v1` |
|运行诊断| `a3s.use.plugin-operation-diagnostic.v1` |
|操作历史 | `a3s.use.plugin-operation-history.v1` / `a3s.use.plugin-operation-history-diagnostic.v1` |
|预锁分辨率诊断| `a3s.use.plugin-resolution-attempt-diagnostic.v1` |
|预先计划下载诊断| `a3s.use.plugin-download-attempt-diagnostic.v1` |
|启用恢复预测| `a3s.use.cognitive-package-enablement-projection.v3` |
|赋能运营| `a3s.use.cognitive-package-enablement-operation.v3` |
|扩展注册表快照 |架构版本 `3` |
|扩展快照光标 | `a3s.use.extension-snapshot-cursor.v3` |
|能力快照 |架构版本 `5` |
|能力快照光标| `a3s.use.capability-snapshot-cursor.v4` |
|能力描述符 | `a3s.use.capability-descriptor.v1` |
|签署的能力描述 | `a3s.use.capability-description-signature.v1`（Ed25519）|
|控制描述符证据快照| `a3s.use.control-capability-descriptor-snapshot.v1`（仅证明兼容性）/`v2`（签名信封）|
|控制描述符快照保留计划| `a3s.use.control-capability-descriptor-snapshot-retention-plan.v1` |
|控制描述符快照保留结果| `a3s.use.control-capability-descriptor-snapshot-retention-result.v1` ||控制描述符快照保留日志| `a3s.use.control-capability-descriptor-snapshot-retention-journal.v1`（内部）|
|控制描述符快照恢复计划| `a3s.use.control-capability-descriptor-snapshot-restore-plan.v1` |
|控制描述符快照恢复结果| `a3s.use.control-capability-descriptor-snapshot-restore-result.v1` |
|能力网关目录| `a3s.use.capability-gateway-catalog.v1` |
|能力网关目录恢复计划| `a3s.use.capability-gateway-catalog-restore-plan.v1` |
|能力网关目录恢复结果| `a3s.use.capability-gateway-catalog-restore-result.v1` |
|能力有效负载恢复计划| `a3s.use.control-capability-payload-restore-plan.v1` |
|能力负载恢复结果| `a3s.use.control-capability-payload-restore-result.v1` |
|能力有效载荷保留计划| `a3s.use.control-capability-payload-retention-plan.v1` |
|能力有效载荷保留结果| `a3s.use.control-capability-payload-retention-result.v1` |
|能力有效负载保留协调员日志| `a3s.use.control-capability-payload-retention-journal.v1`（内部，可重启恢复相边界）|
|能力消费者概况| `a3s.use.capability-consumer-profile.v1` |
|消费者谈判能力| `a3s.use.capability-consumer-negotiation.v1` |
|运行时任务绑定 | `a3s.use.runtime-task-binding.v4` |
|运行时服务配置| `a3s.use.runtime-service-provisioning.v1` |
|运行时服务绑定 | `a3s.use.runtime-service-binding.v3` |
|神器商店实物库存| `a3s.use.artifact-store-inventory.v1` |
| Artifact Store 摘要审核 | `a3s.use.artifact-store-digest-audit.v1` |
|文物检疫计划| `a3s.use.artifact-quarantine-plan.v1` |
|文物检疫记录| `a3s.use.artifact-quarantine-record.v1` |
|文物检疫结果 | `a3s.use.artifact-quarantine-result.v1` |
|神器补水计划| `a3s.use.artifact-rehydration-plan.v1` |
|神器补水记录| `a3s.use.artifact-rehydration-record.v1` |
|神器补水结果| `a3s.use.artifact-rehydration-result.v1` |
|注册表神器参考盘点| `a3s.use.registry-artifact-reference-inventory.v1` |
|全球神器参考盘点| `a3s.use.artifact-reference-inventory.v1` |
|已加入工件可达性库存 | `a3s.use.artifact-reachability-inventory.v1` |
|协调使用状态备份| `a3s.use.state-backup.v2` |
|协调使用状态备份保留计划| `a3s.use.state-backup-retention-plan.v2` |
|协调使用状态备份保留结果| `a3s.use.state-backup-retention-result.v2` ||协调使用状态恢复计划| `a3s.use.state-restore-plan.v1` |
|协调使用状态恢复操作| `a3s.use.state-restore-operation.v1` |
|协调使用状态恢复结果| `a3s.use.state-restore-result.v1` |
|协调使用状态恢复诊断| `a3s.use.state-restore-diagnostic.v1` |
| OKF知识搜索| `a3s.use.okf-knowledge-search-request.v1` / `a3s.use.okf-knowledge-search-response.v1` |
| OKF知识引文 | `a3s.use.okf-knowledge-citation.v1` |
| OKF知识阅读| `a3s.use.okf-knowledge-read-request.v1` / `a3s.use.okf-knowledge-read-response.v1` |
| OKF知识备份| `a3s.use.okf-knowledge-backup.v1` |
| OKF知识备份保留计划| `a3s.use.okf-knowledge-backup-retention-plan.v1` |
| OKF知识备份保留结果| `a3s.use.okf-knowledge-backup-retention-result.v1` |
| OKF知识恢复计划| `a3s.use.okf-knowledge-restore-plan.v2` |
| OKF知识恢复操作| `a3s.use.okf-knowledge-restore-operation.v2` |
| OKF知识恢复结果| `a3s.use.okf-knowledge-restore-result.v2` |
| OKF知识恢复诊断| `a3s.use.okf-knowledge-restore-diagnostic.v2` |

SemVer 依赖性约束、`requires_use`、操作系统/目标检查以及
主机/提供商能力检查是产品行为，而不是向后兼容性
分支机构。旧的预发布模式和持久状态故意不
迁移了。删除不支持的状态并使用当前版本重新安装。

## 实施状态

网关嵌入主机可以从其中派生出特定于消费者的目录
`CapabilityRegistrySnapshot`通过
`CapabilityRegistrySnapshot::capability_gateway_catalog`；助手验证
公共预测修订加上确切的包/出版物/准备证据
在`CapabilityGatewayMcpServer::from_registry_snapshot`获得其RAII之前
租赁。
对于直播主播，`from_verified_registry_snapshot_with_factory_and_options`
现在组成经过验证的描述投影，游标绑定解析器，
确切的 RAII 租赁、消费者谈判和有限准入政策
一个构造函数；发布竞赛不返回任何服务器。签名验证
和收据/运行时/授予支持的不透明引用解析仍然由主机拥有，
并且产品接线仍处于开放状态。

不活动的控制组合现在具有与其相同的权限连接
自己的光标：`ControlCapabilityGatewayInvocationFactory`接收准确的
仅在逐字节比较描述符后重新打开控制租用
具有耐用目录，并且 `CapabilityGatewayResolvedProvider` 保留了这一点
通过完整的工具、资源或提示操作进行租赁。这保持
控制生成上的不透明参考解析而不是意外
回到旧的注册表解析器；主机厂仍拥有
私人授权/运行时/提供者绑定和生产激活仍然开放。

网关现在还具有类型化的消费者边界。 `CapabilityConsumerProfile`
区分默认通用 MCP 客户端和显式 A3S 消费者，
而`CapabilityConsumerNegotiation`绑定一个排序的、摘要绑定的扩展集
发送到网关并拒绝不支持的请求，而不是静默降级
他们。默认情况下，现有构造函数仍然是通用 MCP。配置文件标签
仅是元数据。描述符可以声明规范的`requiredExtensions`，并且
网关删除了协商消费者之前不接受的要求
编译发现或调用路由。标准适配器发布
目录授权、模式验证的 MCP 工具以及有限的不透明 URI 资源
并声明提示；每个发现列表都是确定性的和光标-
分页。发现游标是不透明的并绑定 MCP 表面，经过协商
目录摘要和冻结的主要可见性视图，因此光标来自
替换的发布失败，并用陈旧光标信号关闭，而不是
默默地跳过或重复功能。主机可以注入一个
`CapabilityGatewayDiscoveryPolicy` 冻结
每个经过身份验证的上下文的主体范围内的工具/资源/提示可见性；
被拒绝的路由从发现和直接访问中消失，而提供商的
每次操作授权仍然是强制性的。现有构造函数保留
允许所有兼容性策略，因此生产多主体主机必须
明确选择加入。流程/知识/UI有效负载投影和生产主机
组成仍然分开的门。适配器还消耗每个请求的 rmcp取消：取消运行中的工具、资源或提示会导致
提供商未来及其短暂的准入/解析器租赁，带有类型
当协议仍然可以传递一个结果时，就会产生无秘密的取消结果。参见
[能力消费者配置文件](docs/capability-consumer-profiles.md)
合同及其限制。

代理可见的描述也可以跨越显式的加密信任
边界。 `a3s-use-core` 定义了规范的、域分隔的
`SignedCapabilityDescription`信封； `a3s-use-extension`验证Ed25519
具有强制密钥轮换的有界公钥信任存储的签名，
到期、撤销。网关现在公开签名描述组合
在获取控制快照之前验证每个信封的构造函数
租赁或提供商解析器。私人`VerifiedCapabilityDescription`
包装器保留精确的重播字节，并且必须在恢复后重新验证。的
信任存储源仍然由主机提供，并且该路径尚未连接到
官方注册表/TUF 源或生产控制生命周期。参见
[能力描述签名](docs/capability-description-signatures.md)。

网关还公开了一个共享的、有界的
`CapabilityGatewayNotificationHub`。客户端初始化后，主机
可以发布更新的不可变目录密钥并扇出标准 MCP
`tools/list_changed`、`resources/list_changed` 和 `prompts/list_changed`
同时通知。重复或较旧的发布密钥被合并，
封闭或背压的同行退休了。这是一个通知接缝，
不是可变目录：主机必须将新会话切换到替换目录
服务器并保留上一代租约直至耗尽。会话工厂
用新的发现策略快照替换也被视为视图
更改，因此初始化的客户端即使在
源发布密钥未更改。

需要面向代理的有效负载的重新启动安全所有权的主机可以使用
`CapabilityGatewayCatalogStore`。它验证安装绑定并
规范目录字节，在有界 SHA-256 内容下存储记录
寻址布局，使用无跟随文件检查加上确定性暂存和
硬链接发布，并公开精确的 `get`、`get_exact` 和有界
库存读取。该商店在设计上没有可变的“当前”指针：
控制/生命周期切换必须将返回的摘要绑定到其提交的
生成并保留相应的会话租约。非活动控制
组合现在符合这种交接要求：主机拥有，无副作用
投影仪仅获得承诺的能力权限；具体业主
根据启用的包化身验证每个投影描述符，并且
终端表面证据，持久发布目录和能力指数，
然后将两个身份作为一个类型化的应用程序返回。应用的录音
观察以原子方式将已发布的控制光标与目录一起推进
消化、生成和修订。实时入场重新打开这些确切的字节
在进行包生成租赁之前。现在严格描述符投影仪
还在显式包范围内使用主机验证的签名证明
签名者白名单，检查准确的目录表面依赖性，终端
所有者特定的收据证据、有效的补助金覆盖范围以及经过审查的工具/MCP
在导出不透明路由引用之前的工作负载形状。这是故意的纯子集投影。现在安装拥有的描述符快照存储
支持签名的 v2 准入路径：它验证每个规范的 Ed25519
出版前的信封，在其派生的旁边保留确切的信封
证明投影，并根据当前信任存储重新验证信封
并在重新启动投影时计时。传统的 v1 仅证明路径仍然是
显式兼容模式，并且无法降级已签名的 v2 记录。快照
文件通过其规范字节进行内容寻址（而不是通过可变字节）
key），以有限的无跟随分段/无破坏重放的方式发布，以及
每次重新启动读取时都会重新验证；丢失的快照是安全的重试，而
替换、篡改、过期或撤销被拒绝。协调状态
备份现在只接受精确的内容寻址目录和描述符 -
快照记录并验证其规范所有者字节；仍然重播
根据当前信任策略重新检查已签名的信封。这还是
资格码：与官方绑定的加密密钥源
注册表/TUF元数据、生产控制/运行时/收据接线，以及
干净目标恢复激活保留主机门。运行时工具发布计划
现在通过计划、绑定进行规范的输入/输出模式证明
收据和控制证据；验证工件准入和严格
描述符投影比较相同的描述符和模式摘要。
生产控制激活、生命周期选择的保留策略，以及退休协调仍然是分开的；所有者本机恢复和
保留协调员是资格界限，直到该机构被
组成现场主持人。保留现在记录了一个持久的配对所有者
取消链接之前的阶段日志，并且备份/可达性拒绝运行，直到
挂起的日志已恢复。

嵌入边界现在还包括 `CapabilityGatewaySessionFactory`：
持久发布后，主机可以替换不可变的网关代
订购，保留一个标准 MCP 通知中心，并保留旧的仍在使用中
在其确切租约上进行操作，而稍后在同一端点上请求
遵守新目录。其有界 `drain` 转换关闭新请求
入院，在截止日期内等待已入院的手术，以及
释放工厂的源租约，以便生命周期所有者可以输入
独占保留或恢复栅栏。它的 `from_published` 和 `replace_published` 路径
重新阅读确切的商店出版物并验证协商的消费者
在源变得可见之前进行投影和完整的源目录。他们的
替换使用条件源交换，因此并发本地切换
发布验证后不能被覆盖。非活动控制组合
还提供`reopen_published_capability_gateway`和
`replace_published_capability_gateway`：两者都从耐用品中获得租赁
控制权限，将其保留在不可变的网关服务器内，并拒绝
未出租的替代品。成功的控制绑定排水保留一次性
输入端点身份，因此精确的关闭重试在之后仍然是幂等的
源租约被分离，而直接耗尽或复制未租约
目录仍然被拒绝。有条件替换也拒绝覆盖
较新的本地切换与陈旧的同代构建。生产控制
激活、提供者组成、退休和保留协调仍然承担东道主的责任​​。会话标识源自完整的
在消费者协商之前发布不可变的源，因此进行过滤
可选描述符不会破坏控制租约绑定或生命周期
和解。在升级期间，协调会验证现有的
交换之前端点针对其自己的源身份的先前控制租约
在新获得的出版物租约中。

目录有效负载清理现在也是一个明确的计划/应用操作：
`CapabilityGatewayCatalogStore` 需要生命周期提供的受保护摘要
设置，在其突变锁下重新验证规范库存，并删除
仅经过持久性检查的审查补充。破坏性业主申请
现在需要安装专属维护围栏，所以住
无法修剪控制支持的快照/网关租用。不活跃的
对照组合物添加 `plan_published_capability_payload_retention` 和
`apply_published_capability_payload_retention`：他们衍生出耐用的
已发布的目录（以及匹配的描述符快照（如果存在）），重新检查
将光标置于该专属围栏下，并拒绝将其删除的计划。
`drain_and_retain_published_capability_gateway` 组成端点排水
与关闭和退休路径的计划/应用顺序。
主机仍然添加独立管理的回滚或旧端点摘要
明确地；存储永远不会从内存中的指针猜测活跃度。

同一所有者现在通过以下方式公开计划绑定的干净目标恢复原语
`plan_clean_restore` 和 `apply_clean_restore`。来电者确认
规范的计划摘要并提供准确的目录集；适配器阶段
并验证完整的所有者目录，记录持久的激活标记，
并通过无破坏目录移动来发布它。现有所有者状态是
从未合并或替换，外国分阶段计划被拒绝，并且可以重试
重播持久候选者。这是一个业主原生的构建块：控制
注册、签名描述符恢复、会话消耗和生产
回滚编排仍然属于生命周期主机。

控制描述符快照所有者现在提供相应的
干净目标适配器。它的计划将每个快照摘要绑定到密钥摘要，
控制生成、规范字节计数和签名/仅证明模式；申请
重新检查确切的集合，对于签名的 v2 记录，需要当前的
`CapabilityDescriptionTrustStore` 和登台前的时钟。候选人和
激活证据是可重播的并且发布不会被破坏。的
`ControlCapabilityPayloadRestoreCoordinator` 在一个
单个专属围栏并按固定顺序重放；过程停止
所有者出版物之间的冲突可以通过重播相同的计划来恢复。这是
有序的、可恢复的激活而不是跨目录原子重命名。
`ControlCapabilityPayloadRetentionCoordinator`组成对应的
保留计划在一个专属围栏下，之前验证两个库存
第一个取消链接，并恢复目录→描述符中的确切所有者日志
订单。这是可恢复的有序删除而不是跨目录
原子事务。不活动的控制成分现在提供
重新启动安全游标重新打开边界和游标绑定保留计划/应用
入口点；生产所有者注册、实时网关会话替换
从该租约，生命周期调用排出和保留边界，以及
回滚权限保留在这些存储之外。

控制描述符快照通过以下方式公开相同的所有者级别合约
`plan_retention`、`apply_retention` 和 `recover_retention`。该计划嵌入
完整的受保护/删除分区，每个取消链接都在一个
有界规范期刊、待定期刊阻止发布和读取，以及
非空库存必须至少保留一个快照。生产控制
仍然提供注册、生命周期激活和信任源选择
选择并重新开放描述符生成的权威。配对的
`ControlCapabilityPayloadRetentionCoordinator` 添加跨所有者边界：
之前，这两种库存都在一个专属维护围栏下进行了预检
删除目录记录，然后以固定的方式删除描述符快照
订单；所有者之间的中断可以通过重播相同的计划来恢复。

|面积 |状态 |
| ---| ---|
|六面ACL封装合同|实施并支持固定装置|
| MHS 研究预览适配器配置文件 | A3S 使用边界、最小权限上限、精确托管 MCP 发布门、依赖图和无隐式写入重试规则均已记录并经过合同测试。这不是 MHS 实施或协议一致性声明 |
|已签名的目录 v3、TUF 验证、持久的可替换注册表源以及选择加入公共端点 SSRF 策略 |在引擎和独立 CLI 中实现；托管主机必须为不受信任的租户端点选择严格的策略|共享插件管理器服务、CLI、TUI 和管理器 MCP |类型化应用服务通过一个主机管理器实现搜索、检查、稳定安装列表、状态、安装/升级/卸载以及启用/禁用计划、持久计划重新打开、仅摘要应用、精确操作观察/监视以及可信预准入取消。其标准 MCP 适配器公开了 13 个工具 v5 库存，并需要注入可信确认证据以进行突变或取消。独立注册表支持的兼容性突变使用该服务不会破坏现有的 JSON 字段，而一对一的 `plugin` CLI 公开所有 13 个操作、精确类型化的结果、显式摘要绑定 `--yes` 应用/取消、持久重播和零网络缓存应用。 A3S 代码 CLI、TUI `/packages` 和产品主机管理器 MCP 组成了相同的服务。人类 CLI/TUI 演示现在可以从不可变的信封中导出精确的计划、图表、来源、权限、操作状态和确认边界，而无需更改机器 JSON；产品主机E2E保持开放||能力网关合约和嵌入 MCP 适配器 |已实现并经过合约测试：不可变的无路径描述符/目录合约、不透明调用/工件/端点/资源引用、精确的快照租赁和发布/生命周期生成绑定、具有规范摘要和无静默降级语义的类型化通用 MCP/A3S 消费者配置文件协商、路由编译前协商的 `requiredExtensions` 目录投影以及标准 MCP `CapabilityGatewayMcpServer` 仅通过注入的 `CapabilityGatewayInvocationProvider` 路由目录授权的工具、资源和提示。工具、资源和提示发现是确定性的、有界的和光标分页的；资源读取需要精确的不透明 URI；针对经过审查的声明的即时论证已结束；提供者输出是有界的、无路径的和目录链接的。主机可以注入一个有界的`CapabilityGatewayDiscoveryPolicy`，它冻结列表和直接访问方法中主体范围的可见性，同时保留提供者授权作为单独的门。主机可以在 `/mcp` 上公开 Streamable HTTP，具有承载身份验证、可选的精确源策略、重复标头拒绝、有界的飞行/滚动窗口准入、净化的 HTTP 错误、显式预操作授权挂钩以及类型化的主机身份验证的传输/主体上下文。 `CapabilityGatewayInvocationResolver` 和 `CapabilityGatewayResolvedProvider` 为不透明引用提供单分辨率、租用范围的主机路径；返回的句柄必须在每次操作中保留准确的包生成租约。 `CapabilityGatewayMcpServer::from_verified_registry_snapshot_with_factory_and_options` 组成经验证的目录、同游标解析器、快照租用、协商和准入策略作为一种故障关闭的构造边界。 `CapabilityGatewayNotificationHub`将不可变的发布更改桥接到标准MCP列表更改通知，并且`CapabilityGatewayCatalogStore`提供有界的、规范的、内容寻址的、重新启动安全的有效负载所有权，具有精确的读取、无可变的当前指针、计划绑定的保留以及具有持久激活重放的严格的干净目标恢复适配器。控制描述符快照所有者现在提供带有签名 v2 信任重新验证的匹配计划绑定恢复。非活动控制内核现在自动将该有效负载身份绑定到其应用的功能切换和确切发布的光标；它的组合可以重新打开该游标和种子或替换实时网关会话，同时保留每个服务器克隆中的控制租约。协调的备份清单在一个显式`CapabilityPayloads`系列下验证和归档目录/描述符快照记录。独立的 Rust 客户端发现/调用检查涵盖了无路径边界。生产控制激活、所有者注册、实时生命周期连接、租赁消耗/保留协调、完整收据/运行时/资助支持的描述符投影、CLI 连接、TLS 终止以及 TypeScript/Python 客户端/恢复矩阵保持开放 ||注册表目标观察、显式离线安装/升级、有限源工作集、可恢复下载、使用和确认源清理 |实施中断、范围、篡改和零网络测试； cleanup 从未声明全局 blob 回收 |
|全球原始 blob 和扩展包 Artifact Store |原始验证目标和扩展树由 SHA-256 在一个全局根下进行分片，在跨进程摘要锁下提交，检查链接/重新解析，在注册表源和安装之间共享，在源修剪和范围卸载之间保留，并从安装备份中排除。存储绑定的共享/独占参考边界可防止维护或整个安装恢复与持久参考出版物竞争。物理、注册表引用、全局引用和连接可达性 v1 证据涵盖规范内容、分期、每个持久所有者、期望不匹配和检查的存储使用情况。可选的规范硬配额、完整摘要审核、精确计划逻辑隔离和经过验证的零参考补液仍然是独立的权限。确认 GC 现在仅接受有界显式 Blob/扩展包摘要允许列表，重复完整的零引用证明，将物理和生命周期证据以及前驱完成绑定到一个规范计划中，并在同分片原子退役和有界墓碑删除之前保留全局故障关闭栅栏。终端重播是只读的，无法删除稍后重新创建的目的。源修剪、范围卸载、审计、隔离、补水、配额压力和不可达性从不独立授权全局删除 |
|已签名的本机 Tool/stdio MCP 规划和下载后清单绑定 |实施和合同测试 |
|有界 SemVer 依赖解析和精确锁 |已实施 |
|安装、升级、卸载图排序|已实施 |
|持久的原子注册表切换和精确重播 |已实施 |
|包主机副作用/收据歧义恢复 |每个规范的安装、升级、启用、禁用和卸载检查点都通过子进程退出、精确密钥恢复、单一效果和终端重放测试。真正的 CLI 多节点安装还通过了持久发布前日志终止、零网络精确重放和无生成膨胀检查；卸载通过等效的隐藏、重新启动、接受呼叫耗尽和删除边界。产品主机和平台检查点保持开放 ||赠款图切换效应/收据模糊度恢复 |安装、升级和卸载原子发布/隐藏边界通过子进程退出、精确密钥恢复、单效、已完成日志和无重复测试。外部终止的托管范围管理器进程证明，这三个五节点图切换可以在无需重新授权、网络访问或生成膨胀的情况下恢复，同时保留候选格兰特并仅撤回确切的先前格兰特。真实主机协议进程还证明禁用隐藏/耗尽/精确撤销并启用发布/精确重新授予恢复，涵盖所有五个经过审查的突变。实际代码/运行时产品托管和跨平台资格保持开放 |
| Grant Store 日志/收据崩溃恢复 |规范的两个候选/两个退休生命周期中的所有 14 个持久检查点都通过了子流程退出收敛以及跨准备、切换/退休和切换前回滚的精确终端重放；真正的 CLI 和跨平台产品资格保持开放 || Windows 原子状态发布争用 |注册表源/可信根/目录/目标缓存、扩展收据/快照、工作空间授予、包图、主机计划/结果、生命周期、运行时绑定/配置、流程、知识绑定/恢复/备份、启用、全状态备份、恢复证据和诊断历史记录发布现在共享用于替换、无破坏和事务目录移动语义的有界阻塞原语。 Windows 仅重试瞬时访问、共享和锁定违规，最多两秒；已释放的文件或目录锁以原子方式收敛，持久替换锁保留先前的目标，失败的恢复移动保留其重播源。本机生命周期测试还将活动工件暂存重命名和选定的升级收据替换争用绑定到发布前回滚和重播，而卸载从不等待全局工件字节的读取器。外部竞争目标、重新启动恢复和外部产品主机争用仍然存在 ||免密运行诊断|实施的。最新/以前的包检查点通过`extension inspect --json`公开； `extension diagnose --json` 项目一个精确保留的计划/接纳/取消的安装/升级/卸载图表、主动接纳的启用/禁用操作或最新的主机审查的预接纳启用/禁用计划/取消，以及注册表/TUF、提供商、授予、切换、发布、耗尽、回滚和恢复证据。 `extension diagnose --history --json` 保留最新的 16 个已完成或回滚的操作，并取消每个范围/包 8 MiB 内的图形计划，卸载后仍可正常运行，删除重复数据，并且在损坏或链接状态下无法关闭。预锁定注册表/TUF 尝试公开刷新/缓存的每个注册表验证进度、信任/源摘要、角色版本、有限故障和终端锁定证据。保留图和预计划尝试公开了零网络预期/保留存档和可执行计划目标字节以及来自历史来源的精确目标`missing`/`partial`/`complete`状态。真实的终止进程和主机进程测试涵盖每次切换、部分观察、精确恢复、零副作用计划/取消启用诊断以及完成使用结果抑制。无路径活动/历史/容量恢复证据通过`knowledge restore-status --json`公开
|观察者安全的有界注册表突变锁定 |已实施并经过实际流程测试 |
| Plan-v4 审查了启用/禁用和终端 `NoChange` |在管理器合约和包引擎中实现 ||类型化托管主机管理器 | `CognitivePackageHostManager` 实现主机协议 v6，具有显式用户/工作空间范围类型绑定、精确功能/栅栏验证、持久计划/应用重放、选定表面证据、持久操作观察/监视、预准入取消、注册表出处重新验证、零网络安装/升级从精确计划缓存应用、图形和启用委派以及从用户拥有的准入/完成证据进行故障关闭的过期计划恢复。操作存储通过精确的计划摘要来区分重复的生命周期操作 ID，同时保留旧的查找别名，并且仅当其使用拥有的图、生命周期完成和包状态仍然匹配时才重播最终结果。不同范围类型中的相同文本ID保留不同的主机计划、安装快照、功能游标、调用租约和重放记录；完整的两次安装生命周期矩阵拒绝替换并在升级和卸载期间保留相反的安装。杀死真实的主机协议安装、升级、卸载、禁用和启用应用程序，并在注册表离线时恢复，收敛精确的拨款而不产生膨胀，并保留一个最终结果；对每个外部托管主机的注入保持开放
|工作空间授予组合和撤销前耗尽 |在核心/独立生命周期路径中实施 ||混合原生/托管提供商规划 |在使用和共享 A3S 主机路径中实现：未绑定草稿、指定提供者预检、主机策略、规范的受资助限制的最终选择、持久规划捆绑包/资助快照/提供者生成、精确的应用时间重建、重新启动重播和提供者漂移拒绝均经过测试 |
|确切的出版一代知识租赁|在使用注册表和 SQLite 知识主机中实现。获取将完整的能力投影与已安装的包、清单、OKF包、生命周期生成、生成锁绑定；一项租约在引用的搜索/读取中保留这一代，在隐藏后拒绝新的调用，参与耗尽，并且因包或保留内容漂移而无法关闭。 A3S 代码消耗仍然是一项外部集成任务 |
|独立任务、stdio MCP、显式 A3S 流程预检、技能/UI 和 SQLite/FTS5 OKF 主机 |已实施 ||托管运行时收据生命周期|独立的版本支持任务模板支持重新启动安全的精确生成调度、接收拥有的提供者重新连接、陈旧生成拒绝和接受呼叫消耗。功能快照 v5 仅发布具有稳定主机工具标识的精确安装/包/代匹配的任务绑定。现在，服务准备会在运行时应用之前同步 v1 配置收据，通过精确的运行时和网关证据推进它，并在删除挂起的恢复权限之前提交 v3 绑定。工具和 HTTP MCP 绑定失败、预应用回滚、候选清理以及最终绑定/挂起接收崩溃窗口重播，而不会产生第二个运行时效果或残留。测试二进制子进程矩阵存在于工具和 HTTP MCP 的所有六个嵌套配置窗口中，然后证明精确重播、终端幂等性和无残留网关/运行时删除。类型化端点、停止前耗尽、运行时删除前路由删除、确切的上一代停用和停止绑定重新授权均经过合同测试。 A3S CLI `main` 提交 `563e7e139740e845369f9102a2d47026733797a8` 通过生产 Box 映射、保留的 N/N+1 路由、标准 MCP 初始化、网关和生命周期主机重启、耗尽、精确删除和零残留检查来验证四个真实的 Linux 工具和 MCP 进程。已确认的同代提供商丢失现在仅在准确的运行时重新应用和发布新的网关路由和旧的绑定收据之前退役分配的网关端点；中断的路由删除保留重放权限，而无需停止或删除运行系统单元。作用域 Code Exec 任务发现和租用调用在 A3S CLI `main` 提交 `e77d318beba3cba7f193da8d83bb9ac5c46fc0f7` 和 CI 运行 [32797862154](https://github.com/A3S-Lab/CLI/actions/runs/32797862154) 上合格。真正的提供商进程终止资格、非 Linux 提供商和跨平台产品主机恢复仍然开放 |
|范围有限的 OKF 配额、保留、逻辑删除 GC、SQLite 压缩和使用诊断 |在独立的知识后端中实施 |
|范围本地 OKF 完整性审计、验证数据库备份和轮换、派生 FTS 修复以及权限绑定数据库/绑定恢复 |已验证的备份现在使用精确范围、有界的最旧优先保留以及规范的计划摘要确认、最后备份保留、目录锁定、陈旧计划拒绝和失败关闭候选验证。恢复经过真实进程测试，包括丢失数据库和丢失精确子集绑定恢复、冲突拒绝、主/WAL/SHM保留、绑定文件和文件系统/日志进程退出窗口、持久维护阻止、每个窗口的无路径恢复状态诊断以及终端只读重播。缺少注册表/程序包/生命周期/授予权限、清理机器、协调跨系列和整个产品恢复保持开放状态 ||协调整个安装的备份、保留和审查恢复|备份和保留是在专有维护围栏下实现的，具有确定性的无路径清单、精确的注册表/接收机构摘要、列入允许的控制状态系列、显式的全局工件存储排除、扫描/复制/重新扫描一致性、完整的有效负载验证、精确的计划保留和两代保存。能力网关目录和描述符快照记录现在进入严格的内容寻址`CapabilityPayloads`系列；锁、暂存和保留日志无法作为非最终证据关闭，并且 Artifact Reachability 会遍历同一所有者树，而不是默默地忽略嵌套漂移。相同版本/操作系统/架构恢复现在需要精确独立保留的注册表、工件和授予权限、显式验证的回滚存档、无路径摘要确认、链接/重分析安全候选暂存、七个持久日志阶段、15 个子进程退出恢复边界、终端重播、只读状态和有界崩溃可恢复历史记录。生产所有者本地干净目标激活/保留、缺失权限和干净机器恢复以及跨平台操作灾难恢复演习仍然开放|
|每个声明的主机中的运行时服务、HTTP MCP、托管知识恢复/回滚以及沙盒 UI 组合进行中 || A3S 代码 CLI/TUI 集成 |审查了运行时任务安装、离线重新启动禁用/重新启用、应用时构建偏差拒绝、观察程序热插拔、具有单一效果和无路径历史记录的跨终止进程离线恢复的主机状态修订恢复、具有冻结任务目录证据的作用域 Code Exec 代理发现/调用、上下文审查和 TUI `/packages` 审查。共享主机管理器现在还符合已签名的六面工具/MCP/流程/技能/UI/OKF 安装、调用证据、精确生成升级、卸载、重播和用户/工作空间范围围栏的资格；六面码产品主机E2E及发布资格保持开放|已验证预览安装程序和发布证据 | Linux/macOS 和 Windows 安装程序强制执行 HTTPS、精确标签身份 Sigstore 验证、发布校验和、安全提取、打包 OCR/技能绑定、版本化原子激活、完整树重新安装验证、保留本地证据和托管命令所有权。实现了确定性存档序列化、每个平台的 SPDX SBOM、GitHub OIDC 出处/SBOM 证明以及固定操作/工具。资格运行 [33651777660](https://github.com/A3S-Lab/Use/actions/runs/33651777660) 在所有五个目标上通过了隔离存档执行和无缓存逐字节重建，来自精确的 `main` 提交 `4f6e4725205d06ab81f8ea98bfee85c7eb4b2bcd`；陈旧核心`v0.3.5`发布尝试未创建任何版本。发布工作流程 [33687297386](https://github.com/A3S-Lab/Use/actions/runs/33687297386) 在准确的 `main` 提交 `48a0b76f8a4a87a11d16627c7bd7567920852508` 处通过了标签 `v0.3.7` 的所有 13 个作业，并发布了经过验证的档案，键入了 crates（`a3s-use-core 0.2.6`、`a3s-use-extension 0.3.7`、 `a3s-use 0.3.7`)、SBOM、证明和安装程序。发布工作流程 [33720485826](https://github.com/A3S-Lab/Use/actions/runs/33720485826) 在准确的 `main` 提交 `6d3a7baf32ce998a2e487c40fbf78b4a6cda2579` 处通过了标签 `v0.3.8` 的所有 13 个作业，并发布了经过验证的档案，键入了 crates（`a3s-use-core 0.2.7`、`a3s-use-extension 0.3.8`、 `a3s-use 0.3.8`)、SBOM、证明和安装程序。发布工作流程 [33756618837](https://github.com/A3S-Lab/Use/actions/runs/33756618837) 在准确的 `main` 提交 `a5f3cc40bfb0a1021ca150d2ce4295409b74d220` 处通过了标签 `v0.3.9` 的所有 13 个作业，并发布了 19 个经过验证的发布资产，键入了板条箱（`a3s-use-core 0.2.7`、`a3s-use-extension 0.3.9`、 `a3s-use 0.3.9`)、SBOM、证明和安装程序。发布工作流程 [33791616307](https://github.com/A3S-Lab/Use/actions/runs/33791616307) 在确切的 `main` 提交 `c4c80a223bfff3698ca4b4598e7175c6e3303239` 处通过了标签 `v0.3.10` 的所有 13 个作业，并发布了 19 个作业已验证的发布资产、类型化包（`a3s-use-core 0.2.8`、`a3s-use-extension 0.3.10`、`a3s-use 0.3.10`）、SBOM、证明和安装程序。之前的 `v0.3.6`、`v0.3.7`、`v0.3.8` 和 `v0.3.9` 版本仍然是历史证据；外部操作的完整档案证人和非发布证据保留仍然开放
|完整的Linux/macOS/Windows实时流程E2E和恢复矩阵 |释放拦截器 |
|公共注册操作、外部完整存档再现性见证、非发布证据保留、支持运行手册 |释放拦截器 |

**生产就绪：否。** 该代码具有经过大量测试的基础，但是
上面未完成的行仍然需要释放门。 [路线图.md](ROADMAP.md)
跟踪剩余的产品工作，无需将已完成的内部结构转换为
释放索赔。

## 平台支持

|目标|当前门|产品状态 |
| ---| ---| ---|
| Linux x86_64/arm64 |完整的 A3S 使用工作区 CI 以及发布容器一致性 |发展预览 |
| macOS arm64 / x86_64 |当前 A3S 使用工作区构建和测试 |发展预览 |
| Windows x86_64 |当前 A3S 使用工作区测试、跨注册表/缓存的本机链接状态限定、包图/诊断、生命周期/运行时/流程、备份/恢复和 OKF 路径、扫描器锁定 blob 发布/源清理/包提交/升级接收/生命周期删除恢复、签名注册表/图/授予/流程/OKF CLI 生命周期以及终止进程切换重播 |预览;完整的运行时间/恢复矩阵待定|

本机 CI 运行
[32604181662](https://github.com/A3S-Lab/Use/actions/runs/32604181662)
通过当前使用拥有的工作区和实时流程集成套件
来自确切 `main` 提交的所有五个目标
`40bc5593cbf58ca2da171d85ba578c2d6bd911c8`。这建立了当前
仅使用自有平台基线；产品主机、重新启动、更广泛的防病毒
争用，其余的恢复场景仍然是释放阻碍。

受信任的包和状态路径使用平台感知的元数据检查来拒绝
Unix 符号链接和 Windows 遍历之前的重分析点。平台测试
覆盖范围与生产资质不同。

## 存储库布局

`a3s-use-science` 故意不属于此存储库、工作区，
运行时、CI 或发布。特定领域的科学代码保持独立
拥有并且以后只能通过相同的签名包来使用
注册合同作为任何第三方功能。测试包命名为
`a3s/science` 是合成注册表固定装置，不链接科学箱。

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

## 发展

从此存储库运行检查，而不是从 A3S monorepo 根运行检查：

```bash
cargo fmt --all -- --check
cargo test --workspace --all-targets
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo check -p a3s-use --no-default-features
cargo check -p a3s-use --no-default-features --features extensions
```

构建并验证文档站点：

```bash
cd website
npm ci
npm run format:check
npm run lint
npm run build
npm run check:site
```

贡献规则记录在[AGENTS.md](AGENTS.md)中。公共 Rust 类型
应保持键入状态并在适用的情况下显示`Send + Sync`； I/O使用Tokio；访问控制列表是
默认的人工编写的配置格式。

## 文档

- [产品路线图](ROADMAP.md)
- [插件合约参考](docs/plugin-contracts.md)
- [插件平台架构](docs/plugin-platform-architecture.md)
- [生命周期和安全性](docs/plugin-platform-lifecycle-and-security.md)
- [型号硬件标准集成配置文件](docs/mhs-integration.md)
- [发展计划](docs/plugin-platform-development-plan.md)
- [已验证发布安装](docs/release-installation.md)
- [释放描述符](docs/release-descriptors.md)
- 【代理包管理器第一原则审核](docs/agent-package-manager-audit.md)
- 【OKF知识操作](docs/okf-knowledge-operations.md)
- [注册表缓存操作](docs/registry-cache-operations.md)
- [文档网站](https://a3s-lab.github.io/Use/)

## 许可证

阿帕奇-2.0。请参阅 [许可证](LICENSE) 和
[THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md)。