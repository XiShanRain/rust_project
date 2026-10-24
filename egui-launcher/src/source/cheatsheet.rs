//! FR-SRC-18 速查表：`?` 列出全部语法与快捷键，`? 词` 过滤，`Enter` 打开帮助页。
//!
//! 为什么是一张手写静态表，而不是从各源反射生成：速查表要写的是「用户得背下来的写法」，
//! 它与实现不是一一对应（文件模式一个源就五式触发 + 四条快捷键，各自挂在不同的分支上），
//! 从代码抽反而会漏。正确性靠三类单测钉住（见文件末尾）：
//! 1. 带 `source` 的条目在该源关掉时必须消失（需求书「按功能启用状态过滤」）；
//! 2. 示例写法全表不重样（用户靠它认出"这是同一条"）；
//! 3. `? 文件` / `? 快捷键` 这类过滤词要真能过滤到东西。
//!
//! 帮助页是本程序内的一个模态（`main.rs`），不是外部文档：便携版拷到别的机器上没有 `docs/`。

use super::{DataSource, SearchCtx, SourceId, SourceRow};

/// 速查表条目按这四组排列（也是界面上的分组标记）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Group {
    Syntax,
    Files,
    Keys,
    Window,
}

impl Group {
    pub fn label(self) -> &'static str {
        match self {
            Self::Syntax => "语法",
            Self::Files => "文件",
            Self::Keys => "按键",
            Self::Window => "窗口",
        }
    }

    /// 过滤用别名。放在组上而不是逐条 `tags` 里：`? 快捷键` 要一次叫出整组，
    /// 逐条复写十八遍只会让"改一条漏一条"有机会发生。
    fn aliases(self) -> &'static str {
        match self {
            Self::Syntax => "syntax 语法",
            Self::Files => "files file 文件",
            Self::Keys => "shortcut key 快捷键 按键",
            Self::Window => "window mouse 窗口 鼠标",
        }
    }
}

/// 速查表的一条。字段全是 `'static`：表在编译期就定死，单测与界面都只读它。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CheatEntry {
    /// 示例写法（列表里的大字，等宽显示）。
    pub key: &'static str,
    /// 这一条叫什么（帮助页标题）。
    pub title: &'static str,
    /// 列表行里的那句话。
    pub summary: &'static str,
    /// 帮助页正文，`\n` 分段。
    pub detail: &'static str,
    /// 过滤用同义词（`? file`、`? 快捷键` 靠它命中）。不参与界面显示。
    pub tags: &'static str,
    pub group: Group,
    /// 依赖哪个数据源：该源关掉时这一条整条不出现。`None` ＝ 与数据源无关。
    pub source: Option<SourceId>,
    /// 帮助页上「打开设置页」跳到第几页（1=通用 2=功能 3=热键 4=数据源 5=外观）。
    pub settings_page: Option<u32>,
}

