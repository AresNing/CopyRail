//! App-owned copy shared by the web UI and native menus. User content is never localized.
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub enum Language {
    #[default]
    #[serde(rename = "zh-CN")]
    Chinese,
    #[serde(rename = "en")]
    English,
}

impl Language {
    pub const fn code(self) -> &'static str {
        match self {
            Self::Chinese => "zh-CN",
            Self::English => "en",
        }
    }
}

/// Persist the user's intent separately from the resolved display language.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub enum LanguagePreference {
    #[default]
    #[serde(rename = "system")]
    System,
    #[serde(rename = "zh-CN")]
    Chinese,
    #[serde(rename = "en")]
    English,
}

impl LanguagePreference {
    pub const fn code(self) -> &'static str {
        match self {
            Self::System => "system",
            Self::Chinese => "zh-CN",
            Self::English => "en",
        }
    }

    pub fn resolve(self, system_language: &str) -> Language {
        match self {
            Self::Chinese => Language::Chinese,
            Self::English => Language::English,
            Self::System => {
                if system_language
                    .trim()
                    .split(['-', '_'])
                    .next()
                    .is_some_and(|code| code.eq_ignore_ascii_case("zh"))
                {
                    Language::Chinese
                } else {
                    Language::English
                }
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct LanguageSettings {
    pub preference: LanguagePreference,
    pub effective: Language,
}

/// Translate an app-owned key. Unknown technical messages remain readable.
pub fn translate(language: Language, source: &str) -> &str {
    if language == Language::Chinese {
        return source;
    }
    match source {
        "待处理的同步冲突" => "Sync conflicts",
        "自动顺序原本偏向另一台设备" => {
            "Automatic ordering preferred the other device"
        }
        "自动顺序原本偏向此 Mac" => "Automatic ordering preferred this Mac",
        "此 Mac" => "This Mac",
        "另一台设备" => "Other device",
        "保留此 Mac" => "Keep this Mac",
        "采用另一台设备" => "Use other device",
        "（删除状态）" => "(deleted)",
        "此版本删除了该内容。采用后会同步删除状态。" => {
            "This version deleted the item. Choosing it will sync the deletion."
        }
        "非文本内容；采用时会保留该版本的原始格式和附件。" => {
            "Non-text content. Choosing this version preserves its original formats and attachments."
        }
        "共享内容冲突" => "Shared content conflicts",
        "查看当前内容及两份保留版本。选择会生成一个新版本；已移出的私人原记录不会被恢复或覆盖。每次显示最多 50 项。" => {
            "Compare the current content and both saved versions. Your choice creates a new version without restoring or overwriting private originals that were moved out. Up to 50 conflicts are shown."
        }
        "保留当前删除" => "Keep current deletion",
        "保留当前内容" => "Keep current content",
        "采用版本 A（删除）" => "Use version A (deleted)",
        "采用版本 A" => "Use version A",
        "采用版本 B（删除）" => "Use version B (deleted)",
        "采用版本 B" => "Use version B",
        "当前" => "Current",
        "版本 A" => "Version A",
        "版本 B" => "Version B",
        "此共享板为只读，只有可编辑成员可以提交选择。" => {
            "This shared board is read-only. Only members with edit access can resolve conflicts."
        }
        "桌面服务暂时不可用" => "Desktop service is temporarily unavailable",
        "无法打开内容编辑器，请重试。" => {
            "Could not open the editor. Please try again."
        }
        "没有可撤销的删除操作。" => "No deletion to undo.",
        "无法选择预览正文，请重新操作。" => {
            "Could not select the preview text. Please try again."
        }
        "已复制当前预览内容。" => "Preview content copied.",
        "已复制选中内容。" => "Selection copied.",
        "请先进入内容编辑器，在输入框中剪切或粘贴。" => {
            "Open the content editor to cut or paste in a text field."
        }
        "列表当前没有可重做的操作。" => "Nothing to redo in this list.",
        "内容已复制；未确认当前目标，请手动粘贴或从目标应用重新唤起 CopyRail。" => {
            "Content copied. Paste manually, or reopen CopyRail from the target app to confirm the destination."
        }
        "当前类型暂不支持内容编辑。" => {
            "Editing is not available for this content type yet."
        }
        "已在 CopyRail 内置浏览器中打开链接。" => {
            "Link opened in the CopyRail browser."
        }
        "当前仅支持在内置浏览器中打开链接。" => {
            "Only links can open in the built-in browser."
        }
        "没有可拖出的内容。" => "No content available to drag.",
        "分类位置已变化或排序落点已失效，本次未移动；请重新拖到目标位置。" => {
            "The board or drop position changed. Nothing was moved; drag to the destination again."
        }
        "项目已在这个位置。" => "The item is already here.",
        "Pinboard 顺序已保存。" => "Pinboard order saved.",
        "直接粘贴权限已启用。" => "Direct paste access is enabled.",
        "已请求 macOS 授权；开启 CopyRail 后请点“重新检测”。" => {
            "macOS access requested. Enable CopyRail, then click Check again."
        }
        "已确认直接粘贴权限。" => "Direct paste access confirmed.",
        "尚未授予辅助功能权限；复制仍可正常使用。" => {
            "Accessibility access has not been granted. Copying still works."
        }
        "iCloud 同步已启用，正在后台检查账户并同步；失败时本地队列会完整保留。" => {
            "iCloud sync enabled. Checking the account and syncing in the background; failures keep the local queue intact."
        }
        "已保存 iCloud 同步选择；当前构建不访问 CloudKit，本地待发送队列会完整保留。" => {
            "iCloud preference saved. This build does not access CloudKit; all pending local changes are retained."
        }
        "iCloud 同步已关闭；不会初始化 CloudKit。" => {
            "iCloud sync disabled. CloudKit will not be initialized."
        }
        "已保留此 Mac 的最新版本，最终选择已加入同步队列。" => {
            "Kept this Mac's latest version and queued the resolution for sync."
        }
        "已采用另一台设备的版本，最终选择已加入同步队列。" => {
            "Used the other device's version and queued the resolution for sync."
        }
        "选择已保存并加入共享待发送队列；尚未上传。" => {
            "Choice saved and queued for sharing. It has not been uploaded yet."
        }
        "MCP 本地访问已启用。" => "Local MCP access enabled.",
        "MCP 本地访问已停用，现有连接立即失效。" => {
            "Local MCP access disabled. Existing connections are immediately invalidated."
        }
        "请填写要连接的客户端名称。" => {
            "Enter a name for the client you want to connect."
        }
        "连接已创建并启用；配置只显示这一次，请立即保存到目标客户端。" => {
            "Connection created and enabled. This configuration is shown only once; save it in the target client now."
        }
        "该 MCP 客户端已撤销，正在运行的连接也会失效。" => {
            "MCP client revoked. Active connections will also be invalidated."
        }
        "Pinboard 已更新。" => "Pinboard updated.",
        "Pinboard 项目顺序已保存。" => "Pinboard item order saved.",
        "Pinboard 已删除，剪贴板历史保持不变。" => {
            "Pinboard deleted. Clipboard history is unchanged."
        }
        "菜单打开期间列表已变化，请重新选择内容。" => {
            "The list changed while the menu was open. Select the content again."
        }
        "已向目标应用发送粘贴请求。" => "Paste request sent to the target app.",
        "菜单编辑内容已失效。" => "The item selected in the menu is no longer available.",
        "Pinboard 归属已更新，剪贴板历史保持不变。" => {
            "Pinboard assignment updated. Clipboard history is unchanged."
        }
        "标题不能为空。" => "The title cannot be empty.",
        "标题已更新。" => "Title updated.",
        "内容不能为空。" => "Content cannot be empty.",
        "请输入六位色值：带 #，或至少含一个 A–F 字母。" => {
            "Enter a six-digit hex color with # or at least one letter A–F."
        }
        "内容已更新并重新建立搜索索引。" => {
            "Content updated and search index refreshed."
        }
        "新内容已保存到本地历史。" => "New content saved to local history.",
        "图片已在本地旋转并保存。" => "Image rotated and saved locally.",
        "搜索剪贴板历史" => "Search clipboard history",
        "搜索复制过的内容" => "Search clipboard history",
        "按内容类型筛选" => "Filter by content type",
        "筛选" => "Filter",
        "新建文本、链接或颜色" => "New text, link or color",
        "编辑选中内容" => "Edit selected content",
        "编辑" => "Edit",
        "重命名选中项目（⌘R）" => "Rename selected item (⌘R)",
        "命名" => "Rename",
        "在 Pinboard 中向前移动" => "Move earlier in pinboard",
        "在 Pinboard 中向后移动" => "Move later in pinboard",
        "移出" => "Remove",
        "归类" => "Organize",
        "清空待粘贴列表（不会删除历史）" => "Clear paste queue (keeps history)",
        "顺序粘贴" => "Paste queue",
        "合成历史；不访问系统剪贴板或 iCloud" => {
            "Synthetic history; no system clipboard or iCloud access"
        }
        "隔离验证" => "Demo",
        "在设置中管理采集" => "Manage capture in settings",
        "正在暂停…" => "Pausing…",
        "正在恢复…" => "Resuming…",
        "正在应用设置…" => "Applying settings…",
        "采集已暂停" => "Capture paused",
        "采集与隐私设置" => "Capture and privacy settings",
        "新建 Pinboard" => "New pinboard",
        "编辑当前 Pinboard" => "Edit current pinboard",
        "重命名、改色或排序" => "Rename, change color or reorder",
        "组合筛选" => "Combined filters",
        "清除" => "Clear",
        "内容" => "Content",
        "来源应用" => "Source app",
        "设备" => "Device",
        "时间" => "Time",
        "最近 24 小时" => "Last 24 hours",
        "最近 7 天" => "Last 7 days",
        "最近 30 天" => "Last 30 days",
        "名称" => "Name",
        "分类名称" => "Pinboard name",
        "颜色" => "Color",
        "Pinboard 颜色" => "Pinboard color",
        "创建" => "Create",
        "编辑 Pinboard" => "Edit pinboard",
        "编辑分类" => "Edit pinboard",
        "向前移动" => "Move earlier",
        "向后移动" => "Move later",
        "删除" => "Delete",
        "保存" => "Save",
        "固定到 Pinboard" => "Pin to a board",
        "固定到" => "Pin to",
        "先新建一个 Pinboard" => "Create a pinboard first",
        "重命名" => "Rename",
        "编辑内容" => "Edit content",
        "新建内容" => "New content",
        "只修改显示标题，不改动原始剪贴板内容。" => {
            "Only the display title changes; the original clipboard content is preserved."
        }
        "内容仅保存在本机，保存后会立即更新搜索索引。" => {
            "Content stays on this Mac. Saving updates the search index immediately."
        }
        "关闭编辑器" => "Close editor",
        "内容类型" => "Content type",
        "文本" => "Text",
        "链接" => "Link",
        "标题" => "Title",
        "留空则使用内容首行" => "Leave blank to use the first line",
        "选择颜色" => "Choose a color",
        "十六进制颜色值" => "Hex color value",
        "输入要保存的文本…" => "Enter text to save…",
        "标题可用于搜索；原始格式和内容保持不变。" => {
            "Search by this title. Original formats and content are preserved."
        }
        "保存会保留 Pinboard 归属，并把编辑后的项目移到历史最前。" => {
            "Saving keeps pinboard assignments and moves the edited item to the top of history."
        }
        "新项目的标题会自动取内容首行。" => {
            "New items use the first line as their title."
        }
        "取消" => "Cancel",
        "左右箭头选择 · Space 预览 · F2 卡片操作" => {
            "Arrow keys to select · Space to preview · F2 for card actions"
        }
        "搜索结果" => "Search results",
        "剪贴板时间线" => "Clipboard timeline",
        "正在加载内容…" => "Loading content…",
        "暂时无法加载内容" => "Content is temporarily unavailable",
        "没有找到匹配内容" => "No matching content",
        "给这个分类放入第一条内容" => "Add the first item to this pinboard",
        "留住每一次有用的复制" => "Keep every useful copy",
        "结果更新后即可选择，不会操作上一次查询的内容。" => {
            "Select items once the results update. Previous search results will not be affected."
        }
        "请查看错误提示，稍后将自动重试。" => {
            "Check the error message. CopyRail will retry shortly."
        }
        "已搜索全部历史与 Pinboard；试试其他关键词或清除筛选。" => {
            "Searched all history and pinboards. Try another keyword or clear the filters."
        }
        "从历史或其他 Pinboard 拖入便签，也可以新建内容。" => {
            "Drag items from history or another pinboard, or create new content."
        }
        "当前仅使用合成数据，不监听系统剪贴板或连接 iCloud。" => {
            "This demo uses synthetic data without accessing the system clipboard or iCloud."
        }
        "剪贴板采集已暂停，恢复后才会收集新内容。" => {
            "Clipboard capture is paused. Resume it to collect new content."
        }
        "新复制的内容将保存在此处，机密与瞬态内容默认跳过。" => {
            "New copies appear here. Confidential and transient content is skipped by default."
        }
        "清除搜索与筛选" => "Clear search and filters",
        "＋ 新建内容" => "＋ New content",
        "设置自动粘贴" => "Set up direct paste",
        "CopyRail 设置" => "CopyRail settings",
        "设置" => "Settings",
        "关闭设置" => "Close settings",
        "设置分类" => "Settings categories",
        "通用" => "General",
        "快捷键" => "Shortcuts",
        "历史与隐私" => "History & privacy",
        "备份" => "Backup",
        "高级" => "Advanced",
        "启动、显示与粘贴" => "Startup, appearance and paste",
        "登录时自动启动" => "Launch at login",
        "紧凑卡片布局" => "Compact cards",
        "背景透明度" => "Background transparency",
        "不透明" => "Opaque",
        "通透" => "Clear",
        "调整后自动保存，应用于所有窗口。" => {
            "Saves automatically and applies to all windows."
        }
        "正在保存外观…" => "Saving appearance…",
        "请输入正整数，留空表示不限。" => {
            "Enter a positive whole number, or leave blank for no limit."
        }
        "请输入有效的应用 Bundle ID。" => "Enter a valid app bundle ID.",
        "重试" => "Retry",
        "隔离验证不能修改登录启动。" => {
            "Isolated verification cannot change login startup."
        }
        "外观保存失败，请重试。" => "Could not save appearance. Try again.",
        "顺序粘贴说明" => "About the paste queue",
        "点卡片上的「＋」按顺序加入待粘贴列表。列表有内容时，回车优先粘贴第一条；再次唤起后可继续下一条。" => {
            "Use + on each card to build a paste queue. Return pastes the first item; reopen CopyRail for the next one."
        }
        "主界面的数字表示剩余条数。点击「顺序粘贴」清空列表，不会删除历史；仅复制或粘贴请求失败时不会移出该条。" => {
            "The count shows remaining items. Click Paste queue to clear the list without deleting history. Copy-only results and failed requests keep the item queued."
        }
        "直接粘贴权限" => "Direct paste access",
        "直接粘贴" => "Direct paste",
        "复制无需授权；向目标应用发送 ⌘V 需要 macOS 辅助功能权限。" => {
            "Copying needs no permission. Direct paste requires macOS Accessibility access."
        }
        "已授权" => "Granted",
        "待授权" => "Not granted",
        "检测中" => "Checking",
        "当前运行的应用：" => "Running application:",
        "授权故障排查" => "Troubleshooting",
        "系统开关已开启却仍显示待授权？内测更新后，旧授权可能仍绑定旧签名。先退出 CopyRail，在辅助功能列表选中旧 CopyRail，点“−”移除，再点“＋”添加上方路径的应用。重新打开并点“重新检测”。仅搬到 Applications 不会更新旧授权；授权后请回到目标输入框重新唤起，不会补发上次粘贴。" => {
            "Access enabled in macOS but still shown as not granted? After a beta update, access may be tied to an old signature. Quit CopyRail, remove its old entry from Accessibility with −, then use + to add the app at the path above. Reopen it and click Check again. Moving the app to Applications alone does not refresh access. After granting access, reopen CopyRail from the target text field; the previous paste is not retried."
        }
        "授权直接粘贴" => "Allow direct paste",
        "重新检测" => "Check again",
        "快速打开与键盘操作" => "Open quickly and navigate with the keyboard",
        "全局快捷键" => "Global shortcut",
        "全局快捷键 ⇧⌘V" => "Global shortcut ⇧⌘V",
        "隔离验证不会注册系统快捷键。" => {
            "Demo mode does not register a system shortcut."
        }
        "快捷键已注册；也可通过菜单栏「显示 CopyRail」打开。" => {
            "Shortcut registered. You can also use Show CopyRail in the menu bar."
        }
        "快捷键暂不可用，可能已被其他软件占用。可从菜单栏「显示 CopyRail」打开；释放此组合键后点击「重新启用」。" => {
            "The shortcut is unavailable and may be used by another app. Use Show CopyRail in the menu bar, then free the shortcut and click Enable again."
        }
        "正在读取快捷键状态；菜单栏入口仍可使用。" => {
            "Checking shortcut status. The menu bar entry is still available."
        }
        "处理中" => "Working",
        "隔离未注册" => "Demo only",
        "已注册" => "Registered",
        "未启用" => "Disabled",
        "未知" => "Unknown",
        "技术详情" => "Technical details",
        "正在启用…" => "Enabling…",
        "重新启用" => "Enable again",
        "界面快捷键" => "Keyboard reference",
        "选择内容" => "Select an item",
        "预览 / 关闭预览" => "Toggle preview",
        "粘贴" => "Paste",
        "以纯文本粘贴" => "Paste as plain text",
        "复制" => "Copy",
        "加入 / 移出顺序粘贴" => "Add to / remove from queue",
        "关闭预览、设置或主界面" => "Close preview, settings or panel",
        "保留范围与忽略规则" => "Retention and excluded apps",
        "剪贴板采集" => "Clipboard capture",
        "暂停期间不保存新复制的内容，15 分钟后自动恢复。" => {
            "Pause stops saving new copies and resumes automatically after 15 minutes."
        }
        "隔离验证不采集系统剪贴板。" => {
            "Demo mode does not access the system clipboard."
        }
        "尚未确认生效；等待当前读取或写入结束，请暂勿复制敏感内容。" => {
            "Waiting for confirmation. Avoid copying sensitive content until the current operation finishes."
        }
        "控制剪贴板采集" => "Control clipboard capture",
        "暂停 15 分钟" => "Pause for 15 minutes",
        "最多保留天数" => "Keep history for (days)",
        "永久" => "Forever",
        "最多保留未固定项目" => "Maximum unpinned items",
        "不限" => "Unlimited",
        "忽略这些应用（每行一个 Bundle ID）" => {
            "Excluded apps (one bundle ID per line)"
        }
        "屏幕共享时隐藏内容" => "Hide content while screen sharing",
        "固定到 Pinboard 的内容不会被保留策略清理。机密和瞬态剪贴板类型始终默认跳过。" => {
            "Pinned items are kept regardless of retention limits. Confidential and transient clipboard types are skipped by default."
        }
        "导出与恢复本地数据" => "Export and restore local data",
        "本地备份" => "Local backup",
        "包含历史、Pinboards 与本地设置；不上传到云端。" => {
            "Includes history, pinboards and local settings. Nothing is uploaded."
        }
        "导出备份" => "Export backup",
        "恢复备份" => "Restore backup",
        "本机集成与实验功能" => "Local integrations and experimental features",
        "iCloud 同步状态" => "iCloud sync status",
        "iCloud 同步" => "iCloud sync",
        "本地同步队列不可用" => "Local sync queue unavailable",
        "正在检查本地同步状态…" => "Checking local sync status…",
        "启用 iCloud 同步" => "Enable iCloud sync",
        "MCP 本地访问" => "Local MCP access",
        "让明确授权的 AI 客户端通过本机 stdio 搜索、读取和整理历史。默认关闭，不开放网络端口。" => {
            "Let explicitly authorized AI clients search, read and organize history over local stdio. Off by default; no network port is opened."
        }
        "启用 MCP 本地访问" => "Enable local MCP access",
        "客户端名称，例如 Codex" => "Client name, e.g. Codex",
        "创建并启用" => "Create and enable",
        "尚未授权客户端" => "No authorized clients",
        "尚未使用" => "Never used",
        "撤销" => "Undo",
        "撤销客户端" => "Revoke client",
        "正在检查授权状态…" => "Checking authorization…",
        "仅显示一次的连接配置" => "One-time connection configuration",
        "其中包含访问凭据。保存到目标客户端后请关闭此面板；不要粘贴到聊天或提交到仓库。" => {
            "This contains access credentials. Save it in the target client, then close this panel. Do not paste it into chats or commit it to a repository."
        }
        "通用与隐私选项修改后保存" => {
            "Save after changing general or privacy preferences"
        }
        "保存设置" => "Save changes",
        "PDF 首页缩略图" => "PDF first-page thumbnail",
        "剪贴板图片预览" => "Clipboard image preview",
        "PDF · 首页" => "PDF · First page",
        "跳回历史位置" => "Show in history",
        "加入或移出 顺序粘贴（⌘↩）" => "Add to or remove from paste queue (⌘↩)",
        "PDF 完整预览" => "Full PDF preview",
        "PDF 完整预览不可用。" => "Full PDF preview is unavailable.",
        "重试 PDF 预览" => "Retry PDF preview",
        "正在加载完整 PDF…" => "Loading full PDF…",
        "正在生成预览…" => "Preparing preview…",
        "剪贴板图片完整预览" => "Full clipboard image preview",
        "PDF 预览" => "PDF preview",
        "Quick Look 预览" => "Quick Look preview",
        "关闭预览" => "Close preview",
        "Esc 关闭 · Return 粘贴" => "Esc to close · Return to paste",
        "图片快速操作" => "Image actions",
        "向左旋转" => "Rotate left",
        "向右旋转" => "Rotate right",
        "旋转中…" => "Rotating…",
        "识别中…" => "Recognizing…",
        "提取文字" => "Extract text",
        "在 CopyRail 内置浏览器中打开（⌘O）" => "Open in the CopyRail browser (⌘O)",
        "内置浏览器打开" => "Open in browser",
        "刚刚" => "Just now",
        "暂停中 · 点击恢复" => "Paused · Click to resume",
        "粘贴为纯文本" => "Paste as plain text",
        "复制为纯文本" => "Copy as plain text",
        "快速预览" => "Quick preview",
        "加入或移出 顺序粘贴" => "Add to or remove from paste queue",
        "Pin 到…" => "Pin to…",
        "移出 Pinboard…" => "Remove from pinboard…",
        "在剪贴板历史中显示" => "Show in clipboard history",
        "删除选中内容" => "Delete selected content",
        "显示 CopyRail" => "Show CopyRail",
        "暂停采集 15 分钟" => "Pause capture for 15 minutes",
        "继续采集" => "Resume capture",
        "退出 CopyRail" => "Quit CopyRail",
        "重做" => "Redo",
        "剪切" => "Cut",
        "全选" => "Select all",
        "语言" => "Language",
        "跟随系统" => "System default",
        "界面语言" => "Interface language",
        "立即保存，无需重启。" => "Saved immediately. No restart needed.",
        "正在保存语言…" => "Saving language…",
        "无法保存语言，请重试。" => "Could not save language. Please try again.",
        "剪贴板" => "Clipboard",
        "请等待 Writing Tools 完成后再保存。" => {
            "Wait for Writing Tools to finish before saving."
        }
        "正在保存…" => "Saving…",
        "放弃尚未保存的修改？" => "Discard unsaved changes?",
        "原来的剪贴板记录不会改变。" => "The original clipboard item will not change.",
        "继续编辑" => "Keep editing",
        "放弃更改" => "Discard changes",
        "有尚未保存的富文本修改" => "Unsaved rich-text changes",
        "继续编辑可保留草稿；退出会放弃所有未保存修改，原记录保持不变。" => {
            "Keep editing to preserve your drafts. Quitting discards all unsaved changes and keeps the original items."
        }
        "放弃并退出" => "Discard and quit",
        "请先关闭一个编辑窗口（最多 8 个）。" => {
            "Close an editor window first (maximum 8)."
        }
        "⌘S 保存 · Esc 取消；支持系统字体面板、撤销与右键文本操作。" => {
            "⌘S to save · Esc to cancel. Supports the system font panel, undo and text context menus."
        }
        "打开时定位" => "Selection on open",
        "自动保存，下次打开主界面时生效。" => {
            "Saves automatically. Applies the next time you open the rail."
        }
        "最新内容" => "Latest item",
        "上次停留的位置" => "Last position",
        "定位设置保存失败，请重试。" => {
            "Could not save selection preference. Try again."
        }
        "无法记住当前位置，请重试。" => "Could not remember your position. Try again.",
        "字体…" => "Fonts…",
        "⌘S 保存 · Esc 取消" => "⌘S Save · Esc Cancel",
        "部分格式已简化。保存使用当前格式，取消保留原内容。" => {
            "Some formatting was simplified. Save keeps this version; Cancel keeps the original."
        }
        "下划线" => "Underline",
        "左对齐" => "Align left",
        "居中" => "Center",
        "导出 CopyRail 本地备份" => "Export CopyRail local backup",
        "选择 CopyRail 本地备份" => "Choose CopyRail local backup",
        "恢复会用备份中的历史、Pinboards 和本地设置替换当前数据。此操作完成后不能自动撤销。" => {
            "Restoring replaces current history, pinboards and local settings with the backup. This cannot be undone automatically."
        }
        "恢复 CopyRail 备份？" => "Restore CopyRail backup?",
        "内容已复制；自动粘贴需要在 macOS“系统设置 → 隐私与安全性 → 辅助功能”中允许 CopyRail。" => {
            "Content copied. To paste directly, allow CopyRail in macOS System Settings → Privacy & Security → Accessibility."
        }
        "隔离验证不创建 CloudKit 客户端或同步线程。" => {
            "Demo mode does not create a CloudKit client or sync worker."
        }
        "当前构建未包含 CloudKit 传输；本地队列不会被标记为已发送。" => {
            "This build does not include CloudKit transport. Local changes will not be marked as sent."
        }
        "当前签名未获得 CopyRail CloudKit 容器 entitlement；不会连接 iCloud。" => {
            "This signature has no CopyRail CloudKit container entitlement. It will not connect to iCloud."
        }
        "同步状态暂时不可用。" => "Sync status is temporarily unavailable.",
        _ => source,
    }
}

#[cfg(test)]
mod language_tests {
    use super::*;
    #[test]
    fn follows_the_primary_system_language_and_keeps_manual_overrides() {
        assert_eq!(LanguagePreference::default(), LanguagePreference::System);
        for tag in ["zh", "zh-CN", "zh-Hans-CN", "zh-Hant-TW", "ZH_hk"] {
            assert_eq!(LanguagePreference::System.resolve(tag), Language::Chinese);
            assert_eq!(LanguagePreference::English.resolve(tag), Language::English);
        }
        for tag in ["en", "en-US", "en-GB", "ja-JP", "fr-FR", "", "zho", "en-zh"] {
            assert_eq!(LanguagePreference::System.resolve(tag), Language::English);
            assert_eq!(LanguagePreference::Chinese.resolve(tag), Language::Chinese);
        }
    }
}
