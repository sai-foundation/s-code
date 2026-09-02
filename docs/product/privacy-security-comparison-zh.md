---
site: true
slug: privacy-security-comparison
title: 为什么 Opencoding 的隐私与安全边界更清楚
short_title: 隐私与安全对比
group: Why Opencoding
order: 85
description: 用真实攻击场景、公开源码和竞品官方文档，解释 Opencoding 在哪些隐私与安全设计上更强，以及边界在哪里。
keywords:
  - privacy
  - security
  - sandbox
  - opencode
  - codex
  - claude code
  - 隐私
  - 安全
---

# 为什么 Opencoding 的隐私与安全边界更清楚

## 先说结论

Opencoding 的优势不是一句“本地运行”，而是把**模型凭据、浏览器会话、工具权限、
操作系统隔离、文件并发保护和审计证据**分成可检查的边界。

这让它相对 OpenCode 有一个明确优势：Opencoding 的命令边界由操作系统执行，
而 OpenCode 的官方威胁模型明确说明其权限系统不是安全隔离。相对 Claude Code，
Opencoding 当前支持的平台会在沙箱不可用时失败，而不是默认降级为无沙箱执行。
相对 Codex，两个开源项目在“工作区写入 + 默认断网 + OS 级命令沙箱”这三个
公开默认设计点采用相近模式。Opencoding 可核验的产品差异应限定为自己的拓扑：
可选的独立模型凭据进程、Local Web 的密钥隔离、带文件版本前置检查的编辑，
以及贯穿工具、批准、用量和 diff 的统一事件证据。

> **对比口径：** 2026-08-31 的 Community 源码与各产品官方公开文档。
> “官方页面未声明同类契约”不等于竞品一定没有，只表示我们不把无法验证的推测写成事实。

## 五个真实场景

### 1. README 里的提示注入要求上传 SSH 密钥

攻击内容可能诱导 Agent 执行 `cat ~/.ssh/id_rsa`，再用 `curl` 上传。Opencoding
的结构化命令默认关闭网络，并把可读、可写目录交给 macOS Seatbelt 或 Linux
bubblewrap 强制执行。模型“想这样做”不会扩大操作系统授予的能力；需要网络时，
请求仍要经过策略和批准。

这不是绝对隔离：它不是虚拟机，也不能保护被用户明确放进允许范围的秘密。
但它把一次提示注入从“依赖模型自律”变成“必须同时穿过文件、网络和批准边界”。

### 2. 恶意依赖的安装脚本试图修改 shell 配置

项目内的依赖脚本可能尝试写入 `~/.zshrc`、启动项或工作区外的可执行目录。
Opencoding 的 `run_command` 只给命令树声明的写入根；超出根目录的写入由 OS
沙箱拒绝。默认断网还会阻止它临时下载第二阶段载荷，除非用户明确扩大网络能力。

### 3. 人和 Agent 同时修改同一个文件

`read_file` 返回内容摘要，`apply_patch` 必须携带对应版本。写入前会再次核对
SHA-256，并在发现已变化时失败。它显著缩小陈旧上下文覆盖新内容的窗口，但不是
文件系统事务或跨进程锁；摘要检查与原子替换之间仍有很小的并发窗口。对同一文件
运行外部编辑器或后台生成器时，仍应先停止并发写入再确认最终 diff。

### 4. 浏览器脚本试图窃取 Provider Key

无论使用 direct-provider 还是 independent-proxy 模式，Local Web 都不接收
Provider Key，也不接收 daemon bearer token。direct-provider 模式下 daemon 可访问
凭据值；只有 independent-proxy 模式才由 proxy 单独持有 Provider Key。只有能读取
私有 connection file 的 `opencoding web` 启动器才能签发一次性 bootstrap；裸访问
loopback 首页无法获得授权。页面先清除承载 bootstrap 的 URL fragment，再把它交换
为 `HttpOnly; SameSite=Strict` Cookie，令浏览器 JavaScript、Local Storage 和 URL
都拿不到长期凭据。页面同时使用来源检查、CSRF 信号、CSP、禁止 framing 和
`no-store` 等响应头缩小攻击面。

### 5. “它说测试过了”，但无法证明

工具请求、策略决定、批准、执行结果、模型用量和最终 diff 属于同一会话事件链。
因此安全审查不只看 Agent 的自然语言总结，而能回到实际命令、结果与批准记录。
证据不能消除风险，但能让异常行为可发现、可复盘、可归责。

## 不是每个 Tool Use 都塞进 OS 沙箱

“所有工具都启动一个沙箱进程”不是正确目标。不同工具需要不同的强制边界：

| 工具类型 | 当前强制边界 | 为什么这样设计 |
| --- | --- | --- |
| `run_command` 及子进程 | macOS Seatbelt / Linux bubblewrap；显式读写根；网络默认关闭 | 任意程序和依赖脚本需要 OS 级约束 |
| `read_file`、`list_files`、`apply_patch` | daemon 从稳定的工作区目录句柄逐层打开，拒绝符号链接换位、父目录逃逸和敏感路径；编辑另加 SHA-256 前置检查 | 原生文件操作不需要启动任意进程，边界可以更窄、更结构化 |
| 策略、批准和审计 | 所有操作型工具调用都经过策略判定并记录事件 | “是否允许”与“系统能否越界”是两层不同防线 |
| Git、MCP、本地扩展 | Git 是宿主进程，但忽略系统/用户配置、禁 hooks/fsmonitor/textconv，并在每次操作前拒绝可执行仓库配置；本地 MCP 与扩展仍是经确认的宿主进程 | 这不是“全部进入 OS 沙箱”；Team Grant 模式在 Preview 中直接禁用 actor-scoped MCP runtime |

