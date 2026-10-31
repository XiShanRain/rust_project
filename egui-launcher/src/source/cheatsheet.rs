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
use crate::i18n;

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
        i18n::t(match self {
            Self::Syntax => "cheat.group.syntax",
            Self::Files => "cheat.group.files",
            Self::Keys => "cheat.group.keys",
            Self::Window => "cheat.group.window",
        })
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

/// 速查表的一条。文本字段全是语言包键：表在编译期定死，显示时按当前语言取名。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CheatEntry {
    /// 示例写法（列表里的大字，等宽显示）。示例里的中文词跟着语言走，所以它是键。
    pub example_key: &'static str,
    /// 这一条叫什么（帮助页标题）。
    pub title_key: &'static str,
    /// 列表行里的那句话。
    pub summary_key: &'static str,
    /// 帮助页正文，`\n` 分段。
    pub detail_key: &'static str,
    /// 过滤用同义词（`? file`、`? 快捷键` 靠它命中）。**双语都留**：不显示，
    /// 也不跟着语言换——英文界面下敲「文件」仍该找得到那几条。
    pub tags: &'static str,
    pub group: Group,
    /// 依赖哪个数据源：该源关掉时这一条整条不出现。`None` ＝ 与数据源无关。
    pub source: Option<SourceId>,
    /// 帮助页上「打开设置页」跳到第几页（1=通用 2=功能 3=热键 4=数据源 5=文件搜索 6=外观 7=高级
    /// 8=批量管理 9=使用统计 10=网页引擎）。
    pub settings_page: Option<u32>,
}

impl CheatEntry {
    pub fn example(&self) -> &'static str {
        i18n::t(self.example_key)
    }

    pub fn title(&self) -> &'static str {
        i18n::t(self.title_key)
    }

    pub fn summary(&self) -> &'static str {
        i18n::t(self.summary_key)
    }

    pub fn detail(&self) -> &'static str {
        i18n::t(self.detail_key)
    }
}

