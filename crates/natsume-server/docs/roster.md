# 名单导入与学校 Logo

Server Import 负责固定 XLSX 模板、解析、校验、变更预览和事务提交；Server Contest 负责学校 Logo 的文件匹配、读取、校验、转换及 DOMjudge 导出。下面说明输入文件与图片目录的要求；部署和操作入口见 [Server 指南](../README.md)。

## Excel 模板

使用 [空白模板](../src/component/import/examples/template.xlsx)，或参考含三支虚构队伍、两所虚构学校的 [示例名单](../src/component/import/examples/roster.xlsx)。示例中的密码仅为测试文本。表名必须为 `Teams`，第一行为下面九个表头，列顺序可以调整；其他工作表不参与名单读取。

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

同一中文校名归并为一所学校；没有中文名时以英文名为键。解析结果中的学校按此键排序；Import 根据当前数据库基线规划学校编号，并在提交时持久化分配。同校部分行的英文名可留空，从其他行的非空值补齐；英文名或 country 冲突会报告当前行与该学校首次出现的行。请统一同一学校的命名，导入不猜测学校别名或自动合并校区。

Import 内部的 XLSX 原始文件上限为 8 MiB；HTTP 上传另受 Axum 提取器默认的 2 MiB 上限约束。ZIP 解压后的总大小最多 64 MiB。解析保留的单元格文本累计也最多 64 MiB；共享字符串每引用一次均按其完整 UTF-8 字节数计入，包括表头和 trim 前的文本，不能通过小文件中的重复引用放大内存。名单位于第 2–10001 行，可包含空行；至少有一支队伍。诊断使用实际工作表行列号，错误信息不会回显单元格内容。

## 图片目录

```text
organization-logos/
  示例大学.webp
  Example College.png
```

只扫描目录中的直接子文件，候选扩展名为 `.png`、`.jpg`、`.jpeg`、`.webp`、`.svg`，不区分大小写。图片格式按实际内容识别，不要求内容与扩展名一致；例如 `示例大学.webp` 内实际是 PNG 或 SVG 时也可直接使用，无需重命名扩展名。文件主名必须精确等于名单中的完整中文或英文校名；括号、大小写及其他字符均不自动转换。不使用图片字段、别名配置或外部图片服务。

一所学校匹配到多个文件时报告歧义，例如同时存在 `示例大学.png` 和 `示例大学.webp`，或同时存在中英文文件名。请在目录中只保留一个匹配文件。每所学校展示一条校徽检查结果，Preparation 显示学校编号、匹配文件与状态。

匹配文件必须是普通文件，不能是目录或符号链接。每张图片最多 8 MiB，宽高分别不超过 4096 像素，总像素不超过 4194304；PNG、JPEG、WebP 按内容解码，损坏文件仍会报错。校验不会修改源图片；HTTP 响应与 DOMjudge 导出在内存中转换，不在源目录生成文件。图片目录与文件必须允许 Server 用户读取。

SVG 必须是有效的 SVG XML，支持 XML 声明、UTF-8 BOM 和内部元素引用；校验会解析并在内存中试渲染，检查画布尺寸并保留透明背景。最多 100000 个 XML 节点，按默认 96 DPI 解析尺寸并使用解析后的整数画布，不额外缩放。SVG 应自包含：位图需内嵌为可解码的 PNG、JPEG 或 WebP，不读取其引用的外部文件或网址。文字使用运行环境已安装的字体；将文字转为路径可避免不同环境的字体差异。

## 内部模块与验证

Import 内部的 `workbook` 模块返回包含学校、队伍及受保护密码字段的解析结果。工作表行列、输入规则和解析错误留在 Import，不作为跨应用共享模型。可在 Web Preparation 下载空白模板并上传完整 XLSX；没有 Logo 也可提交名单。

Contest 内部的 `logo_io` 模块负责文件名匹配、读取、解码与转换。HTTP 对栅格图返回原始内容及真实 MIME，对 SVG 返回自然尺寸的 PNG；DOMjudge ZIP 导出统一生成居中、透明补边的 64×64 PNG，名称为 `logos/INST-xxx.png`。具体导出行为见 [Server 指南](../README.md#school-logos-and-domjudge-export)。

```sh
cargo test -p natsume-server --lib component::import --locked
cargo test -p natsume-server --lib component::contest --locked
```
