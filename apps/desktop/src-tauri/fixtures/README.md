# 原生 PDF 合成样本

这些文档内容由项目生成，不含用户文档、剪贴板内容、外部链接或可执行 PDF 动作。内嵌 DejaVu Sans 字体继续适用其原有条款，完整许可见 [DejaVu-Fonts.txt](../../../../licenses/DejaVu-Fonts.txt)。正常发布包不内嵌它们；仅调试隔离场景与测试使用。

- `native-preview-acceptance.pdf`：三页，480×640 / 720×480 / 480×640 pt；内嵌 DejaVu Sans、可选择文字、内部第 1 ↔ 3 页跳转、横版表格及矢量缩放图案。
- `native-preview-locked.pdf`：相同文档的未解锁负例，合成口令 `synthetic-only-password`。RC4-128 仅为该可重复测试生成确定字节，绝不是应用加密方案。
- 另一个无法解析的 PDF 负例由隔离场景在内存中生成，无须保留损坏文件。

更改样本时从项目根目录运行 `scripts/create-native-pdf-fixture.py --font <DejaVuSans.ttf>`，需要 ReportLab 和 pypdf。构建本身无需 Python；已生成的文件直接作为测试资源读取。生成器同时在 `output/pdf/native-preview-acceptance.pdf` 放置可检查副本，随后应通过 Poppler 渲染并逐页检查。样本变更后重新运行完整工程检查。

入口：调试程序 `--native-ui-test --scenario=pdf`；或构建后运行 `sh scripts/prepare-native-qa.sh --pdf`，只准备独立的 `PasteRS PDF QA.app`，不会启动。默认五条 A–E 拖放样本保持不变，不允许通过参数传入用户路径。

验收只操作合成文档的缩略图、分页、缩放、搜索与内部跳转；不要使用查看器内置的复制、打印、保存或外部应用菜单。命令白名单拒绝系统剪贴板写回不等于已经验证 WebKit PDF 插件的每个内置动作都受同一白名单约束。原生 PDF 插件交互、VoiceOver 和该边界仍待单独核验。