pub const ENTRIES: &[CheatEntry] = &[
    // ---------- 语法（按功能分，文件源与终端各自成块） ----------
    CheatEntry {
        key: "?",
        title: "速查表（本页）",
        summary: "`? 词` 只看包含该词的条目",
        detail: "敲 `?` 列出全部语法与快捷键；`? 文件`、`? 快捷键`、`? 终端` 只留相关的条目。\n\
                 ↑↓ 选一条，Enter 或点击打开它的帮助页；Esc 关闭。\n\
                 只认查询串第 0 个字符——前面带空格的 ` ?` 不算。本页只列当前开着的数据源的语法。",
        tags: "help cheatsheet 速查 帮助 语法 syntax",
        group: Group::Syntax,
        source: Some(SourceId::Cheatsheet),
        settings_page: None,
    },
    CheatEntry {
        key: "g 天气",
        title: "网页搜索",
        summary: "`引擎关键字 词` 独占整张列表",
        detail: "出厂九条：`g` Google、`bing` Bing、`bd` 百度、`gh` GitHub、`wiki` 维基、\
                 `yt` YouTube、`tb` 淘宝、`jd` 京东、`tr` Google 翻译。\n\
                 引擎表写在 settings.json 的 `web_engines`（含网址模板，`{query}` 会被替换）。\n\
                 关键字打全、还没跟空格时仍是普通搜索；跟上空格与词才独占。一条结果都没有时按 google → bing 兜底。",
        tags: "web search 搜索 网页 引擎 google 百度 github",
        group: Group::Syntax,
        source: Some(SourceId::Web),
        settings_page: Some(4),
    },
    CheatEntry {
        key: "关键字 参数",
        title: "自定义命令（需参数）",
        summary: "清单里标了「需参数」的命令",
        detail: "`关键字 参数` 独占整张列表，命令行里的 `{query}` 换成参数。\n\
                 关键字打全但还缺参数时，头段先给一行提示；Enter 补成「关键字 + 空格」。\n\
                 参数就是查询串的一部分：日志只记命令名，不记参数。",
        tags: "command 自定义 命令 参数 parameter",
        group: Group::Syntax,
        source: Some(SourceId::Custom),
        settings_page: Some(4),
    },
    CheatEntry {
        key: "{query}",
        title: "参数占位符",
        summary: "自定义命令命令行里的那一格",
        detail: "命令行写成 `ping {query}`，输入 `ping example.com` 时整段替换；可以出现多次。\n\
                 不带 `{query}` 的命令收到多余参数时按它原来的参数跑。\n\
                 改命令：选中清单里那条按 Ctrl+E。",
        tags: "placeholder query 占位符 参数",
        group: Group::Syntax,
        source: Some(SourceId::Custom),
        settings_page: None,
    },
    CheatEntry {
        key: "1+2",
        title: "计算器",
        summary: "能求成一个数时头段加一行",
        detail: "四则、括号与乘方（`^`），可带单位。前导 `=` 是「这是一道算式」的显式写法，可写可不写。\n\
                 整串只是一个数字时不出行（否则第 0 行永远是计算器，数字键执行会集体变成复制数字）。\n\
                 Enter 只把纯数字放进剪贴板（不带千分位、不带单位）。",
        tags: "calc 计算器 算式 数学 加减乘除",
        group: Group::Syntax,
        source: Some(SourceId::Calc),
        settings_page: Some(4),
    },
    CheatEntry {
        key: "10 km to mile",
        title: "单位换算",
        summary: "`量 in|to 单位`，七类量纲",
        detail: "长度、面积、体积、质量、温度、速度、时间、数据八类（温度用 `100C to F` 这种写法）。\n\
                 也可以直接算：`2 h + 30 min`、`1 day + 7`。\n\
                 裸数字沿用左侧单位：`1 day + 7` 是 8 天，不是 8 天 7 秒。换算结果的副标题会写明方向。",
        tags: "unit 单位 换算 convert km mile 温度 长度 重量",
        group: Group::Syntax,
        source: Some(SourceId::Calc),
        settings_page: None,
    },
    CheatEntry {
        key: "0x1F",
        title: "进制互转与日期运算",
        summary: "`0x` / `0b` / `0o`；`today+3`",
        detail: "整串只是一个进制字面量（`0x1F`、`0b1010`、`0o17`）时，副标题给出其它进制的值。\n\
                 日期词：`today` / `now` / `tomorrow` / `yesterday`；`today + 7 days` 得日期，\
                 `2026-12-25 - today` 得相差天数。",
        tags: "hex binary 进制 日期 date today 十六进制",
        group: Group::Syntax,
        source: Some(SourceId::Calc),
        settings_page: None,
    },
    CheatEntry {
        key: "jsb",
        title: "清单匹配：拼音 / 驼峰 / 多词",
        summary: "中文名吃拼音首字母，英文名吃驼峰首字母",
        detail: "拼音首字母：`jsb` 命中「记事本」（应用、自定义命令这类中文标题才有）。\n\
                 驼峰/词首字母：`vsc` 命中 Visual Studio Code。\n\
                 多词查询按空格切开，每个词都要命中才算一条（`报告 2026`）；词序不限。",
        tags: "pinyin 拼音 首字母 驼峰 camel 多词 匹配",
        group: Group::Syntax,
        source: None,
        settings_page: None,
    },
    // ---------- 文件模式（FR-SRC-14，六式） ----------
    CheatEntry {
        key: "D:\\proj\\",
        title: "文件模式第一式：直接打路径",
        summary: "尾部带分隔符＝列整个目录",
        detail: "打绝对路径（`D:\\proj\\`、`\\\\srv\\share\\`）就是浏览那一层；尾部带分隔符列整个目录，\
                 不带的话后面还能接过滤词（`D:\\proj\\rep`）。\n\
                 不打索引、直接读盘；路径不存在就列空清单，不会退回普通搜索。",
        tags: "file path 路径 浏览 目录 browse",
        group: Group::Files,
        source: Some(SourceId::Files),
        settings_page: Some(4),
    },
    CheatEntry {
        key: "'报告",
        title: "文件模式第二式：按文件名搜",
        summary: "在 桌面/下载/文档 的索引里找",
        detail: "撇号后直接跟词。撇号可以是半角 `'`、弯引号 `‘`/`’`（中文输入法常打出这个）或全角 `＇`。\n\
                 索引范围：桌面、下载、文档，深度四级以内，3 万条封顶；每 30 分钟重扫一轮。\n\
                 结果最多 30 条。Enter 用系统关联程序打开；选中文件夹行时 Tab 进入该目录。",
        tags: "file 文件 文件名 搜索 撇号 apostrophe",
        group: Group::Files,
        source: Some(SourceId::Files),
        settings_page: Some(4),
    },
    CheatEntry {
        key: "（空格开头）报告",
        title: "文件模式第六式：空格开头",
        summary: "前导一个空格就等于进文件模式",
        detail: "查询串以空格开头时整串交给文件搜索，与 `'词` 同一结果。\n\
                 空格怎么打进来：设置页「通用 → 空格直接执行」不是「始终执行」那两档时，\
                 Shift+空格 恒为插入一个真空格；「始终执行」那档下空格键被执行占用，\n\
                 就只剩 Shift+空格 这一条路。",
        tags: "file space 空格 文件模式",
        group: Group::Files,
        source: Some(SourceId::Files),
        settings_page: Some(1),
    },
    CheatEntry {
        key: "open 报告",
        title: "文件模式第三式：动词触发",
        summary: "`open` / `find` / `file` 后接词",
        detail: "三个动词等价，与 `'报告` 出同一份结果。\n\
                 动词本身也是文件名的一部分时会被当成词——多打一个词就切回普通查询。",
        tags: "file open find 文件 动词",
        group: Group::Files,
        source: Some(SourceId::Files),
        settings_page: None,
    },
    CheatEntry {
        key: "pic 报告",
        title: "文件模式第四式：类型词",
        summary: "doc / pic / video / audio / zip / exe / cad",
        detail: "七类扩展名：`doc`（文档与表格）、`pic`（图片）、`video`、`audio`、`zip`（压缩包）、\
                 `exe`（可执行与脚本）、`cad`（图纸与模型）。\n\
                 目录不参与类型筛选。`ext=pdf` 这种任意扩展名筛选没实现（需求书里的偏差，已登记）。",
        tags: "file type 类型 扩展名 doc pic video audio zip exe cad",
        group: Group::Files,
        source: Some(SourceId::Files),
        settings_page: None,
    },
    CheatEntry {
        key: "folder 项目",
        title: "文件模式第五式：只找文件夹",
        summary: "`folder` 后接词",
        detail: "只列文件夹，不列文件。与类型词一样，`folder` 后面还得跟至少一个词才进文件模式\
                 ——只打 `folder` 时它可能还是某个程序名的开头。",
        tags: "folder 文件夹 目录",
        group: Group::Files,
        source: Some(SourceId::Files),
        settings_page: None,
    },
    // ---------- 终端（FR-SRC-16） ----------
    CheatEntry {
        key: ">ipconfig /all",
        title: "终端命令",
        summary: "在新终端窗口里执行，执行完停在窗口里",
        detail: "`>` 后直接跟命令（不需要空格；`> ipconfig` 也行），在新控制台窗口里跑，\
                 带 `/k` 或 `-NoExit` ⇒ 输出留在窗口里。\n\
                 只敲 `>`（或 `> `）只给一行用法，不会执行任何东西。\n\
                 用哪个 shell 在设置页「功能 → 终端 shell」四选一：cmd / powershell / pwsh / Windows Terminal。\n\
                 命令原文不落日志。",
        tags: "terminal 终端 shell 命令 cmd powershell",
        group: Group::Syntax,
        source: Some(SourceId::Terminal),
        settings_page: Some(2),
    },
    // ---------- 按键 ----------
    CheatEntry {
        key: "↑ / ↓",
        title: "选择条目",
        summary: "在头段行与结果之间移动选中",
        detail: "↑↓ 只移动选中，不执行。到顶/到底不循环。",
        tags: "key 选择 上下 arrow",
        group: Group::Keys,
        source: None,
        settings_page: None,
    },
    CheatEntry {
        key: "PgUp / PgDn",
        title: "翻页",
        summary: "结果列表翻一页，选中项跟着滚",
        detail: "翻页后选中项仍保持在可视区里（列表会滚动跟随）。",
        tags: "key 翻页 page",
        group: Group::Keys,
        source: None,
        settings_page: None,
    },
    CheatEntry {
        key: "1 … 9 / 0",
        title: "按行号执行",
        summary: "执行前 10 行；裸键仅输入为空时生效",
        detail: "列表前 10 行在行首标了号，按对应数字直接执行那一行。\n\
                 裸数字只在搜索框为空时生效（有字时数字是查询内容）；Ctrl+数字 / Alt+数字 不受这个限制。\n\
                 指到不可执行的行（计算器行→复制、提示行→Toast）时按该行的语义走。",
        tags: "key 数字 行号 digit 执行",
        group: Group::Keys,
        source: None,
        settings_page: None,
    },
    CheatEntry {
        key: "Tab",
        title: "补全 / 下一项",
        summary: "按设置页「Tab 键行为」切换口径",
        detail: "补全优先模式下，只有「能被这个查询补长的候选恰好一条」才补全，否则当作下一项。\n\
                 选中的是文件夹行时恒为进入该目录（不被 Tab 模式影响）。\n\
                 补全后光标停在查询串末尾，接着打字不用先点一下。",
        tags: "key tab 补全 下一项 complete",
        group: Group::Keys,
        source: None,
        settings_page: Some(1),
    },
    CheatEntry {
        key: "Shift+Tab",
        title: "上一项",
        summary: "两种 Tab 模式一致",
        detail: "与 Ctrl+P 同义：循环上一项，不会被补全抢走。",
        tags: "key shift tab 上一项",
        group: Group::Keys,
        source: None,
        settings_page: None,
    },
    CheatEntry {
        key: "Ctrl+N / Ctrl+P",
        title: "下一项 / 上一项",
        summary: "恒为导航；Ctrl+J / Ctrl+K 是另一对",
        detail: "这四个键永远是导航，不管 Tab 模式与补全结果如何。\n\
                 老习惯（Ctrl+J / Ctrl+K）保留着，与它俩等价。",
        tags: "key ctrl 导航 下一项 上一项 navigate",
        group: Group::Keys,
        source: None,
        settings_page: None,
    },
    CheatEntry {
        key: "Space",
        title: "执行选中项",
        summary: "四档：关 / 打全名·别名 / 唯一候选 / 始终",
        detail: "设置页「通用 → 空格直接执行」四选一，默认「打全名·别名」：输入为空，或整串正好等于选中项的名称/别名时才执行。\n\
                 打多词查询时想插空格：Shift+空格 恒为插入一个真空格（「始终执行」那档下的唯一出口）。\n\
                 选中计算器行 / 搜索行时空格不接管（它们只认 Enter、点击与数字键）。",
        tags: "key space 空格 执行",
        group: Group::Keys,
        source: None,
        settings_page: Some(1),
    },
    CheatEntry {
        key: "Enter",
        title: "运行选中项",
        summary: "无匹配时弹「新增」框",
        detail: "运行当前选中项；模态开着时 Enter 是那个模态的「确定」。\n\
                 一条结果都没有时 Enter 不直接跑命令，而是弹出预填了当前输入的「新增」框（FR-SRC-23 修订口径）。",
        tags: "key enter 运行 回车",
        group: Group::Keys,
        source: None,
        settings_page: None,
    },
    CheatEntry {
        key: "Esc",
        title: "分段返回",
        summary: "按序：收菜单 → 关框 → 清空 → 关闭到托盘",
        detail: "顺序：收 ☰ 菜单 → 关确认框 → 关弹窗/帮助页 → 设置页返回搜索 → 清空输入 → 关闭到托盘。\n\
                 清空那一档会主动把焦点要回搜索框（TextEdit 自己按 Esc 会弃焦点）。",
        tags: "key esc 取消 关闭 返回",
        group: Group::Keys,
        source: None,
        settings_page: None,
    },
    CheatEntry {
        key: "Ctrl+Shift+N",
        title: "新建快捷项",
        summary: "选中的是文件行时预填这一行",
        detail: "打开「新增」框；当前选中的是文件搜索行时，把这一行的路径与类型预填进去。\n\
                 当前输入非空时预填当前输入（与无匹配时 Enter 同一个入口）。",
        tags: "key 新建 添加 add new",
        group: Group::Keys,
        source: None,
        settings_page: None,
    },
    CheatEntry {
        key: "Ctrl+E",
        title: "编辑选中项",
        summary: "改名称 / 关键字 / 命令行",
        detail: "编辑清单里的那一条。文件搜索行不在清单里，对它不适用（它对「添加为快捷项」有意义）。\n\
                 内置命令（系统命令分组）的类型是只读的，改不了运行方式。",
        tags: "key 编辑 edit",
        group: Group::Keys,
        source: None,
        settings_page: None,
    },
    CheatEntry {
        key: "Delete / Ctrl+Shift+D",
        title: "删除选中项",
        summary: "都弹确认框；Delete 仅输入为空时生效",
        detail: "从清单移除，不删磁盘上的文件。两个键语义一样，只是可用条件不同：\n\
                 Delete 只在搜索框为空时生效（有字时删除键要删字）；Ctrl+Shift+D 不管有没有字都能用。",
        tags: "key 删除 delete 移除",
        group: Group::Keys,
        source: None,
        settings_page: None,
    },
    CheatEntry {
        key: "Ctrl+D",
        title: "打开所在目录",
        summary: "选中清单项＝开它所在目录；文件行＝定位它",
        detail: "清单项：用资源管理器打开它所在目录。\n\
                 文件搜索行：在资源管理器里定位并选中这个文件。\n\
                 右键菜单里也有同一件事（文件行）。",
        tags: "key 目录 explorer 资源管理器 定位",
        group: Group::Keys,
        source: None,
        settings_page: None,
    },
    CheatEntry {
        key: "Ctrl+Shift+P",
        title: "置顶 / 取消置顶",
        summary: "置顶项名称前带 📌",
        detail: "切换选中项的置顶状态，并给一句 Toast 回执，立即落盘。\n\
                 置顶只压过「同一匹配档位内」的次序，不会让更精确的命中让位。\n\
                 右键菜单里也有同一件事。",
        tags: "key 置顶 pin 取消",
        group: Group::Keys,
        source: None,
        settings_page: None,
    },
    CheatEntry {
        key: "Ctrl+F",
        title: "进入文件模式",
        summary: "在输入前补一个 `'` 触发符",
        detail: "已经在文件模式时只给一句提示，不会叠第二个前缀。",
        tags: "key file 文件模式",
        group: Group::Keys,
        source: None,
        settings_page: None,
    },
    CheatEntry {
        key: "Ctrl+I",
        title: "批量导入全部",
        summary: "后台扫开始菜单，结果合并进清单",
        detail: "一次性把开始菜单里的快捷方式并入清单（按 uid 去重，已有的项不重复添加）。\n\
                 托盘菜单里也有同一件事。",
        tags: "key 导入 import 批量",
        group: Group::Keys,
        source: None,
        settings_page: None,
    },
    CheatEntry {
        key: "Ctrl+,",
        title: "打开 / 返回设置页",
        summary: "五页：通用 / 功能 / 热键 / 数据源 / 外观",
        detail: "打开设置页；已开着时按它返回搜索页（与 Esc 的「设置页返回搜索」同一档）。",
        tags: "key 设置 settings preferences",
        group: Group::Keys,
        source: None,
        settings_page: None,
    },
    CheatEntry {
        key: "Ctrl+M / ☰",
        title: "主菜单",
        summary: "新建、批量导入、设置、速查表、打开日志、退出",
        detail: "顶栏右上的三条杠。批次 F6（2026-10-24）用它换掉了输入框旁那个「＋ 新建」按钮，\n\
                 菜单里第一项就是新建；Ctrl+Shift+N 那条路照旧留着。\n\
                 Esc 只收菜单（不清空输入、不关窗）；点菜单外任意处也收。\n\
                 窗口一隐藏，菜单状态不保留。",
        tags: "key 菜单 menu 新建 设置 hamburger",
        group: Group::Keys,
        source: None,
        settings_page: None,
    },
    CheatEntry {
        key: "呼出键 / 补字符键",
        title: "全局热键",
        summary: "出厂 Ctrl+Alt+Space；两者都可在设置页改",
        detail: "呼出键在设置页「热键」页录制，出厂是 Ctrl+Alt+Space。\n\
                 呼出键设成裸键（如 `` ` ``）时，那一个字符在全系统就打不出来了 ⇒ 同一页给它配一个\
                 「补字符键」把字符打回焦点窗口；呼出键带修饰时补字符键按了什么都不做。\n\
                 热键被别的程序占用时注册会失败，设置页会留一条说明。",
        tags: "key hotkey 热键 呼出 全局 typeback",
        group: Group::Keys,
        source: None,
        settings_page: Some(3),
    },
    // ---------- 窗口 ----------
    CheatEntry {
        key: "左键拖空白",
        title: "移动窗口",
        summary: "顶栏空白或列表右侧空白",
        detail: "顶栏搜索框与 ☰ 之间那条约 20px 的空白、设置页标题右侧的空白、列表右侧的空白都能拖。\n\
                 中键不再执行任何条目（2026-10-18 已废弃）。",
        tags: "mouse 拖动 移动 窗口 drag",
        group: Group::Window,
        source: None,
        settings_page: None,
    },
    CheatEntry {
        key: "拖边缘 / Ctrl+= / Ctrl+-",
        title: "缩放窗口",
        summary: "四边四角各 6px 隐形手柄；键盘一档 60×40",
        detail: "鼠标：左键拖四边或四角（各 6px 隐形手柄），最小 360×240。\n\
                 键盘：Ctrl+= 放大、Ctrl+- 缩小，每档 60×40 逻辑像素，上限 1200×900。\n\
                 窗口大小只记在本次运行里，退出程序就回到默认尺寸。",
        tags: "mouse 缩放 改变大小 resize zoom",
        group: Group::Window,
        source: None,
        settings_page: None,
    },
    CheatEntry {
        key: "列表行密度",
        title: "一行的松紧与窗口高度",
        summary: "甲·紧凑（默认）/ 乙·宽松 / 丙·极紧 / 跟随主题",
        detail: "设置页「通用 → 列表行密度」（批次 F6，2026-10-24 用户：「整体界面高度太高了，\n\
                 项与项之间的间距太大了」，并点名把三档做成可选项、默认甲）。\n\
                 甲＝行高 34 / 图标 24 / 行距 2 ⇒ 窗口高 438；乙＝40 / 28 / 2 ⇒ 498；\n\
                 丙＝24 / 16 / 0 ⇒ 318；跟随主题＝回到主题 JSON 的 row_height 与 icon_size。\n\
                 四档都保持「一屏正好十行」：换档时窗口高度按新行高立刻重算，宽度不动。\n\
                 不在「跟随主题」档时，主题的这两个键不参与渲染（其余键照常生效）。\n\
                 上面那几个总高是按 Light 的内边距 14 算的，换主题只差内边距那几个像素。\n\
                 输入框那一行不受行密度影响：高度固定 44，字号仍跟着主题的「输入字号」走。",
        tags: "行高 图标 密度 高度 间距 列表 density row theme spacing",
        group: Group::Window,
        source: None,
        settings_page: Some(1),
    },
];

