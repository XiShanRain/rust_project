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
    /// 帮助页上「打开设置页」跳到第几页（1=通用 2=功能 3=热键 4=数据源 5=文件搜索 6=外观 7=高级
    /// 8=批量管理 9=使用统计 10=网页引擎）。
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
                 引擎表在设置页「网页引擎」（第 10 页，`Ctrl+W`）里增删改，也仍可直接改 settings.json 的\n\
                 `web_engines`（含网址模板，`{query}` 会被替换）。\n\
                 关键字打全、还没跟空格时仍是普通搜索；跟上空格与词才独占。一条结果都没有时按兜底序列出行（出厂 google → bing，可改）。",
        tags: "web search 搜索 网页 引擎 google 百度 github",
        group: Group::Syntax,
        source: Some(SourceId::Web),
        settings_page: Some(10),
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
        key: "{%p} {%c} {%wt}",
        title: "参数标记",
        summary: "命令行 / 参数里的替身，呼出那一刻取值；编码不是「无参数」才替换",
        detail: "五个标记：`{%p}` 你填的参数 · `{%c}` 呼出那一刻的剪贴板文本 ·\
                 `{%wd}` 前台窗口句柄（十进制）· `{%wt}` 它的标题 · `{%wc}` 它的窗口类名。\n\
                 只在编辑弹窗的「编码」不是「无参数」时才替换；编码＝无参数时它们原样交给程序。\n\
                 带标记却留着「无参数」的，保存时会追问一次。\n\
                 四档编码：无参数＝直接启动；参数不编码＝原样代入；URL 编码＝空格变 `+`、中文按 UTF-8 百分号；\
                 UTF-8 编码与 URL 编码同口径（留着只为认得 Delphi 的旧配置）。\n\
                 编码不是「无参数」的条目，列表行名字前有一个 `*`。\n\
                 窗口那三个取的是呼出之前的前台窗口，不是本程序；弹窗里的「测试执行」把 `{%p}` 当空值、\
                 其余标记按当前现场展开。",
        tags: "param 参数 标记 marker p c wd wt wc 编码 剪贴板 窗口 替身",
        group: Group::Syntax,
        source: None,
        settings_page: None,
    },
    CheatEntry {
        key: "@+ @- @",
        title: "运行标志",
        summary: "命令行首/尾加一个标志，改目标窗口打开时的状态",
        detail: "`@+` 最大化 · `@-` 最小化 · `@` 隐藏（后台起、不弹窗）。\n\
                 写在命令行「尾」（`notepad.exe@`，需求书原文）或「首」（`@notepad.exe`，Delphi 写法）都认；\
                 一条命令只认一个，两头都写着时前导那个说了算。\n\
                 只在本程序启动这一条路上生效；网址不认（浏览器自己开窗）。\n\
                 目标程序自己不认系统给的初始显示状态时这一档没效果——记事本、cmd 这类认。",
        tags: "run flag 运行标志 最大化 最小化 隐藏 后台 @+ @-",
        group: Group::Syntax,
        source: None,
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
    CheatEntry {
        key: "*calc",
        title: "通配符 / 正则",
        summary: "`*` 任意串、`?` 单个字；其余字符按正则解释",
        detail: "查询里出现 `*` 或 `?` 时整串当成一个正则：`*` → `.*`、`?` → `.`，\
                 其余字符原样交给正则引擎（所以 `^` `$` `\\d` `[a-z]` 也能用）。\n\
                 例：`*calc` 找名字里含 calc 的；`c?lc` 要求中间恰好一个字；`*.exe` 只看可执行文件。\n\
                 一律按小写比较（Delphi 版同一口径），因此 `\\D` 这类大写类转义会变成 `\\d`。\n\
                 正则写错（如 `*[`）不报错也不中断，整轮退回普通匹配。\n\
                 通配符查询不再走拼音首字母 / 子序列那几档，也不拆多词——整串就是一个模式。\n\
                 注意 `?` 放在第 0 个字符是「速查表」的入口，要匹配单字请把它放到后面（`c?lc`）。",
        tags: "wildcard regex 通配符 正则 星号 问号 模式 pattern",
        group: Group::Syntax,
        source: None,
        settings_page: None,
    },
    CheatEntry {
        key: "@维护 磁盘",
        title: "分类筛选",
        summary: "`@分类 词` 只列这一类；多个分类取并集",
        detail: "条目的「分类」标签在新增/编辑弹窗的「分类」一栏改，逗号/空格分隔，可多个。\n\
                 敲 `@维护` 只列带这个标签的条目，后面还能接词（`@维护 磁盘` 在维护类里再筛）。\n\
                 `@维护 @开发 磁盘` 是并集：两个分类的条目都进列表。\n\
                 裸 `@` 给一行分类总览（各类名 + 条数），Enter / 数字键只给一句用法提示。\n\
                 缺省分类范围设成「基本」时，无标签与标「基本」的条目恒显；显式 `@分类` 不受范围限制。",
        tags: "@ 分类 标签 筛选 维护 基本 category tag",
        group: Group::Syntax,
        source: None,
        settings_page: Some(1),
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
        summary: "在配置范围的索引里找（默认：桌面/下载/文档）",
        detail: "撇号后直接跟词。撇号可以是半角 `'`、弯引号 `‘`/`’`（中文输入法常打出这个）或全角 `＇`。\n\
                 索引范围/深度/上限/重扫间隔/最小字数都在设置页「文件搜索」可调；\n\
                 缺省：桌面、下载、文档，深度 4，3 万条封顶，每 30 分钟重扫一轮。\n\
                 结果最多 30 条。Enter 用系统关联程序打开；选中文件夹行时 Tab 进入该目录。",
        tags: "file 文件 文件名 搜索 撇号 apostrophe 索引 范围",
        group: Group::Files,
        source: Some(SourceId::Files),
        settings_page: Some(5),
    },
    CheatEntry {
        key: "索引范围",
        title: "文件索引：范围与时机可配",
        summary: "设置 → 文件搜索：目录 / 深度 / 上限 / 重扫 / 最小字数",
        detail: "扫描目录默认是三个用户目录（Desktop 与 OneDrive 重定向位都算），可以增删、支持 %环境变量%。\n\
                 递归深度 2/4/6/8，条目上限 1万/3万/10万，重扫间隔 5～180 分钟，进文件模式的最小字数 1/2/3。\n\
                 排除表按目录名整段跳过，node_modules / .git 等出厂七条可「恢复默认排除」。\n\
                 改动保存后自动在后台重建索引，旧索引在建成前继续可用。",
        tags: "file 索引 范围 目录 深度 上限 重扫 排除 设置 scope index",
        group: Group::Files,
        source: Some(SourceId::Files),
        settings_page: Some(5),
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
    CheatEntry {
        key: "Esc 返回上级",
        title: "目录浏览：逐级返回",
        summary: "目录里按 Esc 退一层",
        detail: "在目录浏览（`D:\\proj\\` 或 `D:\\proj\\rep`）里按 Esc：先去掉过滤词、留在本目录，\
                 再按就退到上一级；退到盘根后再按 Esc 才是清空查询。\n\
                 UNC 共享根（`\\\\srv\\share\\`）与盘根一样，再往上没有能浏览的目录。\n\
                 普通查询与文件索引搜索里 Esc 仍是原来的「清空查询」。\n\
                 状态栏左侧会写着这一刻按 Esc 会发生什么（返回上级 / 清除筛选 / 清空）。",
        tags: "esc 返回 上级 目录 browse parent back",
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
        key: "Insert",
        title: "标记 / 取消标记",
        summary: "文件行与文件/文件夹条目可标；标完选中自动下移",
        detail: "按一下把这行加进标记集合（名称前出现一个「✓ 」），再按一下取消；\
                 加完选中自动落到下一行，连着按就能一次标一批（到末行停在原地，不绕回开头）。\n\
                 能标记的只有两类：文件搜索行，和清单里类型是文件/文件夹的条目。\
                 计算器、终端、网页搜索、系统命令与各类提示行都按不动，只回一句提示。\n\
                 标记集合在窗口隐藏时清空（与 AHK 版同口径：下次呼出是干净的一屏）。\n\
                 标了至少一项时状态栏会写「已标记 N 项」，这时按 → 进的是批量面板。",
        tags: "key insert 标记 多选 mark 批量 batch",
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
        summary: "按序：收菜单 → 关框 → 退面板 → 清空 → 关闭到托盘",
        detail: "顺序：收 ☰ 菜单 → 关高级页/内部/删除确认框 → 关弹窗与帮助页 → 设置页返回搜索 \
                 → 退出操作面板（原输入与选中行放回去） → 目录浏览返回上级 → 清空输入 → 关闭到托盘。\n\
                 面板那一档排在「清空输入」之前是有意的：面板里的过滤词长得像路径时，\
                 不该被「返回上级」或「清空」抢走。\n\
                 清空那一档会主动把焦点要回搜索框（TextEdit 自己按 Esc 会弃焦点）。",
        tags: "key esc 取消 关闭 返回",
        group: Group::Keys,
        source: None,
        settings_page: None,
    },
    CheatEntry {
        key: "→ / ←",
        title: "操作面板",
        summary: "→ 进面板，← / Esc / 空 Backspace 退出",
        detail: "光标在输入末尾时按 → 进入选中项的操作面板（按住不放不会连跳：只认\"新按\"那一下）。\n\
                 面板里输入框变成过滤词，标题位显示「关于〈项标题〉的操作」；Enter 执行高亮那条动作，\n\
                 过滤词为空时数字键 1…0 指向前 10 条（与结果模式同一口径）。学习统计按「原查询」\n\
                 记名，不是按动作名。\n\
                 退出（← / Esc / 输入为空时 Backspace）把原输入文字与选中行原样放回去。\n\
                 有面板可进的行：清单条目、自定义命令行、文件行、网页搜索行、计算器行（批次 E5：\n\
                 它进的是「文本」那一张三格表，见「面板内」那条）；终端 / 各类提示行没有可处置的\n\
                 对象，按 → 不动（状态栏右格也就不亮那枚键帽）。\n\
                 已经有标记时（见 `Insert` 那条）→ 不看选中哪一行，直接进批量面板。\n\
                 右键「操作…」是同一个入口；动作清单见下一条。",
        tags: "key 箭头 面板 操作 action panel 右键",
        group: Group::Keys,
        source: None,
        settings_page: None,
    },
    CheatEntry {
        key: "面板内",
        title: "可做的动作",
        summary: "按对象给：打开 / 提权 / 所在位置 / 复制 / 粘贴 / 属性 / 回收站 / 编辑 / 置顶",
        detail: "按对象类型给，不给按下去只会报错的按钮：打开（条目有 run_title 用它当标题）、\n\
                 以管理员运行（UAC 点了\"否\"算取消、不算失败）、打开方式（系统对话框）、\n\
                 所在位置（explorer /select）、复制路径 / 网址 / 名称、在终端打开（目录进本目录、\n\
                 文件进所在目录）、属性（系统的属性框）、移入回收站（进回收站，不删清单条目）、\n\
                 编辑 / 添加为命令、删除（仍要再确认一次）、置顶 / 取消置顶、大字显示。\n\
                 批次 E6（2026-10-27）补上原文欠的单行两格：复制文件 / 剪切文件——盘上真有其物\n\
                 才给（与属性、回收站同一道门），写进剪贴板的是资源管理器认得的文件对象，\n\
                 跟批量面板那两条是同一份载荷，只是这里只有这一条路径。\n\
                 `shell:` 命名项与 `internal:` 动作行只给\"打开 / 复制 / 编辑 / 大字\"那一半，\n\
                 属性、所在位置、终端、回收站四条不出现——它们对没有盘上路径的对象无从下手。\n\
                 计算器行是一串文本、不是磁盘上的东西 ⇒ 它另有一张三格表：复制这一串（Enter 本来\n\
                 做的那件事）/ 粘贴到原窗口 / 大字显示，没有「打开」——把 14 交给系统去搜，\n\
                 找的是一个叫「14」的程序。\n\
                 做完之后：打开 / 提权 / 终端 / 所在位置 / 粘贴成功 ⇒ 窗口收进托盘；编辑 / 删除 /\n\
                 添加为命令 / 回收站成功 ⇒ 退出面板回结果；复制 / 属性 / 打开方式 / 大字 ⇒ 面板原地留着。",
        tags: "面板 动作 action 提权 属性 回收站 置顶 所在位置 复制 粘贴",
        group: Group::Keys,
        source: None,
        settings_page: None,
    },
    CheatEntry {
        key: "标记后 →",
        title: "批量面板",
        summary: "七个动作：全部打开 / 复制 / 剪切 / 复制路径 / 复制到 / 加入命令 / 回收站",
        detail: "有标记时 → 不看当前选中哪一行，直接对标记集合开面板，标题位写「已标记的 N 项」。\
                 面板里的过滤词、Enter 与数字键跟单条面板是同一套。七条动作：\n\
                 · 全部打开：按标记顺序逐个启动，成功后关面板、窗口收进托盘；一项都没打开就留在原地回一句。\n\
                 · 复制文件 / 剪切文件：写成资源管理器认得的文件对象（CF_HDROP）；剪切另写 Preferred DropEffect，\
                   粘到别处就是移动。粘贴完不替你清空标记。\n\
                 · 复制全部路径：一行一条，Windows 换行。\n\
                 · 复制到…：先弹系统文件夹选择框，再逐个复制；目标里有同名文件由系统问覆盖。\
                   来源正好是目标目录、或在目标目录之下时跳过（不会把文件夹往自己里面拷）。\n\
                 · 全部加入命令：按条目名造自定义命令，已经指向同一路径的跳过，新增的合并进清单并立即落盘。\n\
                 · 全部回收站：先弹确认框（Enter 执行 / Esc 取消）。逐个移入回收站，系统在「永久删除」\
                   警告里点了\"否\"的那一项算取消、不影响后面的；做完收回面板并清空标记。\n\
                 标记集合里只会有文件与文件夹，所以这七条对每一项都成立，不做逐条裁剪。",
        tags: "批量 batch 多选 复制 剪切 回收站 加入命令 复制到",
        group: Group::Keys,
        source: None,
        settings_page: None,
    },
    CheatEntry {
        key: "Ctrl+Enter",
        title: "次级动作",
        summary: "默认快捷键版：文件行定位 / 网页行复制网址 / 计算器行粘贴；可改键、可切成面板版",
        detail: "对当前选中行做\"次级动作\"——打开是 Enter，这一颗做另一件事。\n\
                 设置页「热键」页有两种形态（默认快捷键版，可随时切换）：\n\
                 · 快捷键版：文件行 / 目录行「在资源管理器里定位」，网页搜索行「复制网址」，\n\
                   计算器行「粘贴到呼出前那个窗口」（只打字，不动你的剪贴板）；\n\
                   应用条目、系统命令、自定义命令没有\"一眼就是它\"的次级动作 ⇒ 只提示你按 →\n\
                   看全部，不替你猜（也刻意不含\"打开\"：Enter 已经是打开）。\n\
                 · 面板版：等于把 → 那个入口搬到这颗键上，动作进面板再选。\n\
                 「粘贴到原窗口」那一格怎么保证不贴错地方：呼出那一刻先记下当时前台是谁，\n\
                 贴之前过三道闸——那个窗口还开着、系统肯把焦点给它、当场焦点真的在它那儿。\n\
                 任一条不成立就一个字都不投，只回一句为什么；投出去走打字通道，不借剪贴板。\n\
                 这颗键只在搜索页、没有弹窗、没开面板时生效；它不注册给系统，所以不抢别的\n\
                 程序的键，也不会像全局呼出键那样被系统整条吞掉。必须带 Ctrl 或 Alt（裸键与\n\
                 单带 Shift 的键都只是打字），Win 组合设不了；键本身可在同一页录制、可恢复默认\n\
                 （Backspace / Delete），改动立刻生效并写进 settings.json。\n\
                 新增/编辑弹窗开着时，这颗键在弹窗里改做「测试执行」——照当前表单启动一次，\n\
                 窗口不收、不记使用统计（批次 D4，见 Ctrl+O 那条）。",
        tags: "key ctrl enter 次级 动作 secondary 定位 复制网址 粘贴 paste 计算器",
        group: Group::Keys,
        source: None,
        settings_page: Some(3),
    },
    CheatEntry {
        key: "Ctrl+O",
        title: "弹窗·选文件",
        summary: "只在新增/编辑弹窗里生效：命令行＝所选文件，别名/名称按文件名推",
        detail: "给新增/编辑弹窗补路径用的（需求书 FR-CFG-06），搜索页里这颗键不干别的。\n\
                 开系统的文件选择框，只能挑已存在的文件——避免手打一个不存在的名字。选定后：\n\
                 · 命令行＝那个文件的全路径；\n\
                 · 别名/名称按 Delphi ALTRun 的 `btnFileClick` 口径从文件名推：剥掉最后一段扩展名，\n\
                   以 `//` 开头的按网址处理（Chrome `--app=…` 那类命令行就是这么变成主机名的）。\n\
                 那两格已经填了别的值 ⇒ 先问一句「替换 / 保留」；按「保留」只补空着的格，\n\
                 命令行照样换（那才是这条命令到底是什么）。询问开着时按 Esc 只收询问、不关弹窗。\n\
                 不想按键也可以直接把 .lnk / 程序 / 文件夹拖进弹窗（Delphi 的 `WM_DropFiles` 同款）：\n\
                 .lnk 会解引用成它指向的目标，指向系统 Installer 缓存目录的 .lnk 则按 .lnk 本体导入。\n\
                 系统命令（`internal:<id>`）那一档整组按钮与热键都不给用——命令行被覆盖就执行不了了。",
        tags: "key 弹窗 dialog 选文件 浏览 browse 拖入 drop lnk 路径",
        group: Group::Keys,
        source: None,
        settings_page: None,
    },
    CheatEntry {
        key: "Ctrl+Shift+O",
        title: "弹窗·选目录",
        summary: "只在新增/编辑弹窗里生效：命令行＝所选目录，名字取目录末段",
        detail: "和 `Ctrl+O` 同一条路，只是选的是文件夹（同一套系统选择框，切到选目录那档）。\n\
                 命令行＝目录全路径；别名/名称取最后一个 `\\` 之后那一段，不剥扩展名——\n\
                 `1.2图片` 这类名字带点的目录要原样保留（Delphi 在这条分支上先判 `DirectoryExists`）。\n\
                 类型跟着变成「文件夹」。同样：那两格已有别的值先问「替换 / 保留」，Esc 只收询问。",
        tags: "key 弹窗 dialog 选目录 选文件夹 目录 directory 浏览",
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
        key: "Ctrl+E / F3",
        title: "编辑选中项",
        summary: "改名称 / 关键字 / 命令行",
        detail: "编辑清单里的那一条。文件搜索行不在清单里，对它不适用（它对「添加为快捷项」有意义）。\n\
                 内置命令（系统命令分组）的类型是只读的，改不了运行方式。\n\
                 `F3` 是批次 B8（2026-10-26，需求书附录 A）加的别名，与 `Ctrl+E` 完全同一条路；\n\
                 操作面板开着时两者都不接管（跟 `Insert` 同口径）。",
        tags: "key f3 编辑 edit",
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
        summary: "置顶项图标右上角带角标",
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
        key: "Ctrl+G",
        title: "切换缺省分类范围",
        summary: "全部 ⇄ 基本，列表立刻跟着变",
        detail: "在「全部」（所有条目都进候选池）与「基本」（无标签 ∪ 标「基本」）两档之间切换，\n\
                 与设置页「通用 → 缺省分类范围」是同一处配置，切完立即写盘并给一句 Toast。\n\
                 显式 `@分类` 筛选不受这一档影响。",
        tags: "key scope 分类 范围 全部 基本 切换",
        group: Group::Keys,
        source: None,
        settings_page: Some(1),
    },
    CheatEntry {
        key: "Ctrl+Del",
        title: "隐藏选中项",
        summary: "出厂 / 扫描 / 自定义条目都行；设置 → 高级 可恢复",
        detail: "把选中条目写进 `hidden[]` 并从列表剔除，同时在 settings.json 里存一份完整快照；\n\
                 不删磁盘文件，也不删清单里的其它字段。\n\
                 只在搜索框为空时生效——有字时 Ctrl+Del 是输入框的「删除后一个词」。\n\
                 重新扫描或「补回缺失的出厂命令」都不会把它塞回来；恢复后置顶与学习计数一起回来。\n\
                 要彻底删掉用 Delete / Ctrl+Shift+D（会弹确认框）。\n\
                 恢复入口：设置 → 高级 →「隐藏的条目」。",
        tags: "key 隐藏 hide hidden 条目 扫描 恢复",
        group: Group::Keys,
        source: None,
        settings_page: Some(7),
    },
    CheatEntry {
        key: "Ctrl+L",
        title: "大字显示",
        summary: "把选中行放大到屏幕中央；没有选中行就显示输入原文",
        detail: "无边框置顶、`141414` 黑底白字（不跟随主题），窗口在工作区居中。\n\
                 字号按字符数分档：≤12 → 96、≤30 → 64、≤80 → 40、≤300 → 28、其余 18；\n\
                 放不进屏幕会继续降档，窗口尺寸始终不超过工作区。\n\
                 主窗留在原位，大字在它上面；关掉大字后焦点回到主窗，可以接着改查询。\n\
                 关闭方式：Esc / 点一下 / 切到别的窗口。显示内容不写日志。",
        tags: "key 大字 放大 展示 large type",
        group: Group::Keys,
        source: None,
        settings_page: None,
    },
    CheatEntry {
        key: "Ctrl+C",
        title: "复制这一行",
        summary: "文件取路径、计算器取数字、应用取名称；不关窗",
        detail: "复制的是当前高亮那一行的「可复制内容」，与 `Ctrl+L` 大字显示同一串（批次 B8，2026-10-26）：\n\
                 文件 / 目录 / 网页搜索行取路径或网址，计算器行取算出来的数字，\n\
                 清单里的应用与命令取名称，参数提示行取命令原文。\n\
                 搜索框里有焦点且选中了文字时，这一颗键让给输入框——复制的是你选中的那段字，不是列表行。\n\
                 复制完窗口留在原地（AHK 版是复制即关窗，这里不关，方便接着 Ctrl+V）。\n\
                 这一屏没有可复制的行时只给一句提示。复制的内容不写日志，只记字符数。",
        tags: "key ctrl c 复制 copy 剪贴板",
        group: Group::Keys,
        source: None,
        settings_page: None,
    },
    CheatEntry {
        key: "Alt+Enter",
        title: "复制这一行（另一颗键）",
        summary: "与 Ctrl+C 同一串内容；面板开着时它就是 Enter",
        detail: "批次 E6（2026-10-27，需求书 FR-ACT-01 的最后一格）：`Alt+Enter` 复制当前高亮那一行，\n\
                 取的是与 `Ctrl+C` 完全相同的一串（AHK 那两颗键本来就是同一个 `CopyText()`）。\n\
                 搜索框里有焦点且选中了文字时同样让给输入框，复制你选中的那段字。\n\
                 操作面板开着的时候这颗键不接管：面板里的 Enter 就是「执行高亮那条动作」，\n\
                 多按一个 Alt 不改变它（与 AHK 的 `_Execute` 同口径）。\n\
                 它是窗口内热键，不注册给系统 ⇒ 别的程序里的 Alt+Enter（属性对话框那类）不受影响；\n\
                 也正因如此，呼出热键与次级动作键都不许录成这一组（`? 热键` 那页会被挡）。\n\
                 复制完不关窗，内容不写日志，只记字符数。",
        tags: "key alt enter 复制 copy 剪贴板 这一行",
        group: Group::Keys,
        source: None,
        settings_page: None,
    },
    CheatEntry {
        key: "Ctrl+↑ / Ctrl+↓",
        title: "翻搜索历史",
        summary: "回填之前执行过的那串；只填文本，不替你执行",
        detail: "批次 C3（2026-10-26，需求书 FR-QRY-10）：`Ctrl+↑` 往更早一条翻，`Ctrl+↓` 往更新一条翻。\n\
                 翻到最早就停在那儿（输入框不动）；翻回最末再按一次 `Ctrl+↓` 回到你自己刚敲的那串。\n\
                 存进历史的是「提交出去的那一串」：回车启动、点开文件行、`>` 终端命令、计算器复制、\n\
                 面板里选一条走掉，都算一次执行。启动失败也照记（AHK 的 `Knowledge.Record` 就是先记账再运行，\n\
                 你要的是改两个字再试一次，框里那串得留着）。\n\
                 只存文本、不存结果：回填进来的串仍然要按回车才执行，也不会替你选中任何一行。\n\
                 重复按小写认（`C:\\Foo` 与 `c:\\foo` 是同一条），存的时候按原样存，不改大小写。\n\
                 上限与「不再记」在「通用」页「搜索历史」那一格；立刻清干净用「高级」页那个按钮。\n\
                 操作面板开着、文件模式（' 与 > 那几式）下不接管：那两处 ↑↓ 是它们自己的导航键。\n\
                 裸 `↑` `↓` 仍是选择条目，多按一个 Shift（`Ctrl+Shift+↑`）照旧算翻历史。",
        tags: "key ctrl up down 箭头 历史 history 回填 搜索 查询",
        group: Group::Keys,
        source: None,
        settings_page: Some(1),
    },
    CheatEntry {
        key: "参数框",
        title: "先填参数再运行",
        summary: "编码≠无参数、且带 {%p}（或一个标记都没有）时，回车前先弹这一框",
        detail: "批次 B6（2026-10-28，需求书 FR-ACT-07）：回车执行命中带参项时先出参数框，\
                 填完再按一次回车才真的启动；Esc 取消，什么都不启动。\n\
                 只有 `{%p}`（或「编码≠无参数却一个标记都没有」）会弹框；只写 `{%c}` / `{%wt}` 这类现场标记的条目直接启动。\n\
                 框下面列出「最近用过的参数」，点一条就填进输入框；这张表是 `param_history.json`，\
                 在框里按确定且值非空就记一条（启动失败也记）。顺序是文件里的先后（新的在前），\
                 复用同一个值只加计数、不挪位置；最多 50 条，超出时删用得最少的那条（并列删靠后的）。\n\
                 只有用户亲手敲进这个框的那串进历史；剪贴板、窗口标题这些现场值不存，参数原文也不写日志。\n\
                 清空入口在「高级」页的「清空参数历史」（这一条不写备份）。\n\
                 这一框不改变条目本身；编码、标记的写法在编辑弹窗里改（Ctrl+E）。",
        tags: "key 参数 参数框 parameter prompt 历史 history 最近用过",
        group: Group::Keys,
        source: None,
        settings_page: Some(7),
    },
    CheatEntry {
        key: "F1",
        title: "关于本程序",
        summary: "版本、程序位置、清单条数；Esc 关闭",
        detail: "弹窗里有版本号、exe 的路径、当前清单条目数，和一句「日志不记查询框原文」的说明。\n\
                 批次 B8（2026-10-26，需求书附录 A）新增；与 `F2` 同为「整窗口级」，\n\
                 设置页开着时照样按得动。☰ 菜单里没有这一项，托盘菜单里也没有。\n\
                 关闭：Esc 或点「关闭 (Esc)」，关掉后焦点回到搜索框。",
        tags: "key f1 关于 about 版本",
        group: Group::Keys,
        source: None,
        settings_page: None,
    },
    CheatEntry {
        key: "F4",
        title: "打开配置文件所在目录",
        summary: "在资源管理器里打开配置目录（不是单个文件）",
        detail: "需求书附录 A 原文是「打开配置文件」，这里打开的是那个目录：\n\
                 目录里同时有 `settings.json`、`user_items.json` 和 `launcher.log`，\n\
                 指到某一个文件没有唯一答案，所以给目录（批次 B8 登记的口径偏差）。\n\
                 清单目录可写时就在 exe 旁边，否则退到 `%LOCALAPPDATA%`——两种情况都按实际那一个打开。\n\
                 与选中行无关，操作面板开着时也有效。目录不存在时给一句提示。",
        tags: "key f4 配置 文件 settings.json 目录 打开",
        group: Group::Keys,
        source: None,
        settings_page: None,
    },
    CheatEntry {
        key: "Ctrl+Backspace",
        title: "删掉输入框里前一个词",
        summary: "egui 输入框自带，本程序没有接管这颗键",
        detail: "光标在搜索框里时 `Ctrl+Backspace` 删前一个词，有选区时删掉选区——\n\
                 这一段是 egui 的 TextEdit 自己做的，本程序没有注册这颗键。\n\
                 对面的 `Ctrl+Del` 删后一个词，但输入框为空时 `Ctrl+Del` 是「隐藏选中项」。\n\
                 「空过滤词按 Backspace ＝退出操作面板」只认不带修饰的那一下（批次 B8 同日真机抓到\n\
                 `Ctrl+Backspace` 会连面板一起退出，已改成看事件自带的修饰键）。",
        tags: "key ctrl backspace 删词 退格 删除 输入框",
        group: Group::Keys,
        source: None,
        settings_page: None,
    },
    CheatEntry {
        key: "Ctrl+, / F2",
        title: "打开 / 返回设置页",
        summary: "九页：通用 / 功能 / 热键 / 数据源 / 文件搜索 / 外观 / 高级 / 批量管理 / 使用统计",
        detail: "打开设置页；已开着时按它返回搜索页（与 Esc 的「设置页返回搜索」同一档）。\n\
                 `F2` 是批次 B8（2026-10-26，需求书附录 A）加的别名，两颗键走同一段代码；\n\
                 设置页已经开着时它也有效（与 `F1` 同为「整窗口级」，取键在分页之前）。",
        tags: "key f2 设置 settings preferences",
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
        key: "Ctrl+B",
        title: "批量管理",
        summary: "设置页第 8 页：勾选批量删、只选失效项、绝对 ↔ 相对路径互转",
        detail: "直接打开设置页「批量管理」（左侧分类栏最后一项，从设置页点进去也一样）。\n\
                 勾选条目后可以：转为相对写法（`%LAUNCHER_DIR%\\…`，只转本程序目录下的行）、还原成绝对路径、\n\
                 从清单里删除。删除只动 `user_items.json`，不碰磁盘文件；转换只改命令行那一格，参数与工作目录不动。\n\
                 状态列沿用主界面的四态：「只选失效项」只勾 `Not found`，`Offline`（拔掉的 U 盘 / 断线的共享）不算失效。\n\
                 两种批量动作都要先确认，确认框上写清条数。需求书原文的便携写法是 `.\\`，本仓用 `%LAUNCHER_DIR%`\n\
                 代替（`.\\` 认的是进程当前目录，而本程序从不 SetCurrentDirectory，同一份清单会时灵时不灵）。",
        tags: "key 批量 batch 管理 manage 删除 相对路径 绝对路径 失效",
        group: Group::Keys,
        source: None,
        settings_page: Some(8),
    },
    CheatEntry {
        key: "Ctrl+R",
        title: "批量管理·重新校验",
        summary: "只在设置页「批量管理」里有效",
        detail: "重跑一遍路径四态检查，状态列立刻刷新（走进这一页时也会自动跑一轮）。\n\
                 上一轮还没回来时不攒线程：断开的网络盘会让检查挂很久，这时按它只提示稍等。",
        tags: "key 校验 check 刷新 路径 失效",
        group: Group::Keys,
        source: None,
        settings_page: Some(8),
    },
    CheatEntry {
        key: "Ctrl+T",
        title: "使用统计",
        summary: "设置页第 9 页：合计 / 近 30 天柱状 / 按功能次数 / 按条目前 10",
        detail: "直接打开设置页「使用统计」（左侧分类栏；批次 D2 起它后面还多了一页「网页引擎」）。这一页是只读看板，除了「清除统计」没有能改的东西。\n\
                 合计与「按功能次数」的数据在 `usage.json`：只记哪天、哪一类功能、几次，查询词 / 条目名 / 路径都不进这个文件。\n\
                 记账点跟条目上的累计次数是同一批：清单项与系统命令跑成功各记一次，计算器复制成功、网页搜索、\n\
                 文件与目录、终端命令、自定义命令打开成功同理；没跑成的不记。\n\
                 条目上的「累计次数 / 最近使用」是另一本账（排序用），清统计不动它——要清那个去「重置学习排序」。\n\
                 保留 400 天（含今天）：更早的按真实日期裁掉，读不出来的日期键直接丢。\n\
                 一次启动可能同时落「清单项」与「自定义命令」两个桶（这条清单被启动 / 一条自定义命令被执行），\n\
                 所以「合计」会比实际启动次数多——这是有意的口径，不是重复计错（2026-10-29 用户裁定保留）。",
        tags: "key 统计 stats usage 使用次数 图表 使用频率",
        group: Group::Keys,
        source: None,
        settings_page: Some(9),
    },
    CheatEntry {
        key: "Ctrl+W",
        title: "网页引擎表",
        summary: "设置页第 10 页：增删改搜索引擎、选兜底引擎",
        detail: "直接打开设置页「网页引擎」（左侧分类栏）。\n\
                 每一条是「关键字 / 名称 / 网址模板」：关键字 + 空格 + 词 才独占整张列表，模板里的 {query} 换成年词。\n\
                 改一格、点别处就保存；「恢复出厂九条」把整张表换回出厂的九条，自己加的条目会被清掉。\n\
                 「兜底搜索」决定一条结果都没有时用哪个引擎出行，出厂是 Google → Bing。\n\
                 关键字或网址没填全的行、以及关键字与前面重复的行，离开这一页时不保存（页面上会先提示条数）。",
        tags: "key 网页 引擎 搜索 engine 设置 settings 兜底",
        group: Group::Keys,
        source: None,
        settings_page: Some(10),
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
    CheatEntry {
        key: "Alt+R",
        title: "副呼出热键",
        summary: "第二颗呼出键，效果与主呼出键相同；设置页可改、可清除",
        detail: "出厂 Alt+R（AHK 版的「次热键」）。设置页「热键」页可录制新组合，也可以清除——\
                 清掉之后就只剩主呼出键。\n\
                 与主呼出键同一套规矩：必须带修饰键、不能和主呼出键或补字符键是同一组；\
                 注册成功才写进 settings.json，失败会照实报错并把原来那颗装回去。",
        tags: "key hotkey 热键 呼出 副 次 extra",
        group: Group::Keys,
        source: None,
        settings_page: Some(3),
    },
    CheatEntry {
        key: "双击 Ctrl / 双击 Shift",
        title: "双击呼出",
        summary: "连按两下同一个修饰键切一次显隐（默认关闭）",
        detail: "设置页「热键」页三档：关闭 / 双击 Ctrl / 双击 Shift，出厂是关闭。\n\
                 判定口径照 AHK 版：两下之间 400ms 以内才算双击；单次按住超过 300ms 不算\
                 （那是 Ctrl+C 这类组合键的起手）；两次抬起间隔小于 25ms 的当远控软件 /\
                 自动化的同步抖动丢掉。左右 Ctrl / Shift 都算，两下可以是不同的那一边。\n\
                 开启时本程序会挂一个「只旁听、不拦键」的键盘钩子：所有按键照常送到你正在用的\
                 程序，它只负责数那两下。Shift+CapsLock 这类组合照常工作。",
        tags: "key 双击 呼出 double tap ctrl shift",
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
        key: "鼠标滚轮",
        title: "移动选中",
        summary: "一格一行，到顶到底停住不回绕",
        detail: "批次 B8（2026-10-26，需求书附录 A「滚轮移动选中」，口径同 AHK 版）：\n\
                 在结果列表上滚一格 = 选中上移或下移一行，边界与 ↑↓ 完全一样（不绕回）。\n\
                 列表位置不另外滚，仍由既有的「滚动跟随选中」负责，所以后面的行照样看得见。\n\
                 一格只走一行，不会又移选中又滚一截——两处用的是同一个判据。\n\
                 要快走有 PgUp / PgDn（一整页）。带修饰的滚轮不移动选中。\n\
                 新增/编辑弹窗、删除确认、批量确认、关于页、帮助页盖在上面时，\n\
                 滚轮回到默认行为：只滚列表，不动选中。",
        tags: "mouse 滚轮 滚动 wheel scroll 选中",
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
    [
        entry.key,
        entry.title,
        entry.summary,
        entry.tags,
        entry.group.aliases(),
    ]
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
        Some(
            hits.into_iter()
                .map(|entry| SourceRow::Cheat(CheatHit { entry }))
                .collect(),
        )
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
            assert!(
                !e.tags.is_empty(),
                "「{}」没有过滤词，`? 关键词` 搜不到它",
                e.title
            );
            if let Some(page) = e.settings_page {
                // 批次 F1 加了第 5 页「外观」，上界跟着走（现量：SettingsTab::from_page 认 1..=5）。
                // 【2026-10-24 批次 F7：加了第 6 页「高级」，上界改为 1..=6。】
                // 【2026-10-27 批次 D7：插入第 5 页「文件搜索」，外观/高级顺延为 6/7，上界 1..=7。】
                // 【2026-10-28 批次 D5：追加第 8 页「批量管理」，上界 1..=8。】
                // 【2026-10-28 批次 D8：追加第 9 页「使用统计」，上界 1..=9。】
                // 【2026-10-29 批次 D2：追加第 10 页「网页引擎」，上界 1..=10。】
                assert!(
                    (1..=10).contains(&page),
                    "「{}」指向不存在的设置页 {page}",
                    e.title
                );
            }
            // 列表与帮助页都是纯 `ui.label`，不解析 Markdown：`**粗体**` 会原样带着星号出场
            //（2026-10-23 真机截图里就是这么露的）。
            for text in [e.key, e.title, e.summary, e.detail, e.tags] {
                assert!(
                    !text.contains("**"),
                    "「{}」的文案里有 Markdown 粗体标记：{text}",
                    e.title
                );
            }
        }
    }

    #[test]
    fn filters_hit_what_the_acceptance_criteria_name() {
        // 需求书验收原话：「敲 `?` 能看到"空格=文件模式、`>`=终端"等条目」。
        let all = entries("", &all_on);
        assert!(
            all.iter().any(|e| e.key.contains('>')),
            "`?` 里看不到终端条目"
        );
        assert!(
            all.iter().any(|e| e.key.contains("空格")),
            "`?` 里看不到空格开头的文件模式"
        );
        for needle in ["file", "文件", "快捷键", "终端", "计算器", "复制", "滚轮", "关于"] {
            assert!(entries(needle, &all_on).iter().all(|e| matches(e, needle)));
            assert!(
                !entries(needle, &all_on).is_empty(),
                "过滤词「{needle}」一条也命不中"
            );
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
            web_fallback: &[],
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
        let mut cx2 = SearchCtx {
            query: "?没有这种条目",
            index: &mut index2,
            ..cx
        };
        let rows = src.take_over(&mut cx2).expect("过滤词为空也不是普通搜索");
        assert!(matches!(rows.as_slice(), [SourceRow::CheatHint(_)]));

        // 不是第 0 个字符 → 不参与（`x?` 归普通搜索）。
        let mut index3 = crate::matching::MatchIndex::default();
        let mut cx3 = SearchCtx {
            query: "x?",
            index: &mut index3,
            ..cx2
        };
        assert!(src.take_over(&mut cx3).is_none());
    }
}