pub const ENTRIES: &[CheatEntry] = &[
    CheatEntry {
        example_key: "cheat.help.example",
        title_key: "cheat.help.title",
        summary_key: "cheat.help.summary",
        detail_key: "cheat.help.detail",
        tags: "help cheatsheet 速查 帮助 语法 syntax",
        group: Group::Syntax,
        source: Some(SourceId::Cheatsheet),
        settings_page: None,
    },
    CheatEntry {
        example_key: "cheat.web.example",
        title_key: "cheat.web.title",
        summary_key: "cheat.web.summary",
        detail_key: "cheat.web.detail",
        tags: "web search 搜索 网页 引擎 google 百度 github",
        group: Group::Syntax,
        source: Some(SourceId::Web),
        settings_page: Some(10),
    },
    CheatEntry {
        example_key: "cheat.custom_args.example",
        title_key: "cheat.custom_args.title",
        summary_key: "cheat.custom_args.summary",
        detail_key: "cheat.custom_args.detail",
        tags: "command 自定义 命令 参数 parameter",
        group: Group::Syntax,
        source: Some(SourceId::Custom),
        settings_page: Some(4),
    },
    CheatEntry {
        example_key: "cheat.query_placeholder.example",
        title_key: "cheat.query_placeholder.title",
        summary_key: "cheat.query_placeholder.summary",
        detail_key: "cheat.query_placeholder.detail",
        tags: "placeholder query 占位符 参数",
        group: Group::Syntax,
        source: Some(SourceId::Custom),
        settings_page: None,
    },
    CheatEntry {
        example_key: "cheat.param_markers.example",
        title_key: "cheat.param_markers.title",
        summary_key: "cheat.param_markers.summary",
        detail_key: "cheat.param_markers.detail",
        tags: "param 参数 标记 marker p c wd wt wc 编码 剪贴板 窗口 替身",
        group: Group::Syntax,
        source: None,
        settings_page: None,
    },
    CheatEntry {
        example_key: "cheat.runflags.example",
        title_key: "cheat.runflags.title",
        summary_key: "cheat.runflags.summary",
        detail_key: "cheat.runflags.detail",
        tags: "run flag 运行标志 最大化 最小化 隐藏 后台 @+ @-",
        group: Group::Syntax,
        source: None,
        settings_page: None,
    },
    CheatEntry {
        example_key: "cheat.calc.example",
        title_key: "cheat.calc.title",
        summary_key: "cheat.calc.summary",
        detail_key: "cheat.calc.detail",
        tags: "calc 计算器 算式 数学 加减乘除",
        group: Group::Syntax,
        source: Some(SourceId::Calc),
        settings_page: Some(4),
    },
    CheatEntry {
        example_key: "cheat.units.example",
        title_key: "cheat.units.title",
        summary_key: "cheat.units.summary",
        detail_key: "cheat.units.detail",
        tags: "unit 单位 换算 convert km mile 温度 长度 重量",
        group: Group::Syntax,
        source: Some(SourceId::Calc),
        settings_page: None,
    },
    CheatEntry {
        example_key: "cheat.radix_date.example",
        title_key: "cheat.radix_date.title",
        summary_key: "cheat.radix_date.summary",
        detail_key: "cheat.radix_date.detail",
        tags: "hex binary 进制 日期 date today 十六进制",
        group: Group::Syntax,
        source: Some(SourceId::Calc),
        settings_page: None,
    },
    CheatEntry {
        example_key: "cheat.match_modes.example",
        title_key: "cheat.match_modes.title",
        summary_key: "cheat.match_modes.summary",
        detail_key: "cheat.match_modes.detail",
        tags: "pinyin 拼音 首字母 驼峰 camel 多词 匹配",
        group: Group::Syntax,
        source: None,
        settings_page: None,
    },
    CheatEntry {
        example_key: "cheat.wildcard.example",
        title_key: "cheat.wildcard.title",
        summary_key: "cheat.wildcard.summary",
        detail_key: "cheat.wildcard.detail",
        tags: "wildcard regex 通配符 正则 星号 问号 模式 pattern",
        group: Group::Syntax,
        source: None,
        settings_page: None,
    },
    CheatEntry {
        example_key: "cheat.tags.example",
        title_key: "cheat.tags.title",
        summary_key: "cheat.tags.summary",
        detail_key: "cheat.tags.detail",
        tags: "@ 分类 标签 筛选 维护 基本 category tag",
        group: Group::Syntax,
        source: None,
        settings_page: Some(1),
    },
    CheatEntry {
        example_key: "cheat.path.example",
        title_key: "cheat.path.title",
        summary_key: "cheat.path.summary",
        detail_key: "cheat.path.detail",
        tags: "file path 路径 浏览 目录 browse",
        group: Group::Files,
        source: Some(SourceId::Files),
        settings_page: Some(4),
    },
    CheatEntry {
        example_key: "cheat.name_search.example",
        title_key: "cheat.name_search.title",
        summary_key: "cheat.name_search.summary",
        detail_key: "cheat.name_search.detail",
        tags: "file 文件 文件名 搜索 撇号 apostrophe 索引 范围",
        group: Group::Files,
        source: Some(SourceId::Files),
        settings_page: Some(5),
    },
    CheatEntry {
        example_key: "cheat.index_scope.example",
        title_key: "cheat.index_scope.title",
        summary_key: "cheat.index_scope.summary",
        detail_key: "cheat.index_scope.detail",
        tags: "file 索引 范围 目录 深度 上限 重扫 排除 设置 scope index",
        group: Group::Files,
        source: Some(SourceId::Files),
        settings_page: Some(5),
    },
    CheatEntry {
        example_key: "cheat.leading_space.example",
        title_key: "cheat.leading_space.title",
        summary_key: "cheat.leading_space.summary",
        detail_key: "cheat.leading_space.detail",
        tags: "file space 空格 文件模式",
        group: Group::Files,
        source: Some(SourceId::Files),
        settings_page: Some(1),
    },
    CheatEntry {
        example_key: "cheat.verbs.example",
        title_key: "cheat.verbs.title",
        summary_key: "cheat.verbs.summary",
        detail_key: "cheat.verbs.detail",
        tags: "file open find 文件 动词",
        group: Group::Files,
        source: Some(SourceId::Files),
        settings_page: None,
    },
    CheatEntry {
        example_key: "cheat.types.example",
        title_key: "cheat.types.title",
        summary_key: "cheat.types.summary",
        detail_key: "cheat.types.detail",
        tags: "file type 类型 扩展名 doc pic video audio zip exe cad",
        group: Group::Files,
        source: Some(SourceId::Files),
        settings_page: None,
    },
    CheatEntry {
        example_key: "cheat.folders.example",
        title_key: "cheat.folders.title",
        summary_key: "cheat.folders.summary",
        detail_key: "cheat.folders.detail",
        tags: "folder 文件夹 目录",
        group: Group::Files,
        source: Some(SourceId::Files),
        settings_page: None,
    },
    CheatEntry {
        example_key: "cheat.esc_up.example",
        title_key: "cheat.esc_up.title",
        summary_key: "cheat.esc_up.summary",
        detail_key: "cheat.esc_up.detail",
        tags: "esc 返回 上级 目录 browse parent back",
        group: Group::Files,
        source: Some(SourceId::Files),
        settings_page: None,
    },
    CheatEntry {
        example_key: "cheat.terminal.example",
        title_key: "cheat.terminal.title",
        summary_key: "cheat.terminal.summary",
        detail_key: "cheat.terminal.detail",
        tags: "terminal 终端 shell 命令 cmd powershell",
        group: Group::Syntax,
        source: Some(SourceId::Terminal),
        settings_page: Some(2),
    },
    CheatEntry {
        example_key: "cheat.arrows.example",
        title_key: "cheat.arrows.title",
        summary_key: "cheat.arrows.summary",
        detail_key: "cheat.arrows.detail",
        tags: "key 选择 上下 arrow",
        group: Group::Keys,
        source: None,
        settings_page: None,
    },
    CheatEntry {
        example_key: "cheat.insert.example",
        title_key: "cheat.insert.title",
        summary_key: "cheat.insert.summary",
        detail_key: "cheat.insert.detail",
        tags: "key insert 标记 多选 mark 批量 batch",
        group: Group::Keys,
        source: None,
        settings_page: None,
    },
    CheatEntry {
        example_key: "cheat.paging.example",
        title_key: "cheat.paging.title",
        summary_key: "cheat.paging.summary",
        detail_key: "cheat.paging.detail",
        tags: "key 翻页 page",
        group: Group::Keys,
        source: None,
        settings_page: None,
    },
    CheatEntry {
        example_key: "cheat.digits.example",
        title_key: "cheat.digits.title",
        summary_key: "cheat.digits.summary",
        detail_key: "cheat.digits.detail",
        tags: "key 数字 行号 digit 执行",
        group: Group::Keys,
        source: None,
        settings_page: None,
    },
    CheatEntry {
        example_key: "cheat.tab.example",
        title_key: "cheat.tab.title",
        summary_key: "cheat.tab.summary",
        detail_key: "cheat.tab.detail",
        tags: "key tab 补全 下一项 complete",
        group: Group::Keys,
        source: None,
        settings_page: Some(1),
    },
    CheatEntry {
        example_key: "cheat.shift_tab.example",
        title_key: "cheat.shift_tab.title",
        summary_key: "cheat.shift_tab.summary",
        detail_key: "cheat.shift_tab.detail",
        tags: "key shift tab 上一项",
        group: Group::Keys,
        source: None,
        settings_page: None,
    },
    CheatEntry {
        example_key: "cheat.ctrl_np.example",
        title_key: "cheat.ctrl_np.title",
        summary_key: "cheat.ctrl_np.summary",
        detail_key: "cheat.ctrl_np.detail",
        tags: "key ctrl 导航 下一项 上一项 navigate",
        group: Group::Keys,
        source: None,
        settings_page: None,
    },
    CheatEntry {
        example_key: "cheat.space.example",
        title_key: "cheat.space.title",
        summary_key: "cheat.space.summary",
        detail_key: "cheat.space.detail",
        tags: "key space 空格 执行",
        group: Group::Keys,
        source: None,
        settings_page: Some(1),
    },
    CheatEntry {
        example_key: "cheat.enter.example",
        title_key: "cheat.enter.title",
        summary_key: "cheat.enter.summary",
        detail_key: "cheat.enter.detail",
        tags: "key enter 运行 回车",
        group: Group::Keys,
        source: None,
        settings_page: None,
    },
    CheatEntry {
        example_key: "cheat.esc.example",
        title_key: "cheat.esc.title",
        summary_key: "cheat.esc.summary",
        detail_key: "cheat.esc.detail",
        tags: "key esc 取消 关闭 返回",
        group: Group::Keys,
        source: None,
        settings_page: None,
    },
    CheatEntry {
        example_key: "cheat.panel.example",
        title_key: "cheat.panel.title",
        summary_key: "cheat.panel.summary",
        detail_key: "cheat.panel.detail",
        tags: "key 箭头 面板 操作 action panel 右键",
        group: Group::Keys,
        source: None,
        settings_page: None,
    },
    CheatEntry {
        example_key: "cheat.panel_actions.example",
        title_key: "cheat.panel_actions.title",
        summary_key: "cheat.panel_actions.summary",
        detail_key: "cheat.panel_actions.detail",
        tags: "面板 动作 action 提权 属性 回收站 置顶 所在位置 复制 粘贴",
        group: Group::Keys,
        source: None,
        settings_page: None,
    },
    CheatEntry {
        example_key: "cheat.batch_panel.example",
        title_key: "cheat.batch_panel.title",
        summary_key: "cheat.batch_panel.summary",
        detail_key: "cheat.batch_panel.detail",
        tags: "批量 batch 多选 复制 剪切 回收站 加入命令 复制到",
        group: Group::Keys,
        source: None,
        settings_page: None,
    },
    CheatEntry {
        example_key: "cheat.secondary.example",
        title_key: "cheat.secondary.title",
        summary_key: "cheat.secondary.summary",
        detail_key: "cheat.secondary.detail",
        tags: "key ctrl enter 次级 动作 secondary 定位 复制网址 粘贴 paste 计算器",
        group: Group::Keys,
        source: None,
        settings_page: Some(3),
    },
    CheatEntry {
        example_key: "cheat.pick_file.example",
        title_key: "cheat.pick_file.title",
        summary_key: "cheat.pick_file.summary",
        detail_key: "cheat.pick_file.detail",
        tags: "key 弹窗 dialog 选文件 浏览 browse 拖入 drop lnk 路径",
        group: Group::Keys,
        source: None,
        settings_page: None,
    },
    CheatEntry {
        example_key: "cheat.pick_dir.example",
        title_key: "cheat.pick_dir.title",
        summary_key: "cheat.pick_dir.summary",
        detail_key: "cheat.pick_dir.detail",
        tags: "key 弹窗 dialog 选目录 选文件夹 目录 directory 浏览",
        group: Group::Keys,
        source: None,
        settings_page: None,
    },
    CheatEntry {
        example_key: "cheat.new_item.example",
        title_key: "cheat.new_item.title",
        summary_key: "cheat.new_item.summary",
        detail_key: "cheat.new_item.detail",
        tags: "key 新建 添加 add new",
        group: Group::Keys,
        source: None,
        settings_page: None,
    },
    CheatEntry {
        example_key: "cheat.edit.example",
        title_key: "cheat.edit.title",
        summary_key: "cheat.edit.summary",
        detail_key: "cheat.edit.detail",
        tags: "key f3 编辑 edit",
        group: Group::Keys,
        source: None,
        settings_page: None,
    },
    CheatEntry {
        example_key: "cheat.delete.example",
        title_key: "cheat.delete.title",
        summary_key: "cheat.delete.summary",
        detail_key: "cheat.delete.detail",
        tags: "key 删除 delete 移除",
        group: Group::Keys,
        source: None,
        settings_page: None,
    },
    CheatEntry {
        example_key: "cheat.open_dir.example",
        title_key: "cheat.open_dir.title",
        summary_key: "cheat.open_dir.summary",
        detail_key: "cheat.open_dir.detail",
        tags: "key 目录 explorer 资源管理器 定位",
        group: Group::Keys,
        source: None,
        settings_page: None,
    },
    CheatEntry {
        example_key: "cheat.pin.example",
        title_key: "cheat.pin.title",
        summary_key: "cheat.pin.summary",
        detail_key: "cheat.pin.detail",
        tags: "key 置顶 pin 取消",
        group: Group::Keys,
        source: None,
        settings_page: None,
    },
    CheatEntry {
        example_key: "cheat.file_mode.example",
        title_key: "cheat.file_mode.title",
        summary_key: "cheat.file_mode.summary",
        detail_key: "cheat.file_mode.detail",
        tags: "key file 文件模式",
        group: Group::Keys,
        source: None,
        settings_page: None,
    },
    CheatEntry {
        example_key: "cheat.import_all.example",
        title_key: "cheat.import_all.title",
        summary_key: "cheat.import_all.summary",
        detail_key: "cheat.import_all.detail",
        tags: "key 导入 import 批量",
        group: Group::Keys,
        source: None,
        settings_page: None,
    },
    CheatEntry {
        example_key: "cheat.scope_toggle.example",
        title_key: "cheat.scope_toggle.title",
        summary_key: "cheat.scope_toggle.summary",
        detail_key: "cheat.scope_toggle.detail",
        tags: "key scope 分类 范围 全部 基本 切换",
        group: Group::Keys,
        source: None,
        settings_page: Some(1),
    },
    CheatEntry {
        example_key: "cheat.hide.example",
        title_key: "cheat.hide.title",
        summary_key: "cheat.hide.summary",
        detail_key: "cheat.hide.detail",
        tags: "key 隐藏 hide hidden 条目 扫描 恢复",
        group: Group::Keys,
        source: None,
        settings_page: Some(7),
    },
    CheatEntry {
        example_key: "cheat.large_type.example",
        title_key: "cheat.large_type.title",
        summary_key: "cheat.large_type.summary",
        detail_key: "cheat.large_type.detail",
        tags: "key 大字 放大 展示 large type",
        group: Group::Keys,
        source: None,
        settings_page: None,
    },
    CheatEntry {
        example_key: "cheat.copy_line.example",
        title_key: "cheat.copy_line.title",
        summary_key: "cheat.copy_line.summary",
        detail_key: "cheat.copy_line.detail",
        tags: "key ctrl c 复制 copy 剪贴板",
        group: Group::Keys,
        source: None,
        settings_page: None,
    },
    CheatEntry {
        example_key: "cheat.alt_enter.example",
        title_key: "cheat.alt_enter.title",
        summary_key: "cheat.alt_enter.summary",
        detail_key: "cheat.alt_enter.detail",
        tags: "key alt enter 复制 copy 剪贴板 这一行",
        group: Group::Keys,
        source: None,
        settings_page: None,
    },
    CheatEntry {
        example_key: "cheat.history.example",
        title_key: "cheat.history.title",
        summary_key: "cheat.history.summary",
        detail_key: "cheat.history.detail",
        tags: "key ctrl up down 箭头 历史 history 回填 搜索 查询",
        group: Group::Keys,
        source: None,
        settings_page: Some(1),
    },
    CheatEntry {
        example_key: "cheat.param_box.example",
        title_key: "cheat.param_box.title",
        summary_key: "cheat.param_box.summary",
        detail_key: "cheat.param_box.detail",
        tags: "key 参数 参数框 parameter prompt 历史 history 最近用过",
        group: Group::Keys,
        source: None,
        settings_page: Some(7),
    },
    CheatEntry {
        example_key: "cheat.about.example",
        title_key: "cheat.about.title",
        summary_key: "cheat.about.summary",
        detail_key: "cheat.about.detail",
        tags: "key f1 关于 about 版本",
        group: Group::Keys,
        source: None,
        settings_page: None,
    },
    CheatEntry {
        example_key: "cheat.config_dir.example",
        title_key: "cheat.config_dir.title",
        summary_key: "cheat.config_dir.summary",
        detail_key: "cheat.config_dir.detail",
        tags: "key f4 配置 文件 settings.json 目录 打开",
        group: Group::Keys,
        source: None,
        settings_page: None,
    },
    CheatEntry {
        example_key: "cheat.ctrl_backspace.example",
        title_key: "cheat.ctrl_backspace.title",
        summary_key: "cheat.ctrl_backspace.summary",
        detail_key: "cheat.ctrl_backspace.detail",
        tags: "key ctrl backspace 删词 退格 删除 输入框",
        group: Group::Keys,
        source: None,
        settings_page: None,
    },
    CheatEntry {
        example_key: "cheat.settings_page.example",
        title_key: "cheat.settings_page.title",
        summary_key: "cheat.settings_page.summary",
        detail_key: "cheat.settings_page.detail",
        tags: "key f2 设置 settings preferences",
        group: Group::Keys,
        source: None,
        settings_page: None,
    },
    CheatEntry {
        example_key: "cheat.menu.example",
        title_key: "cheat.menu.title",
        summary_key: "cheat.menu.summary",
        detail_key: "cheat.menu.detail",
        tags: "key 菜单 menu 新建 设置 hamburger",
        group: Group::Keys,
        source: None,
        settings_page: None,
    },
    CheatEntry {
        example_key: "cheat.manage.example",
        title_key: "cheat.manage.title",
        summary_key: "cheat.manage.summary",
        detail_key: "cheat.manage.detail",
        tags: "key 批量 batch 管理 manage 删除 相对路径 绝对路径 失效",
        group: Group::Keys,
        source: None,
        settings_page: Some(8),
    },
    CheatEntry {
        example_key: "cheat.manage_recheck.example",
        title_key: "cheat.manage_recheck.title",
        summary_key: "cheat.manage_recheck.summary",
        detail_key: "cheat.manage_recheck.detail",
        tags: "key 校验 check 刷新 路径 失效",
        group: Group::Keys,
        source: None,
        settings_page: Some(8),
    },
    CheatEntry {
        example_key: "cheat.usage.example",
        title_key: "cheat.usage.title",
        summary_key: "cheat.usage.summary",
        detail_key: "cheat.usage.detail",
        tags: "key 统计 stats usage 使用次数 图表 使用频率",
        group: Group::Keys,
        source: None,
        settings_page: Some(9),
    },
    CheatEntry {
        example_key: "cheat.web_engines.example",
        title_key: "cheat.web_engines.title",
        summary_key: "cheat.web_engines.summary",
        detail_key: "cheat.web_engines.detail",
        tags: "key 网页 引擎 搜索 engine 设置 settings 兜底",
        group: Group::Keys,
        source: None,
        settings_page: Some(10),
    },
    CheatEntry {
        example_key: "cheat.global_hotkey.example",
        title_key: "cheat.global_hotkey.title",
        summary_key: "cheat.global_hotkey.summary",
        detail_key: "cheat.global_hotkey.detail",
        tags: "key hotkey 热键 呼出 全局 typeback",
        group: Group::Keys,
        source: None,
        settings_page: Some(3),
    },
    CheatEntry {
        example_key: "cheat.extra_hotkey.example",
        title_key: "cheat.extra_hotkey.title",
        summary_key: "cheat.extra_hotkey.summary",
        detail_key: "cheat.extra_hotkey.detail",
        tags: "key hotkey 热键 呼出 副 次 extra",
        group: Group::Keys,
        source: None,
        settings_page: Some(3),
    },
    CheatEntry {
        example_key: "cheat.dbltap.example",
        title_key: "cheat.dbltap.title",
        summary_key: "cheat.dbltap.summary",
        detail_key: "cheat.dbltap.detail",
        tags: "key 双击 呼出 double tap ctrl shift",
        group: Group::Keys,
        source: None,
        settings_page: Some(3),
    },
    CheatEntry {
        example_key: "cheat.selection_panel.example",
        title_key: "cheat.selection_panel.title",
        summary_key: "cheat.selection_panel.summary",
        detail_key: "cheat.selection_panel.detail",
        tags: "key 选中 选区 selection 动作面板 ctrl alt",
        group: Group::Keys,
        source: None,
        settings_page: Some(3),
    },
    CheatEntry {
        example_key: "cheat.move_window.example",
        title_key: "cheat.move_window.title",
        summary_key: "cheat.move_window.summary",
        detail_key: "cheat.move_window.detail",
        tags: "mouse 拖动 移动 窗口 drag",
        group: Group::Window,
        source: None,
        settings_page: None,
    },
    CheatEntry {
        example_key: "cheat.wheel.example",
        title_key: "cheat.wheel.title",
        summary_key: "cheat.wheel.summary",
        detail_key: "cheat.wheel.detail",
        tags: "mouse 滚轮 滚动 wheel scroll 选中",
        group: Group::Window,
        source: None,
        settings_page: None,
    },
    CheatEntry {
        example_key: "cheat.resize.example",
        title_key: "cheat.resize.title",
        summary_key: "cheat.resize.summary",
        detail_key: "cheat.resize.detail",
        tags: "mouse 缩放 改变大小 resize zoom",
        group: Group::Window,
        source: None,
        settings_page: None,
    },
    CheatEntry {
        example_key: "cheat.row_density.example",
        title_key: "cheat.row_density.title",
        summary_key: "cheat.row_density.summary",
        detail_key: "cheat.row_density.detail",
        tags: "行高 图标 密度 高度 间距 列表 density row theme spacing",
        group: Group::Window,
        source: None,
        settings_page: Some(1),
    }
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
        entry.example(),
        entry.title(),
        entry.summary(),
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

/// 过滤词一条也没命中时的替换建议（Toast 与提示行副标题用同一句）。
pub fn no_match_toast() -> &'static str {
    i18n::t("cheat.no_match_toast")
}