/// 查询串 → 过滤词。只认第 0 个字符：` ?` 归普通搜索（与 `>` 同一口径）。
pub fn route(raw: &str) -> Option<&str> {
    Some(raw.strip_prefix('?')?.trim())
}

/// 过滤词是否命中这一条。空词命中全部；只搜示例写法 / 名字 / 一句话 / 同义词，不搜正文——
/// 正文里什么都可能提到，拿它过滤会让 `? 文件` 把整表都带出来。
pub fn matches(entry: &CheatEntry, needle: &str) -> bool {
    if needle.is_empty() {
        return true;
    }
    let needle = needle.to_lowercase();
    [entry.key, entry.title, entry.summary, entry.tags, entry.group.aliases()]
        .iter()
        .any(|s| s.to_lowercase().contains(&needle))
}

/// 本帧该列哪些条目：所属源关掉的不列，过滤词不命中的不列。顺序照表。
pub fn entries(needle: &str, on: &dyn Fn(SourceId) -> bool) -> Vec<&'static CheatEntry> {
    ENTRIES
        .iter()
        .filter(|e| e.source.map_or(true, |id| on(id)) && matches(e, needle))
        .collect()
}

/// 速查表里的一行。`entry` 是表里的静态引用 ⇒ 进帮助页不需要再查一遍。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CheatHit {
    pub entry: &'static CheatEntry,
}

