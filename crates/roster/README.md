# 名单与学校 Logo 预检

`natsume-check-logos` 读取固定 XLSX 模板和本地图片目录，按学校汇总缺图、同校多图和无效图片。工具只读文件，不需要 Server、数据库、配置文件或网络连接。

```sh
cargo build -p natsume-roster --bin natsume-check-logos --locked
./target/debug/natsume-check-logos roster.xlsx --logos ./organization-logos
```

例如：

```text
 WARN School logo is missing school="示例学院" rows=[4] expected_stems=["示例学院", "Example College"]
 INFO Logo check completed schools=2 matched=1 missing=1 ambiguous=0 invalid=0
```

缺图时直接把图片补充到目录中，或按名单中的校名重命名，再运行同一命令。每次重新扫描目录，输入文件不会被修改，报告不输出账号密码。

诊断及统计统一通过 `tracing` 写入 stderr：缺图和歧义为 WARN，无效图片和检查失败为 ERROR，汇总为 INFO。日志使用结构化字段，不包含 ANSI 控制序列；可用 `2> logos.log` 保存。

| 退出码 | 含义 |
| --- | --- |
| 0 | 所有学校都有唯一、可读且可解码的图片 |
| 1 | 检查完成，但有缺图、歧义或无效图片 |
| 2 | 参数、工作簿或目录错误导致无法完成检查 |

## Excel 模板

使用 [空白模板](examples/template.xlsx)，或参考含三支虚构队伍、两所虚构学校的 [示例名单](examples/roster.xlsx)。示例中的密码仅为测试文本。表名必须为 `Teams`，第一行为下面九个表头，列顺序可以调整；其他工作表不参与名单读取。

| 列 | 规则 |
| --- | --- |
| organization_zh | 学校中文名，与英文名至少填写一项 |
| organization_en | 学校英文名，同校非空英文名必须一致 |
| country | 三位大写国家代码；单元格留空时使用 CHN，同校需一致 |
| account | 唯一的 DOMjudge 账号，必须符合下面的 ID 规则 |
| password | 当前密码，非空，保留原始空格和字符，最多 512 个 UTF-8 字节 |
| seat | 唯一座位号，保留原始文本及前导零，最多 64 个 UTF-8 字节 |
| team_name_zh | 队伍中文名，与英文名至少填写一项 |
| team_name_en | 队伍英文名 |
| category | 类别代码，如 participant、star，必须符合下面的 ID 规则 |

所有单元格均使用文本类型。数值、日期、布尔值、公式及 Excel 错误单元格会被拒绝；字面字符串 `#N/A` 可用。请粘贴为值，不能依靠 Excel 的数字显示格式保留前导零。表头必须完整，即使 country 等单元格可以留空也应保留对应列。

账号和类别 ID 为 1–36 个 ASCII 字母、数字、`_`、`.`、`-`，不能以 `.` 或 `-` 开头、不能以 `.` 结尾。账号、密码和座位按原文本保存；其他字段去掉首尾空白。所有字段禁止控制字符，学校名和队名最多 1024 个 UTF-8 字节。

同一中文校名归并为一所学校；没有中文名时以英文名为键。学校按此键排序，工具不分配 INST ID。同校部分行的英文名可留空，从其他行的非空值补齐；英文名或 country 冲突会报告当前行与该学校首次出现的行。请统一同一学校的命名，工具不猜测学校别名或自动合并校区。

XLSX 最多 8 MiB，ZIP 解压后的总大小最多 64 MiB。名单位于第 2–10001 行，可包含空行；至少有一支队伍。诊断使用实际工作表行列号，错误信息不会回显单元格内容。

## 图片目录

```text
organization-logos/
  示例大学.webp
  Example College.png
```

只扫描目录中的直接子文件，候选扩展名为 `.png`、`.jpg`、`.jpeg`、`.webp`、`.svg`，不区分大小写。图片格式按实际内容识别，不要求内容与扩展名一致；例如 `示例大学.webp` 内实际是 PNG 或 SVG 时也可直接使用，无需重命名扩展名。文件主名必须精确等于名单中的完整中文或英文校名；括号、大小写及其他字符均不自动转换。不使用图片字段、别名配置或外部图片服务。

一所学校匹配到多个文件时报告歧义，例如同时存在 `示例大学.png` 和 `示例大学.webp`，或同时存在中英文文件名。请在目录中只保留一个匹配文件。同校多支队伍只检查一次，报告列出涉及的行号。

匹配文件必须是普通文件，不能是目录或符号链接。每张图片最多 8 MiB，宽高分别不超过 4096 像素，总像素不超过 4194304；PNG、JPEG、WebP 按内容解码，损坏文件仍会报错。工具不会输出 PNG 文件或修改源图片。检查的是当前用户的读权限，部署后仍需确保 Server 用户可读。

SVG 必须是有效的 SVG XML，支持 XML 声明、UTF-8 BOM 和内部元素引用；预检会解析并在内存中试渲染，检查画布尺寸并保留透明背景。最多 100000 个 XML 节点，按默认 96 DPI 解析尺寸并使用解析后的整数画布，不额外缩放。SVG 应自包含：位图需内嵌为可解码的 PNG、JPEG 或 WebP，不读取其引用的外部文件或网址。文字使用运行环境已安装的字体；将文字转为路径可避免不同环境的字体差异。

## 库边界与验证

`parse_xlsx` 返回包含学校、队伍及受保护密码字段的只读资料；`LogoDirectory` 负责文件名匹配，`validate_logo` 负责图片读取和解码验证。公共接口不依赖 HTTP、数据库或控制协议。

Server 已复用同一个 `parse_xlsx` 入口。可在 Web Preparation 下载空白模板并上传完整 XLSX；Server 负责稳定 INST ID 分配、变更预览和事务提交。离线预检只负责校验，不分配 ID；没有 Logo 也可提交名单。

TODO(roster-export)：DOMjudge 导出统一写入实际 PNG；按内容解码栅格源图、栅格化 SVG，生成 `logos/INST-xxx.png`，不根据源扩展名推断格式。此步骤尚未实现。

```sh
cargo test -p natsume-roster --locked
cargo clippy -p natsume-roster --all-targets --locked -- -D warnings
```
