# Install CopyRail 1.0.0

[Download v1.0.0](https://github.com/AresNing/CopyRail/releases/tag/v1.0.0)

## English

**Requirements:** Apple Silicon Mac (M1 or later), macOS 13 or later. Intel, Windows and Linux installers are not included. Native acceptance was performed on macOS 26.5.1; older supported deployment targets have not been individually tested.

1. Download `CopyRail_1.0.0_aarch64.dmg` from this repository's release page.
2. Quit any running CopyRail before updating. Open the disk image and drag **CopyRail.app** into **Applications**. Eject the disk image, then open CopyRail from Applications.
3. The app uses local ad-hoc signing and **has not been Developer ID-signed or notarized by Apple**. If macOS blocks it as an unidentified developer, check that it came from this release, then use **System Settings → Privacy & Security → Open Anyway** after the first launch attempt. See [Apple's instructions](https://support.apple.com/en-gb/guide/mac-help/mh40616/mac). Do not disable Gatekeeper globally.
4. Press `⇧⌘V` to open the rail. If another app uses that shortcut, use the CopyRail menu bar entry and check Settings → Shortcuts.
5. To enable direct paste, grant CopyRail Accessibility permission when requested. Copy-only use does not require this permission.

The ZIP is an alternative: extract it and move **CopyRail.app** into Applications. Both downloads contain the same app, installation instructions and licenses. `SHA256SUMS` covers the DMG, ZIP, third-party source archive and `RELEASE_MANIFEST.json`; verify with `shasum -a 256 -c SHA256SUMS` after downloading all four files into the same folder.

Updating retains the existing `io.pasters.desktop` application identity and local history/settings. Installers contain no clipboard history or personal settings. The application starts clipboard collection when launched; hiding the rail leaves it running. Pause collection in Settings or quit to stop it. The app does not enable login startup or grant Accessibility automatically.

## 简体中文

**系统要求：** Apple Silicon Mac（M1 或更新芯片），macOS 13 或更新版本。本次不提供 Intel、Windows 或 Linux 安装包。原生验证在 macOS 26.5.1 完成，较旧系统尚未逐一实测。

1. 从上方 Release 下载 `CopyRail_1.0.0_aarch64.dmg`。
2. 更新前先退出 CopyRail。打开 DMG，把 **CopyRail.app** 拖入 **Applications（应用程序）**，推出磁盘映像，再从应用程序目录启动。
3. 此包采用本地签名，**没有 Apple Developer ID 签名或公证**。若首次打开被提示开发者无法验证，确认下载来源后，在 **系统设置 → 隐私与安全性 → 仍要打开** 中允许本次应用。详见 [Apple 官方说明](https://support.apple.com/en-gb/guide/mac-help/mh40616/mac)，无需关闭系统全局安全检查。
4. 使用 `⇧⌘V` 打开主界面。若快捷键冲突，可通过菜单栏打开并在「设置 → 快捷键」中查看状态。
5. 自动粘贴需要按提示授予 CopyRail 辅助功能权限，仅复制不需要此权限。

也可下载 ZIP，解压后将 CopyRail.app 放入应用程序目录。两种格式包含同一个应用、安装说明和许可证。`SHA256SUMS` 提供安装包及发布清单的校验值。

更新沿用现有应用身份，保留本地历史和设置。安装包不含剪贴板记录或个人设置。启动应用后开始采集，隐藏面板仍会继续运行；可在设置中暂停采集，或退出软件。不会自动开启登录启动或授予辅助功能权限。