/// 过滤词一条也没命中时的提示行。不可执行，只给一句提示。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheatHint {
    pub title: String,
    pub subtitle: &'static str,
}

pub const NO_MATCH_TOAST: &str = "换个词试试：? 文件 / ? 快捷键 / ? 终端";

impl CheatHint {
    pub fn empty(filter: &str) -> Self {
        // 过滤词就是查询串的一部分：它只进界面，不进日志。
        Self {
            title: format!("没有匹配「{filter}」的条目"),
            subtitle: "换个词试试：? 文件 / ? 快捷键 / ? 终端 / ? 计算器",
        }
    }
}

pub struct CheatSource;

impl DataSource for CheatSource {
    fn id(&self) -> SourceId {
        SourceId::Cheatsheet
    }

    fn label(&self) -> &'static str {
        "速查表"
    }

    fn note(&self) -> &'static str {
        "`?` 列出全部语法与快捷键（`? 文件` 过滤），`Enter` 打开帮助页。表里只列此刻开着的数据源的语法；关掉之后 `?` 回到普通搜索。"
    }

    fn take_over(&mut self, cx: &mut SearchCtx<'_>) -> Option<Vec<SourceRow>> {
        let needle = route(cx.query)?;
        let hits = entries(needle, cx.source_on);
        if hits.is_empty() {
            return Some(vec![SourceRow::CheatHint(CheatHint::empty(needle))]);
        }
        Some(hits.into_iter().map(|entry| SourceRow::Cheat(CheatHit { entry })).collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn all_on(_: SourceId) -> bool {
        true
    }

    #[test]
    fn route_only_reads_position_zero() {
        assert_eq!(route("?"), Some(""));
        assert_eq!(route("?   "), Some(""), "尾随空白不算过滤词");
        assert_eq!(route("? file"), Some("file"));
        assert_eq!(route("?文件"), Some("文件"), "不需要空格");
        assert_eq!(route(" ?文件"), None, "前导空格归文件模式，不抢");
        assert_eq!(route("文件?"), None);
        assert_eq!(route(""), None);
    }

    #[test]
    fn every_entry_has_a_unique_key_and_a_body() {
        let mut keys: Vec<&str> = ENTRIES.iter().map(|e| e.key).collect();
        let n = keys.len();
        keys.sort_unstable();
        keys.dedup();
        assert_eq!(keys.len(), n, "示例写法重样了，用户分不清是哪一条");
        for e in ENTRIES {
            assert!(!e.title.is_empty() && !e.summary.is_empty() && !e.detail.is_empty());
            assert!(!e.tags.is_empty(), "「{}」没有过滤词，`? 关键词` 搜不到它", e.title);
            if let Some(page) = e.settings_page {
                // 批次 F1 加了第 5 页「外观」，上界跟着走（现量：SettingsTab::from_page 认 1..=5）。
                assert!((1..=5).contains(&page), "「{}」指向不存在的设置页 {page}", e.title);
            }
            // 列表与帮助页都是纯 `ui.label`，不解析 Markdown：`**粗体**` 会原样带着星号出场
            //（2026-10-23 真机截图里就是这么露的）。
            for text in [e.key, e.title, e.summary, e.detail, e.tags] {
                assert!(!text.contains("**"), "「{}」的文案里有 Markdown 粗体标记：{text}", e.title);
            }
        }
    }

    #[test]
    fn filters_hit_what_the_acceptance_criteria_name() {
        // 需求书验收原话：「敲 `?` 能看到"空格=文件模式、`>`=终端"等条目」。
        let all = entries("", &all_on);
        assert!(all.iter().any(|e| e.key.contains('>')), "`?` 里看不到终端条目");
        assert!(all.iter().any(|e| e.key.contains("空格")), "`?` 里看不到空格开头的文件模式");
        for needle in ["file", "文件", "快捷键", "终端", "计算器"] {
            assert!(entries(needle, &all_on).iter().all(|e| matches(e, needle)));
            assert!(!entries(needle, &all_on).is_empty(), "过滤词「{needle}」一条也命不中");
        }
        assert!(entries("file", &all_on).len() < all.len());
    }

    #[test]
    fn a_disabled_source_takes_its_entries_with_it() {
        // 关掉文件源 ⇒ 六式全消失；关掉终端 ⇒ `>` 那条消失；与源无关的快捷键照留。
        let off = |id: SourceId| id != SourceId::Files && id != SourceId::Terminal;
        let keys = entries("", &off);
        assert!(!keys.iter().any(|e| e.source == Some(SourceId::Files)));
        assert!(!keys.iter().any(|e| e.source == Some(SourceId::Terminal)));
        assert!(keys.iter().any(|e| e.group == Group::Keys));
        // 过滤词只命中被关掉的源 ⇒ 给的是空列表（由源那边转成提示行），不是全表。
        assert!(entries("folder", &off).is_empty());
    }

    #[test]
    fn the_source_takes_over_and_bare_question_mark_lists_everything() {
        let items: Vec<super::super::application::LaunchItem> = Vec::new();
        let mut index = crate::matching::MatchIndex::default();
        let mut src = CheatSource;
        let mut cx = SearchCtx {
            query: "?",
            items: &items,
            engines: &[],
            learn_rank: super::super::application::LearnRank::Count,
            index: &mut index,
            now_ms: 0,
            term_shell: super::super::terminal::TermShell::default(),
            source_on: &all_on,
        };
        let rows = src.take_over(&mut cx).expect("`?` 必须独占");
        assert_eq!(rows.len(), entries("", &all_on).len());
        assert!(rows.iter().all(|r| matches!(r, SourceRow::Cheat(_))));

        // 不认识的过滤词 → 一行提示（不可执行）。
        let mut index2 = crate::matching::MatchIndex::default();
        let mut cx2 = SearchCtx { query: "?没有这种条目", index: &mut index2, ..cx };
        let rows = src.take_over(&mut cx2).expect("过滤词为空也不是普通搜索");
        assert!(matches!(rows.as_slice(), [SourceRow::CheatHint(_)]));

        // 不是第 0 个字符 → 不参与（`x?` 归普通搜索）。
        let mut index3 = crate::matching::MatchIndex::default();
        let mut cx3 = SearchCtx { query: "x?", index: &mut index3, ..cx2 };
        assert!(src.take_over(&mut cx3).is_none());
    }
}
