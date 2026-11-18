# AgentCue

跨平台（**macOS + Windows**）轻量级 AI Coding Agent 悬浮进度胶囊与完成弹窗提醒工具，基于 **Tauri v2 + Rust + React + Tailwind CSS** 构建，统一监控 **Claude Code**、**Codex** 与 **Google Antigravity**。

## 核心特性

- **常驻无焦点悬浮窗（双模式可切换）**：
  - **自由悬浮胶囊**（默认，支持自由拖拽记忆位置）与 **顶部居中灵动岛模式**。
  - macOS 使用非激活的 `NSPanel`，并在桌面联调中验证了展开面板时的输入焦点。Windows 使用 `WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW`。[agent-hud-open 的 tao 补丁](https://github.com/jazzenchen/tao/tree/e5c8659a4b91a44e806bafcd63ece1f11fcba503)
- **双通道状态采集（开箱即用 + 毫秒级增强）**：
  - **被动只读监听**：启动即自动只读扫描 Antigravity `conversation_summaries.db` / `transcript.jsonl`、Codex `rollout-*.jsonl` 与 Claude Code `projects/**/*.jsonl`，零配置展示当前任务与工具步骤。
  - **跨平台 Hook 伴侣（`agent-ing-hook`）**：通过本地 `127.0.0.1` 转发，接入时备份客户端配置并保留已有第三方钩子。
- **独立角落完成 / 阻塞弹窗 (Toast)**：
  - 任务完成、报错中断或**卡在权限审批（`PermissionRequest`）**时弹出独立悬浮卡片并播放合成提示音，支持一键跳回对应终端或 IDE 窗口。

## 本地开发与构建

```bash
# 1. 安装前端依赖
pnpm install

# 2. 启动开发模式（确保 cargo 在 PATH 中）
PATH="$HOME/.cargo/bin:$PATH" pnpm tauri dev

# 3. 运行 Rust 单元测试（状态机、Hook 解析器、非破坏性配置合并）
PATH="$HOME/.cargo/bin:$PATH" cargo test --manifest-path src-tauri/Cargo.toml

# 4. macOS 通用包（Apple Silicon + Intel，本地临时签名）
rustup target add aarch64-apple-darwin x86_64-apple-darwin
APPLE_SIGNING_IDENTITY=- pnpm tauri build --target universal-apple-darwin --bundles app,dmg

# 5. Windows x64 的 NSIS 安装包（在 Windows 开发机上）
rustup target add x86_64-pc-windows-msvc
pnpm tauri build --target x86_64-pc-windows-msvc --bundles nsis
```