换句话说：**每次操作都应被治理，但只有会启动任意代码的执行路径需要进程沙箱。**
Git 的隐式程序执行面已经失败关闭；后续仍要把 Git、本地 MCP 和扩展 hook 纳入更一致的 OS 隔离策略。

## 与竞品公开设计对比

| 设计点 | Opencoding Community | OpenCode | Codex | Claude Code |
| --- | --- | --- | --- | --- |
| 本地命令 OS 隔离 | 内置；macOS Seatbelt / Linux bubblewrap | 官方威胁模型明确：无沙箱，权限是提示与可见性 UX | 内置 OS 沙箱 | 内置 Bash 沙箱，但默认需启用 |
| 命令网络默认值 | 关闭；显式请求并经过策略/批准 | 权限规则可询问或拒绝，但没有 OS 沙箱出口边界 | `workspace-write` 默认关闭 | 沙箱默认未启用；启用后按域名治理，沙箱缺失时默认可回退到无沙箱执行 |
| 沙箱不可用时 | 失败，不降级执行 | 不适用：产品本身无沙箱 | 当前 sandbox mode 继续定义边界；可显式选择危险全权限 | 默认警告后无沙箱运行；可配置 `failIfUnavailable` 改为失败 |
| 陈旧文件写保护 | `read` 摘要 + `apply_patch` 版本前置条件 | 所引官方页未声明同类契约 | 所引官方页未声明同类契约 | 所引官方页未声明同类契约 |
| Local Web 长期密钥 | Provider Key 和 daemon bearer token 不进入浏览器 JS/Storage/URL | 产品拓扑不同；官方说明本地不存代码或上下文，但 server mode 需用户自行保护 | 产品拓扑不同，不能直接对应 | 产品拓扑不同，不能直接对应 |
| 原生 Windows 沙箱 | 暂不支持；命令执行失败关闭 | Windows 可运行，但官方仍声明 Agent 无沙箱 | 支持 WSL2 与原生 Windows 沙箱 | 支持 WSL2；不支持原生 Windows 沙箱 |

公平地说，OpenCode 同样强调本地运行且不存储代码或上下文；Codex 的默认断网、
工作区写边界和 OS 沙箱很强，并且 Windows 支持优于 Opencoding；Claude Code 在启用
沙箱并配置 fail-closed 后也能提供强文件与网络隔离。我们的结论不是“其他产品都不安全”，
而是 Opencoding 在上述特定设计点提供了更清楚、可审查且默认收紧的契约。

## 我们真正领先的原因

1. **能力先于批准。** 批准是人的决定，沙箱是系统的上限；二者不能互相替代。
2. **默认最小能力。** 命令默认断网、写入根显式；超时回收命令进程组并等待直接子进程。
3. **密钥与界面分层。** 模型凭据、执行服务和浏览器会话属于不同进程与认证边界。
4. **编辑带乐观并发检查。** Agent 必须提交刚刚读到的版本摘要；它不是跨进程事务锁。
5. **安全结论带证据。** 测试、diff、策略、批准、用量与审计事件能一起复核。
6. **公开承认未覆盖范围。** 可信的安全设计必须告诉用户哪里仍需要容器、VM 或人工审查。

## 仍然要坦白的边界

- macOS 和 Linux 是当前受支持的平台；Windows 还没有原生执行沙箱，系统会失败关闭。
- 命令沙箱不是 VM 或 microVM，不能对抗操作系统内核漏洞，也不能撤销用户主动授予的宽权限。
- 超时会终止命令进程组，但主动创建全新脱离会话的宿主进程可能逃逸；对敌意代码应再使用外层容器或 VM。
- Git、本地 MCP 和扩展 hook 尚未全部进入统一的 OS 级隔离路径；Git 已关闭已知配置执行面，Team Grant 模式则暂不加载 MCP runtime。
- 文件读取与编辑使用 daemon 内的能力边界，而不是为每次操作启动一个 OS 沙箱。
- 发送给模型的 prompt、代码片段和工具结果仍受所配置模型 Provider 的数据政策约束；
  “Provider Key 不进浏览器”不等于“数据不发给 Provider”。
- 任何 Agent 都可能生成有漏洞的代码。沙箱保护执行环境，不替代代码审查和真实测试。

## 如何核验这些说法

- [Opencoding Community：平台沙箱实现](https://github.com/shilongliu-iteria/opencoding-community/blob/main/crates/platform-runtime/src/lib.rs)
- [Opencoding Community：工具执行与版本前置条件](https://github.com/shilongliu-iteria/opencoding-community/blob/main/crates/execution/src/lib.rs)
- [Opencoding Community：Local Web bootstrap 与安全响应头](https://github.com/shilongliu-iteria/opencoding-community/blob/main/crates/daemon/src/lib.rs)
- [OpenCode 官方威胁模型：No Sandbox](https://github.com/anomalyco/opencode/security)
- [OpenCode 官方权限规则](https://opencode.ai/v2/docs/permissions)
- [OpenCode 官方隐私说明](https://opencode.ai/)
- [Codex 官方：Agent approvals & security](https://developers.openai.com/codex/agent-approvals-security)
- [Claude Code 官方：Sandboxing](https://code.claude.com/docs/en/sandboxing)
- [Claude Code 官方：Settings 与 sandbox 默认值](https://code.claude.com/docs/en/configuration)

这份页面只比较公开、可定位的设计与默认值。产品升级后应重新核对官方文档，
并用可重复的安全测试验证实现，而不是把这张表当成永久不变的排名。
