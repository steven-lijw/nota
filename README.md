# Nota

用 Rust + [GPUI](https://www.gpui.rs/) 写的本地 Notion 复刻版：块编辑器、数据库多视图，右侧是调用本机 `claude -p` 的 AI 面板。所有内容都以 Markdown 文件保存在本地的一个 vault 文件夹里。

## 运行

需要：

- Rust（`curl https://sh.rustup.rs -sSf | sh`，用最新 stable）
- macOS：Xcode Command Line Tools（`xcode-select --install`）。默认开启了 `runtime-shaders`，不需要完整的 Xcode。

```bash
cargo run --release              # 第一次编译大约 5–10 分钟
./scripts/bundle-macos.sh        # 打包成 dist/Nota.app，可以拖进「应用程序」
```

## Vault

按以下顺序决定用哪个文件夹：

1. 命令行第一个参数：`cargo run --release -- ~/Documents/MyVault`
2. 环境变量 `NOTA_VAULT`
3. 上次在「Settings → Switch vault…」里选的文件夹（记在 `~/Library/Application Support/nota/config.json`）
4. 默认 `~/Nota`

空文件夹第一次打开会写入几页示例内容。

每个页面是一个 `.md` 文件，开头是 YAML front matter：

```markdown
---
id: 4f2a…               # 稳定 id，文件名只是给人看的
title: 周计划
icon: 📅
parent: 9c1b…           # 父页面或数据库的 id
props:                  # 数据库行的属性值
  Status: In progress
---

正文就是普通 Markdown。嵌套块用 4 个空格缩进。
```

数据库也是一个页面（`kind: database`），`schema` 和 `views` 存在它自己的 front matter 里；每一行是一个 `parent` 指向它的页面。文件可以随意挪进子文件夹，层级只看 `parent`。没有 front matter 的普通 `.md` 拖进来也能打开（会把第一个 `# 标题` 当作标题）。`.nota/state.json` 只存展开状态、深色模式等界面状态。

## 功能

**块编辑器**：正文、H1–H3、无序/有序/待办/折叠列表、引用、Callout、代码块、分割线、子页面链接。`/` 菜单（支持中文/拼音关键词筛选）、Markdown 快捷输入（`# `、`- `、`1. `、`[] `、`> `、`" `、```` ``` ````、`---`）、行内 `**粗体**` `*斜体*` `` `代码` `` `~~删除线~~`、选中文字浮出格式工具栏、左侧 `+` 和拖拽手柄、块菜单（删除/复制/转换）、多块选择、撤销重做、复制粘贴（Markdown）、中文输入法。

**数据库**：Table / Board / List / Gallery / Calendar 五种视图，每个视图独立的筛选、排序、隐藏属性、分组属性和日期属性。属性类型：Text、Number、Select、Multi-select、Status、Date、Checkbox、URL。看板和日历支持拖拽改值。点单元格直接编辑，打开行就是一个完整页面。

**其他**：侧边栏页面树（拖拽移动、收藏、右键菜单）、面包屑、前进后退、`⌘K` 搜索、回收站（恢复/彻底删除）、Emoji 图标、深色模式、在 Finder 中显示。

## AI 面板（`⌘J`）

点右上角 ✦ 或侧边栏「Nota AI」打开。每次提问会在 vault 目录里运行：

```
claude -p --output-format stream-json --verbose --include-partial-messages …
```

- 当前页面内容会作为上下文一起发过去（点输入框上方的页面标签可以关掉）。
- 同一个对话会用 `--resume` 接着上一轮。
- 默认 **Read-only**：只允许 `Read`/`Glob`/`Grep`，Claude 能翻你的整个 vault 来回答问题，但不能改文件。回答下方有「Insert into page」可以把内容插进当前页面。
- 切换到 **Can edit your pages** 后用 `--permission-mode acceptEdits`，Claude 可以直接新建或修改 vault 里的 Markdown 文件，结束后 Nota 会自动重新加载。

前提是 `claude` 在终端里能用并且已经登录。从 Finder 打开的 app 拿不到 shell 的 PATH，Nota 会通过登录 shell 和常见安装位置（`~/.claude/local`、`~/.local/bin`、`/opt/homebrew/bin` 等）去找；都找不到的话可以设 `NOTA_CLAUDE=/完整/路径/claude`。

## 快捷键

| | |
|---|---|
| `⌘N` 新页面 · `⌘K`/`⌘P` 搜索 · `⌘J` AI · `⌘\` 侧边栏 · `⌘⇧L` 深色模式 · `⌘[` / `⌘]` 后退/前进 | |
| `⌘B/I/U` 粗体/斜体/下划线 · `⌘⇧S` 删除线 · `⌘E` 行内代码 | |
| `Tab`/`⇧Tab` 缩进 · `⌘⇧↑/↓` 移动块 · `⌘D` 复制块 · `Esc` 选中整块 · `⌘⌥0–8` 转换块类型 | |

## 代码结构

```
crates/core   与界面无关：数据模型、Markdown 读写、vault、数据库查询（有单元测试：cargo test -p nota-core）
crates/app    GPUI 界面
  workspace.rs     窗口：侧边栏、顶栏、搜索、菜单
  page_view.rs     单个页面：图标、标题、行属性、编辑器或数据库
  editor*.rs       块编辑器
  database.rs      数据库的五种视图和筛选/排序/属性菜单
  props.rs         属性值显示与编辑弹窗
  ai.rs            AI 面板（claude -p）
  text_view.rs     自绘文本元素（换行、光标、选区、输入法）
```

只做了单人本地使用：没有协作、同步、评论、权限、公式/关联属性。GPUI 还在 1.0 之前，升级 `gpui` 版本时可能需要改 API。
