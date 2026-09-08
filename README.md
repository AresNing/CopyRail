# CopyRail

A local-first clipboard workspace for macOS, built with Rust, Tauri and Leptos.

CopyRail 是一个本地优先的剪贴板工作空间，用来保存、搜索、整理和再次使用复制过的内容。界面采用黑白灰背景与独立设计的卡片／轨道品牌图形，保留内容分类和状态颜色。

**当前版本：0.1.0 MVP（源码预发布）**。开发与验证平台为 Apple Silicon macOS；尚未完成完整日用验收，不提供已签名、公证的安装包。共享、多设备同步及移动端不属于本次 MVP。

## 已实现的能力

- 文本、富文本、HTML、链接、图片、文件、PDF 与颜色内容的捕获和预览。
- SQLite 本地历史、全文搜索、来源／类型筛选、内容去重与历史保留策略。
- Pinboard 分类、新建与编辑内容、多选、排序、归类、删除与撤销。
- 普通／紧凑布局、浅色／深色外观、菜单栏入口、快捷键与原生拖动预览。
- 暂停采集、应用排除、机密／瞬态类型过滤、本地备份与恢复。
- 复制、纯文本输出和受权限／目标窗口检查约束的直接粘贴。
- 默认关闭的本地 stdio MCP 接口，支持按客户端授权。

上述为实现范围，完整跨应用粘贴、拖放、复杂富文本、多屏及 VoiceOver 仍有未验收项。请参阅 [验证记录](docs/verification.md) 和 [安全与隐私说明](SECURITY.md)。

跨桌面快捷键唤起已在本机 beta.9 通过用户复测：同一屏幕的另一个普通桌面可就地弹出、输入搜索，并再次按快捷键收起。修复采用不激活应用的原生浮动面板，并修正启动阶段覆盖应用模式的问题；全屏空间和多屏场景仍待验收。详见 [验证记录](docs/verification.md#post-mvp-fix-current-space-invocation-2026-09-08)。

## 从源码运行

需要 macOS、Xcode Command Line Tools、[Rustup](https://rustup.rs/) 和 Trunk。当前验证工具链为 Rust **1.98.0**、Trunk **0.21.14**；仓库固定 Rust 工具链并提交 Cargo.lock。首次构建会下载公开依赖。

```sh
git clone https://github.com/AresNing/CopyRail.git
cd CopyRail
rustup target add wasm32-unknown-unknown
cargo install trunk --version 0.21.14 --locked
cd apps/desktop
../../scripts/trunk.sh build --config Trunk.toml
cd ../..
cargo run -p pasters-desktop --no-default-features --locked
```

`--no-default-features` 用于排除实验性 CloudKit 适配层，请在 MVP 的普通运行和打包命令中保留。`scripts/dev.sh` 是包含默认功能的旧开发入口，不作为本次 MVP 的推荐启动方式。

正常启动会采集**后续新复制的真实内容**。隐藏面板仍会采集；使用菜单栏暂停或退出。首次体验请用无敏感测试内容。`⇧⌘V` 呼出面板，搜索并选择卡片后可按 `Space` 预览、`⌘C` 复制，再到目标应用手动粘贴。`Return` 直接粘贴另需辅助功能权限；不能把“已复制”视为“已粘贴”。

数据目录为 `~/Library/Application Support/io.pasters.desktop/`，数据库为 `history.db`。历史与备份未由应用加密；升级前退出应用并备份数据。恢复备份会替换现有数据。内部 crate、可执行文件与存储身份暂时保留 `paste` / `pasters` 命名，以维持已有历史与备份兼容。

## 检查与测试

```sh
./scripts/check.sh
node --test scripts/maintain-artifacts.test.mjs scripts/check-local-qa-preparation.test.mjs scripts/check-native-tab-trace.test.mjs
```

完整检查包含格式、Rust 单元／集成测试、严格 Clippy、WASM 检查和界面构建。脚本测试需要 Node.js 22 或更新版本。编译界面测试另外需要安装在标准路径的 Google Chrome：

```sh
node scripts/check-copyrail-design-ui.mjs
node scripts/check-paste-outcome-ui.mjs
```

这些浏览器测试使用合成数据和模拟 IPC，不等于 macOS 原生端到端验收。调试原生隔离模式可在完成界面构建后运行：

```sh
cargo run -p pasters-desktop --no-default-features --locked -- --native-ui-test
```

隔离模式仅限 debug 构建，使用临时合成数据库并拒绝真实采集、剪贴板写回、云端和账户操作。系统嵌入的 PDF 阅读器菜单仍需单独验证，详见 [PDF 样本说明](apps/desktop/src-tauri/fixtures/README.md)。

## 项目结构

- `apps/desktop`：Leptos / WASM 界面与样式。
- `apps/desktop/src-tauri`：Tauri 桌面壳层、原生窗口、预览、编辑与 MCP。
- `crates/paste-domain`、`paste-core`、`paste-storage`：领域、用例、本地数据库与搜索。
- `crates/paste-platform`：macOS 剪贴板、权限、窗口及系统能力。
- `crates/paste-sync`、`paste-cloudkit`：实验性同步模型和适配层，非本次发布承诺。
- `scripts`：构建、合成测试及本地产物维护工具。

进一步阅读：[架构](docs/architecture.md) · [验证与限制](docs/verification.md) · [依赖清单](docs/dependencies.md)。

## 许可证与独立性

Copyright 2026 CopyRail contributors.

项目自有代码、文档及自行绘制的品牌图形采用 **[Apache License 2.0](LICENSE)**，另有标注的内容除外。该许可证允许商用和遵守条款的闭源二次分发，并包含条款限定范围内的专利授权。适用署名见 [NOTICE](NOTICE)，第三方依赖和 PDF 样本字体继续遵守 [各自的许可](THIRD_PARTY_NOTICES.md)。以许可证英文全文为准。

CopyRail 的早期功能研究参考了 Paste 等剪贴板工具，现采用独立名称、品牌图形和界面设计。CopyRail 是独立项目，与 Paste Team ApS 无隶属、赞助或认可关系。项目许可证不授予第三方代码、素材或商标权利，也不构成全面法律审查的结论。