impl CheatHint {
    pub fn empty(filter: &str) -> Self {
        // 过滤词就是查询串的一部分：它只进界面，不进日志。
        Self {
            title: i18n::tf("cheat.no_match_title", &[filter]),
            subtitle: i18n::t("cheat.no_match_subtitle"),
        }
    }
}

pub struct CheatSource;

impl DataSource for CheatSource {
    fn id(&self) -> SourceId {
        SourceId::Cheatsheet
    }

    fn label(&self) -> &'static str {
        i18n::t("source.cheatsheet.label")
    }

    fn note(&self) -> &'static str {
        i18n::t("source.cheatsheet.note")
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
        let mut keys: Vec<&str> = ENTRIES.iter().map(|e| e.example()).collect();
        let n = keys.len();
        keys.sort_unstable();
        keys.dedup();
        assert_eq!(keys.len(), n, "示例写法重样了，用户分不清是哪一条");
        for e in ENTRIES {
            for lang in [i18n::Language::ZhCn, i18n::Language::En] {
                for key in [e.example_key, e.title_key, e.summary_key, e.detail_key] {
                    let text = i18n::tr(lang, key);
                    assert_ne!(text, key, "{key} 在 {} 里缺键", lang.tag());
                    assert!(!text.is_empty(), "{key} 在 {} 里是空串", lang.tag());
                }
            }
            assert!(
                !e.tags.is_empty(),
                "「{}」没有过滤词，`? 关键词` 搜不到它",
                e.title()
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
                    e.title()
                );
            }
            // 列表与帮助页都是纯 `ui.label`，不解析 Markdown：`**粗体**` 会原样带着星号出场
            //（2026-10-23 真机截图里就是这么露的）。tags 不显示，但也一并扫。
            let mut texts = vec![e.tags];
            for key in [e.example_key, e.title_key, e.summary_key, e.detail_key] {
                for lang in [i18n::Language::ZhCn, i18n::Language::En] {
                    texts.push(i18n::tr(lang, key));
                }
            }
            for text in texts {
                assert!(
                    !text.contains("**"),
                    "「{}」的文案里有 Markdown 粗体标记：{text}",
                    e.title()
                );
            }
        }
    }

    /// 英文速查表里不许出现汉字：这是批次 F14 的验收口径（示例词也跟着翻译）。
    #[test]
    fn english_cheatsheet_text_has_no_chinese() {
        for e in ENTRIES {
            for key in [e.example_key, e.title_key, e.summary_key, e.detail_key] {
                let text = i18n::tr(i18n::Language::En, key);
                assert!(
                    !text.chars().any(|c| ('\u{4e00}'..='\u{9fff}').contains(&c)),
                    "键 {key} 的英文里还有汉字：{text}"
                );
            }
        }
    }

    #[test]
    fn filters_hit_what_the_acceptance_criteria_name() {
        // 需求书验收原话：「敲 `?` 能看到"空格=文件模式、`>`=终端"等条目」。
        let all = entries("", &all_on);
        assert!(
            all.iter().any(|e| e.example().contains('>')),
            "`?` 里看不到终端条目"
        );
        assert!(
            all.iter().any(|e| e.example().contains("空格")),
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
