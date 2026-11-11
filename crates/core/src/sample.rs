//! Starter content for a brand new vault.

use crate::markdown::from_markdown;
use crate::model::*;
use crate::query::{date_to_string, today};
use crate::vault::Vault;
use anyhow::Result;

fn opt(name: &str, color: &str) -> SelectOption {
    SelectOption {
        name: name.into(),
        color: color.into(),
    }
}

pub fn seed(vault: &mut Vault) -> Result<()> {
    // --- Getting started -------------------------------------------------
    let home = vault.create_page(PageKind::Page, None);
    let shortcuts = vault.create_page(PageKind::Page, Some(home.clone()));
    {
        let p = vault.page_mut(&shortcuts).unwrap();
        p.title = "快捷键速查".into();
        p.icon = Some("⌨️".into());
        p.blocks = from_markdown(
            r#"## 文本格式

- `⌘B` 加粗 · `⌘I` 斜体 · `⌘U` 下划线
- `⌘⇧S` 删除线 · `⌘E` 行内代码
- 选中文字后粘贴一个网址，会直接变成链接

## 块操作

- `/` 打开块菜单，输入文字可以筛选
- `Tab` / `⇧Tab` 缩进与取消缩进
- `⌘⇧↑` / `⌘⇧↓` 上下移动当前块
- `⌘D` 复制当前块
- `⌘⌥1/2/3` 标题，`⌘⌥4` 待办，`⌘⌥5` 列表，`⌘⌥0` 正文

## Markdown 快捷输入

在一行开头输入下面的符号再按空格：

- `#` `##` `###` 三级标题
- `-` 无序列表，`1.` 有序列表，`[]` 待办
- `>` 折叠列表，`"` 引用，`---` 分割线，三个反引号是代码块

## 全局

- `⌘P` 或 `⌘K` 搜索，`⌘N` 新建页面
- `⌘\` 收起侧边栏，`⌘J` 打开 AI 面板
"#,
        );
    }
    {
        let p = vault.page_mut(&home).unwrap();
        p.title = "快速上手".into();
        p.icon = Some("👋".into());
        p.favorite = true;
        let md = format!(
            r#"欢迎使用 **Nota** —— 一个用 Rust 和 GPUI 写的本地笔记应用。所有内容都以 Markdown 文件保存在你的 vault 文件夹里。

> [!💡] 在任意空行输入 `/` 可以插入各种块；选中文字可以设置**加粗**、*斜体*、<u>下划线</u>、~~删除线~~ 和 `行内代码`。

## 基础块

- 这是一个无序列表
    - 按 `Tab` 可以缩进成子项
- 把鼠标移到块的左侧，可以看到 `+` 和拖拽手柄

1. 有序列表会自动编号
2. 按回车继续下一项

- [x] 已完成的待办
- [ ] 点击方框可以勾选

+ 这是一个折叠列表，点击左侧箭头展开
    里面可以放任何内容。

> 引用块：保持简单，保持专注。

```rust
fn main() {{
    println!("Hello, Nota!");
}}
```

---

## 页面与数据库

侧边栏里每个页面都可以无限嵌套子页面。输入 `/page` 可以在当前位置新建子页面：

[[{shortcuts}|快捷键速查]]

数据库支持表格、看板、列表、画廊和日历五种视图，可以到侧边栏的「任务看板」里试试。

## AI 助手

点击右上角的 ✦ 按钮（或 `⌘J`）打开右侧 AI 面板。它会调用你电脑上的 `claude -p`，并且以 vault 文件夹作为工作目录，所以可以直接基于你的笔记回答问题。
"#,
            shortcuts = shortcuts
        );
        p.blocks = from_markdown(&md);
    }

    // --- Tasks database --------------------------------------------------
    let db = vault.create_page(PageKind::Database, None);
    {
        let p = vault.page_mut(&db).unwrap();
        p.title = "任务看板".into();
        p.icon = Some("✅".into());
        let mut status = Property::new("Status", PropKind::Status);
        status.options = vec![
            opt("Not started", "gray"),
            opt("In progress", "blue"),
            opt("Done", "green"),
        ];
        let mut priority = Property::new("Priority", PropKind::Select);
        priority.options = vec![opt("High", "red"), opt("Medium", "yellow"), opt("Low", "gray")];
        let mut tags = Property::new("Tags", PropKind::MultiSelect);
        tags.options = vec![
            opt("学习", "purple"),
            opt("工作", "blue"),
            opt("生活", "orange"),
            opt("健身", "green"),
        ];
        let mut table = View::new("All tasks", ViewKind::Table);
        table.widths.insert("Name".into(), 280.0);
        let mut board = View::new("Board", ViewKind::Board);
        board.group_by = Some("Status".into());
        board.hidden = vec!["Done".into(), "Estimate".into()];
        let mut calendar = View::new("Calendar", ViewKind::Calendar);
        calendar.date_prop = Some("Due".into());
        let mut list = View::new("To do", ViewKind::List);
        list.filters.push(Filter {
            prop: "Status".into(),
            op: FilterOp::IsNot,
            value: "Done".into(),
        });
        list.sorts.push(Sort {
            prop: "Due".into(),
            desc: false,
        });
        list.hidden = vec!["Done".into(), "Estimate".into(), "Tags".into()];
        let mut gallery = View::new("Gallery", ViewKind::Gallery);
        gallery.hidden = vec!["Done".into(), "Estimate".into()];
        p.database = Some(Database {
            schema: vec![
                Property::new("Name", PropKind::Title),
                status,
                priority,
                Property::new("Due", PropKind::Date),
                tags,
                Property::new("Estimate", PropKind::Number),
                Property::new("Done", PropKind::Checkbox),
            ],
            views: vec![table, board, calendar, list, gallery],
        });
        p.blocks = vec![Block::paragraph("")];
    }

    let day = |offset: i64| date_to_string(today() + chrono::Duration::days(offset));
    let rows: Vec<(&str, &str, &str, &str, i64, Vec<&str>, f64, &str)> = vec![
        (
            "整理课程笔记",
            "📝",
            "In progress",
            "High",
            1,
            vec!["学习"],
            2.0,
            "## 要点\n\n- 把每节课的重点整理成一页\n- 标出还没弄懂的地方\n\n- [ ] 第 1–3 讲\n- [ ] 第 4–6 讲\n",
        ),
        (
            "准备小组展示",
            "🎤",
            "Not started",
            "High",
            4,
            vec!["学习", "工作"],
            3.0,
            "和组员确认分工，然后做幻灯片。\n",
        ),
        (
            "更新简历",
            "📄",
            "Not started",
            "Medium",
            9,
            vec!["工作"],
            1.5,
            "",
        ),
        (
            "10 公里训练",
            "🏃",
            "In progress",
            "Medium",
            2,
            vec!["健身"],
            1.0,
            "> 慢跑为主，注意拉伸。\n",
        ),
        (
            "读完一本书",
            "📚",
            "Done",
            "Low",
            -3,
            vec!["生活"],
            5.0,
            "",
        ),
        (
            "周末大扫除",
            "🧹",
            "Done",
            "Low",
            -1,
            vec!["生活"],
            2.0,
            "",
        ),
    ];
    for (title, icon, status, priority, due, tags, estimate, body) in rows {
        let id = vault.create_page(PageKind::Page, Some(db.clone()));
        let p = vault.page_mut(&id).unwrap();
        p.title = title.into();
        p.icon = Some(icon.into());
        p.props
            .insert("Status".into(), PropValue::Text(status.into()));
        p.props
            .insert("Priority".into(), PropValue::Text(priority.into()));
        p.props.insert("Due".into(), PropValue::Text(day(due)));
        p.props.insert(
            "Tags".into(),
            PropValue::List(tags.into_iter().map(String::from).collect()),
        );
        p.props
            .insert("Estimate".into(), PropValue::Number(estimate));
        p.props
            .insert("Done".into(), PropValue::Bool(status == "Done"));
        let blocks = from_markdown(body);
        if !blocks.is_empty() {
            p.blocks = blocks;
        }
    }

    // --- A plain notes page ----------------------------------------------
    let notes = vault.create_page(PageKind::Page, None);
    {
        let p = vault.page_mut(&notes).unwrap();
        p.title = "读书笔记".into();
        p.icon = Some("📖".into());
        p.blocks = from_markdown(
            r#"# 《深度工作》

> 高质量的产出 = 时间 × 专注度

## 摘录

1. 深度工作是一种越来越稀缺、也越来越有价值的能力。
2. 把浮浅的事务集中处理，给深度工作留出整块时间。

## 我的想法

- 每天固定两个小时不看消息
- 给每周做一次复盘
"#,
        );
    }

    vault.save_all()?;
    Ok(())
}
