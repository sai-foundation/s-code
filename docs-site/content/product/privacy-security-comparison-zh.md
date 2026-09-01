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
相对 Codex，两者在“工作区写入 + 默认断网 + OS 级命令沙箱”上属于同一安全等级；
Opencoding 更突出的差异是可自行审查的 Community 执行平面、独立的模型凭据进程、
Local Web 的密钥隔离，以及带文件版本前置条件的编辑和统一证据链。

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

`read_file` 返回内容摘要，`apply_patch` 必须携带对应的短版本。若人或另一个
Agent 已经更新文件，旧版本写入会失败并要求重新读取，而不是静默覆盖新内容。
这个 SHA-256 前置条件既是可靠性机制，也是防止陈旧上下文破坏代码的安全边界。

### 4. 浏览器脚本试图窃取 Provider Key

Provider Key 只属于独立模型 API 进程。Local Web 不接收 Provider Key，也不接收
daemon bearer token。一次性 bootstrap 交换为 `HttpOnly; SameSite=Strict` Cookie，
令浏览器 JavaScript、Local Storage 和 URL 都拿不到长期凭据。页面同时使用来源检查、
CSRF 信号、CSP、禁止 framing 和 `no-store` 等响应头缩小攻击面。

### 5. “它说测试过了”，但无法证明

工具请求、策略决定、批准、执行结果、模型用量和最终 diff 属于同一会话事件链。
因此安全审查不只看 Agent 的自然语言总结，而能回到实际命令、结果与批准记录。
证据不能消除风险，但能让异常行为可发现、可复盘、可归责。

## 不是每个 Tool Use 都塞进 OS 沙箱

“所有工具都启动一个沙箱进程”不是正确目标。不同工具需要不同的强制边界：

| 工具类型 | 当前强制边界 | 为什么这样设计 |
| --- | --- | --- |
| `run_command` 及子进程 | macOS Seatbelt / Linux bubblewrap；显式读写根；网络默认关闭 | 任意程序和依赖脚本需要 OS 级约束 |
| `read_file`、`list_files`、`apply_patch` | daemon 内的工作区能力检查、路径规范化、敏感路径规则；编辑另加 SHA-256 前置条件 | 原生文件操作不需要启动任意进程，边界可以更窄、更结构化 |
| 策略、批准和审计 | 所有操作型工具调用都经过策略判定并记录事件 | “是否允许”与“系统能否越界”是两层不同防线 |
| Git、MCP、本地扩展 | 目前并非全部经过同一个 OS 命令沙箱 | 这是已知边界，不应宣称已经完成全工具隔离 |

换句话说：**每次操作都应被治理，但只有会启动任意代码的执行路径需要进程沙箱。**
我们还需要继续把 Git 子进程、本地 MCP 和扩展 hook 纳入更一致的隔离策略。

## 与竞品公开设计对比

| 设计点 | Opencoding Community | OpenCode | Codex | Claude Code |
| --- | --- | --- | --- | --- |
| 本地命令 OS 隔离 | 内置；macOS Seatbelt / Linux bubblewrap | 官方威胁模型明确：无沙箱，权限是提示与可见性 UX | 内置 OS 沙箱 | 内置 Bash 沙箱，但默认需启用 |
| 命令网络默认值 | 关闭；显式请求并经过策略/批准 | 权限规则可询问或拒绝，但没有 OS 沙箱出口边界 | `workspace-write` 默认关闭 | 普通网络请求默认批准；启用沙箱后可做域名边界 |
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
2. **默认最小能力。** 命令默认断网、写入根显式、超时回收整个子进程树。
3. **密钥与界面分层。** 模型凭据、执行服务和浏览器会话属于不同进程与认证边界。
4. **编辑是带前置条件的事务。** Agent 必须证明自己编辑的是刚刚读到的版本。
5. **安全结论带证据。** 测试、diff、策略、批准、用量与审计事件能一起复核。
6. **公开承认未覆盖范围。** 可信的安全设计必须告诉用户哪里仍需要容器、VM 或人工审查。

## 仍然要坦白的边界

- macOS 和 Linux 是当前受支持的平台；Windows 还没有原生执行沙箱，系统会失败关闭。
- 命令沙箱不是 VM 或 microVM，不能对抗操作系统内核漏洞，也不能撤销用户主动授予的宽权限。
- Git 子进程、本地 MCP 和扩展 hook 尚未全部进入统一的 OS 级隔离路径。
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
