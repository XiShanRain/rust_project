//! FR-UIX-01 / FR-UIX-02 主题系统（批次 F1，2026-10-24）。
//!
//! 主题 = 一份 JSON，键就是需求书 6.2 那 24 个；`base` 缺省 Light，递归深度上限 8。
//! 用户主题放便携目录的 `Themes\*.json`，**同名覆盖内置**；`base` 与自身同名视为
//! 「改这个内置主题」（用户 `Dark.json` 写 `"base": "Dark"` ⇒ 在**内置** Dark 上改）。
//! 用户主题文件全丢、全坏都能启动：解析失败只记日志跳过，解析链断开一律回落 Light。
//!
//! 颜色写 `RRGGBB`（需求书口径，不带 `#`；这里对带 `#` 的也宽容接受，写错的颜色
//! 保留继承来的值并记一行日志，不会让整份主题失效）。
//!
//! `System` 不是第 20 套内置主题，而是一个**选择项**：解析时读注册表
//! `HKCU\...\Themes\Personalize\AppsUseLightTheme`，在 Light / Dark 之间二选一。

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;

/// 跟随系统的选择项名（不作为内置主题名出现）。
pub const SYSTEM_THEME: &str = "System";
/// 一切继承的根。
pub const FALLBACK_THEME: &str = "Light";
/// `base` 递归深度上限（需求书 FR-UIX-01）。
pub const MAX_DEPTH: usize = 8;
/// 用户主题目录名（便携目录下）。
pub const THEMES_DIR: &str = "Themes";

/// 内置主题名（数量取开发计划 F1 那一行写的 19；需求书 E-10 只要求"不承诺 21"）。
pub const BUILTIN_NAMES: [&str; 19] = [
    "Light",
    "Dark",
    "Compact",
    "Modern",
    "Nord",
    "Solarized Light",
    "Solarized Dark",
    "Gruvbox Light",
    "Gruvbox Dark",
    "Dracula",
    "One Dark",
    "Tokyo Night",
    "Monokai",
    "Catppuccin Latte",
    "Catppuccin Mocha",
    "GitHub Light",
    "GitHub Dark",
    "High Contrast",
    "Sepia",
];

/// 落盘的原始形态：每个键都是 `Option`，`None` = 继承 `base`。
#[derive(Debug, Clone, Default, serde::Deserialize, serde::Serialize)]
pub struct ThemeFile {
    pub base: Option<String>,
    pub font_name: Option<String>,
    pub input_font_size: Option<f32>,
    pub title_font_size: Option<f32>,
    pub subtitle_font_size: Option<f32>,
    pub shortcut_font_size: Option<f32>,
    pub padding: Option<f32>,
    pub row_height: Option<f32>,
    pub icon_size: Option<f32>,
    pub selected_radius: Option<f32>,
    pub opacity: Option<u8>,
    pub background: Option<String>,
    pub border: Option<String>,
    pub input_text: Option<String>,
    pub separator: Option<String>,
    pub title: Option<String>,
    pub subtitle: Option<String>,
    pub shortcut: Option<String>,
    pub selected_background: Option<String>,
    pub selected_title: Option<String>,
    pub selected_subtitle: Option<String>,
    pub selected_shortcut: Option<String>,
    pub highlight: Option<String>,
    pub selected_highlight: Option<String>,
}

/// 解析完成的一套主题：所有键都有值。
#[derive(Debug, Clone, PartialEq)]
pub struct Theme {
    pub name: String,
    pub font_name: String,
    pub input_font_size: f32,
    pub title_font_size: f32,
    pub subtitle_font_size: f32,
    pub shortcut_font_size: f32,
    pub padding: f32,
    pub row_height: f32,
    pub icon_size: f32,
    pub selected_radius: f32,
    pub opacity: u8,
    pub background: egui::Color32,
    pub border: egui::Color32,
    pub input_text: egui::Color32,
    pub separator: egui::Color32,
    pub title: egui::Color32,
    pub subtitle: egui::Color32,
    pub shortcut: egui::Color32,
    pub selected_background: egui::Color32,
    pub selected_title: egui::Color32,
    pub selected_subtitle: egui::Color32,
    pub selected_shortcut: egui::Color32,
    pub highlight: egui::Color32,
    pub selected_highlight: egui::Color32,
}

impl Theme {
    /// 需求书 6.2 的 Light 基准值。一切主题都从它开始叠加。
    pub fn light() -> Self {
        Self {
            name: FALLBACK_THEME.to_string(),
            font_name: "auto".to_string(),
            // 批次 F6（2026-10-24 用户：「输入框大一点，像 ahk 版的输入框一样，大气一点」）：
            // 6.2 原表这条是 20（＝AHK 版 `InputFontSize` 的 20pt），按用户点名的口径抬到 22。
            // 这是本项对 AHK 参考**有意超出**的一档，需求书 6.2 里带日期注记跟着改了。
            // 只动基准值：自己写了 `input_font_size` 的主题（如 Compact 的 18）照旧覆盖它。
            input_font_size: 22.0,
            title_font_size: 13.0,
            subtitle_font_size: 9.5,
            shortcut_font_size: 9.0,
            padding: 14.0,
            row_height: 54.0,
            icon_size: 32.0,
            selected_radius: 0.0,
            opacity: 255,
            background: egui::Color32::from_rgb(0xFA, 0xFA, 0xFA),
            border: egui::Color32::from_rgb(0xC8, 0xC8, 0xC8),
            input_text: egui::Color32::from_rgb(0x1F, 0x1F, 0x1F),
            separator: egui::Color32::from_rgb(0xE4, 0xE4, 0xE4),
            title: egui::Color32::from_rgb(0x1F, 0x1F, 0x1F),
            subtitle: egui::Color32::from_rgb(0x80, 0x80, 0x80),
            shortcut: egui::Color32::from_rgb(0xA0, 0xA0, 0xA0),
            selected_background: egui::Color32::from_rgb(0xDD, 0xE7, 0xF6),
            selected_title: egui::Color32::from_rgb(0x00, 0x00, 0x00),
            selected_subtitle: egui::Color32::from_rgb(0x4A, 0x55, 0x68),
            selected_shortcut: egui::Color32::from_rgb(0x4A, 0x55, 0x68),
            highlight: egui::Color32::from_rgb(0x00, 0x5F, 0xE0),
            selected_highlight: egui::Color32::from_rgb(0x00, 0x47, 0xB8),
        }
    }

    /// 把一份（可能只写了几个键的）主题文件叠上来。颜色写错＝保留继承值。
    fn overlay(&mut self, f: &ThemeFile) {
        if let Some(v) = &f.font_name {
            self.font_name = v.clone();
        }
        macro_rules! num {
            ($field:ident, $min:expr) => {
                if let Some(v) = f.$field {
                    self.$field = v.max($min);
                }
            };
        }
        num!(input_font_size, 6.0);
        num!(title_font_size, 6.0);
        num!(subtitle_font_size, 6.0);
        num!(shortcut_font_size, 6.0);
        num!(padding, 0.0);
        num!(row_height, 12.0);
        num!(icon_size, 8.0);
        num!(selected_radius, 0.0);
        if let Some(v) = f.opacity {
            self.opacity = v;
        }
        macro_rules! color {
            ($field:ident) => {
                if let Some(s) = &f.$field {
                    match parse_hex(s) {
                        Some(c) => self.$field = c,
                        None => eprintln!(
                            "theme: 「{}」的 {} = {:?} 不是 RRGGBB，保留继承值 {:02X}{:02X}{:02X}",
                            self.name,
                            stringify!($field),
                            s,
                            self.$field.r(),
                            self.$field.g(),
                            self.$field.b()
                        ),
                    }
                }
            };
        }
        color!(background);
        color!(border);
        color!(input_text);
        color!(separator);
        color!(title);
        color!(subtitle);
        color!(shortcut);
        color!(selected_background);
        color!(selected_title);
        color!(selected_subtitle);
        color!(selected_shortcut);
        color!(highlight);
        color!(selected_highlight);
    }

    /// 深色判定：`(299R + 587G + 114B) / 1000 < 128`（需求书 6.2 末行）。
    pub fn is_dark(&self) -> bool {
        let c = self.background;
        (299 * c.r() as u32 + 587 * c.g() as u32 + 114 * c.b() as u32) / 1000 < 128
    }

    /// 窗口底色带 `opacity`（255 = 不透明）。GL 后端下 alpha 会被 DWM 真正合成。
    pub fn window_fill(&self) -> egui::Color32 {
        let c = self.background;
        egui::Color32::from_rgba_unmultiplied(c.r(), c.g(), c.b(), self.opacity)
    }

    /// 搜索框 / 文本输入底色：在背景与边框之间取一点，深浅两套都能看出边界。
    pub fn input_fill(&self) -> egui::Color32 {
        mix(self.background, self.border, 0.14)
    }

    /// egui 控件的全局配色。两侧样式表都写同一份（见 [`Theme::apply`]），
    /// 这样系统明暗切换不会把已经选好的主题顶掉。
    pub fn visuals(&self) -> egui::Visuals {
        let mut v = if self.is_dark() {
            egui::Visuals::dark()
        } else {
            egui::Visuals::light()
        };
        v.dark_mode = self.is_dark();
        v.panel_fill = self.background;
        v.window_fill = self.background;
        v.window_stroke = egui::Stroke::new(1.0, self.border);
        v.window_corner_radius = egui::CornerRadius::same(WINDOW_RADIUS);
        v.extreme_bg_color = self.input_fill();
        v.text_edit_bg_color = Some(self.input_fill());
        v.faint_bg_color = mix(self.background, self.border, 0.10);
        v.code_bg_color = mix(self.background, self.border, 0.16);
        // 不设 `override_text_color`：它会把 `.weak()` 的副标题也压成同一种颜色，
        // 六键里的 subtitle/shortcut/selected_* 就没了出口。各控件状态各自给色。
        v.weak_text_color = Some(self.subtitle);
        v.hyperlink_color = self.highlight;
        v.selection.bg_fill = self.selected_background;
        v.selection.stroke = egui::Stroke::new(1.0, self.selected_title);
        v.warn_fg_color = self.highlight;
        v.error_fg_color = egui::Color32::from_rgb(0xD1, 0x3B, 0x3B);

        let radius = egui::CornerRadius::same(self.selected_radius.round().clamp(0.0, 255.0) as u8);
        let border = egui::Stroke::new(1.0, self.border);
        v.widgets.noninteractive.bg_fill = self.background;
        v.widgets.noninteractive.weak_bg_fill = self.background;
        v.widgets.noninteractive.bg_stroke = border;
        v.widgets.noninteractive.fg_stroke = egui::Stroke::new(1.0, self.title);
        v.widgets.noninteractive.corner_radius = radius;

        v.widgets.inactive.bg_fill = mix(self.background, self.border, 0.12);
        v.widgets.inactive.weak_bg_fill = egui::Color32::TRANSPARENT;
        v.widgets.inactive.bg_stroke = border;
        v.widgets.inactive.fg_stroke = egui::Stroke::new(1.0, self.input_text);
        v.widgets.inactive.corner_radius = radius;

        v.widgets.hovered.bg_fill = mix(self.background, self.selected_background, 0.55);
        v.widgets.hovered.weak_bg_fill = mix(self.background, self.selected_background, 0.35);
        v.widgets.hovered.bg_stroke = egui::Stroke::new(1.0, self.highlight);
        v.widgets.hovered.fg_stroke = egui::Stroke::new(1.0, self.title);
        v.widgets.hovered.corner_radius = radius;

        v.widgets.active.bg_fill = self.selected_background;
        v.widgets.active.weak_bg_fill = self.selected_background;
        v.widgets.active.bg_stroke = egui::Stroke::new(1.0, self.selected_highlight);
        v.widgets.active.fg_stroke = egui::Stroke::new(1.0, self.selected_title);
        v.widgets.active.corner_radius = radius;

        v.widgets.open.bg_fill = mix(self.background, self.border, 0.16);
        v.widgets.open.weak_bg_fill = mix(self.background, self.border, 0.16);
        v.widgets.open.bg_stroke = border;
        v.widgets.open.fg_stroke = egui::Stroke::new(1.0, self.title);
        v.widgets.open.corner_radius = radius;
        v
    }

    /// 字号表：Body/Button 用 `title_font_size`，Small 用副标题字号，等宽用快捷键字号。
    /// 需求书 6.2 的 `input_font_size` 不在这里——它只给搜索框（TextEdit 自己带字体）。
    pub fn text_styles(&self) -> BTreeMap<egui::TextStyle, egui::FontId> {
        let mut m = BTreeMap::new();
        m.insert(
            egui::TextStyle::Small,
            egui::FontId::proportional(self.subtitle_font_size),
        );
        m.insert(
            egui::TextStyle::Body,
            egui::FontId::proportional(self.title_font_size),
        );
        m.insert(
            egui::TextStyle::Button,
            egui::FontId::proportional(self.title_font_size),
        );
        m.insert(
            egui::TextStyle::Monospace,
            egui::FontId::monospace(self.shortcut_font_size),
        );
        m.insert(
            egui::TextStyle::Heading,
            egui::FontId::proportional(self.title_font_size + 4.0),
        );
        m
    }

    /// 主窗口的圆角外框：底色（带 opacity）+ 边框色 + 固定圆角 + 主题内边距。
    pub fn central_frame(&self) -> egui::Frame {
        egui::Frame::new()
            .fill(self.window_fill())
            .stroke(egui::Stroke::new(1.0, self.border))
            .corner_radius(egui::CornerRadius::same(WINDOW_RADIUS))
            .inner_margin(egui::Margin::same(
                self.padding.round().clamp(0.0, 127.0) as i8
            ))
    }

    /// 装进 egui：两套主题样式都写同一份，避免系统明暗切换时观感跳变。
    pub fn apply(&self, ctx: &egui::Context) {
        let theme = if self.is_dark() {
            egui::Theme::Dark
        } else {
            egui::Theme::Light
        };
        ctx.set_theme(theme);
        let visuals = self.visuals();
        let text_styles = self.text_styles();
        ctx.all_styles_mut(|style| {
            style.visuals = visuals.clone();
            style.text_styles = text_styles.clone();
        });
    }
}

/// 窗口圆角：需求书 6.2 没有给圆角键（`selected_radius` 是选中行用的），
/// FR-UIX-06 只说"圆角"，所以取一个固定值。
const WINDOW_RADIUS: u8 = 10;

/// `RRGGBB`（也接受 `#RRGGBB`）→ 颜色。位宽不对一律 `None`。
pub fn parse_hex(s: &str) -> Option<egui::Color32> {
    let s = s.trim().trim_start_matches('#');
    if s.len() != 6 || !s.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let v = u32::from_str_radix(s, 16).ok()?;
    Some(egui::Color32::from_rgb(
        (v >> 16) as u8,
        (v >> 8) as u8,
        v as u8,
    ))
}

fn mix(a: egui::Color32, b: egui::Color32, t: f32) -> egui::Color32 {
    let t = t.clamp(0.0, 1.0);
    let f = |x: u8, y: u8| (x as f32 * (1.0 - t) + y as f32 * t).round() as u8;
    egui::Color32::from_rgb(f(a.r(), b.r()), f(a.g(), b.g()), f(a.b(), b.b()))
}

// ---------------------------------------------------------------- 内置主题

fn hex(s: &str) -> Option<String> {
    Some(s.to_string())
}

/// 13 个颜色键的紧凑写法（顺序见 [`Pal::apply`]）。
struct Pal([&'static str; 13]);

impl Pal {
    fn file(self, base: Option<&str>) -> ThemeFile {
        let [
            background,
            border,
            input_text,
            separator,
            title,
            subtitle,
            shortcut,
            selected_background,
            selected_title,
            selected_subtitle,
            selected_shortcut,
            highlight,
            selected_highlight,
        ] = self.0;
        ThemeFile {
            base: base.map(str::to_string),
            background: hex(background),
            border: hex(border),
            input_text: hex(input_text),
            separator: hex(separator),
            title: hex(title),
            subtitle: hex(subtitle),
            shortcut: hex(shortcut),
            selected_background: hex(selected_background),
            selected_title: hex(selected_title),
            selected_subtitle: hex(selected_subtitle),
            selected_shortcut: hex(selected_shortcut),
            highlight: hex(highlight),
            selected_highlight: hex(selected_highlight),
            ..Default::default()
        }
    }
}

const LIGHT_PAL: Pal = Pal([
    "FAFAFA", "C8C8C8", "1F1F1F", "E4E4E4", "1F1F1F", "808080", "A0A0A0", "DDE7F6", "000000",
    "4A5568", "4A5568", "005FE0", "0047B8",
]);

fn builtin_file(name: &str) -> Option<ThemeFile> {
    let f = match name {
        "Light" => ThemeFile::default(),
        "Dark" => Pal([
            "1F1F1F", "3A3A3A", "E8E8E8", "333333", "E8E8E8", "9A9A9A", "787878", "2F4463",
            "FFFFFF", "C9D4E2", "B8C4D4", "6BB0FF", "9ECBFF",
        ])
        .file(Some("Light")),
        "Compact" => {
            let mut f = LIGHT_PAL.file(Some("Light"));
            f.padding = Some(10.0);
            f.row_height = Some(44.0);
            f.icon_size = Some(26.0);
            f.input_font_size = Some(18.0);
            f.title_font_size = Some(12.0);
            f.subtitle_font_size = Some(8.5);
            f.shortcut_font_size = Some(8.5);
            f.selected_radius = Some(4.0);
            f
        }
        "Modern" => {
            let mut f = Pal([
                "FFFFFF", "D0D7E2", "202124", "E8EAED", "202124", "5F6368", "80868B", "E8F0FE",
                "174EA6", "1967D2", "1967D2", "1A73E8", "185ABC",
            ])
            .file(Some("Light"));
            f.padding = Some(16.0);
            f.row_height = Some(56.0);
            f.selected_radius = Some(8.0);
            f
        }
        "Nord" => Pal([
            "2E3440", "4C566A", "ECEFF4", "3B4252", "ECEFF4", "A3ABB8", "7B8595", "434C5E",
            "FFFFFF", "D8DEE9", "C0C8D4", "88C0D0", "8FBCBB",
        ])
        .file(Some("Light")),
        "Solarized Light" => Pal([
            "FDF6E3", "E0D8C3", "586E75", "EEE8D5", "073642", "93A1A1", "93A1A1", "EEE8D5",
            "002B36", "586E75", "657B83", "268BD2", "1A6FA8",
        ])
        .file(Some("Light")),
        "Solarized Dark" => Pal([
            "002B36", "0B3B47", "93A1A1", "073642", "FDF6E3", "839496", "657B83", "073642",
            "FDF6E3", "93A1A1", "839496", "268BD2", "6FC1E8",
        ])
        .file(Some("Light")),
        "Gruvbox Light" => Pal([
            "FBF1C7", "D5C4A1", "3C3836", "E5D5AD", "3C3836", "7C6F64", "A89984", "EBDBB2",
            "282828", "504945", "665C54", "AF3A03", "9D0006",
        ])
        .file(Some("Light")),
        "Gruvbox Dark" => Pal([
            "282828", "504945", "EBDBB2", "3C3836", "EBDBB2", "A89984", "928374", "3C3836",
            "FBF1C7", "D5C4A1", "BDAE93", "FABD2F", "FE8019",
        ])
        .file(Some("Light")),
        "Dracula" => Pal([
            "282A36", "44475A", "F8F8F2", "383A48", "F8F8F2", "B0B4C8", "6272A4", "44475A",
            "FFFFFF", "D0D3E0", "9AA0C0", "8BE9FD", "50FA7B",
        ])
        .file(Some("Light")),
        "One Dark" => Pal([
            "282C34", "3E4451", "ABB2BF", "353B45", "D7DAE0", "8B93A1", "636D83", "3E4451",
            "FFFFFF", "C8CDD6", "9DA5B4", "61AFEF", "98C379",
        ])
        .file(Some("Light")),
        "Tokyo Night" => Pal([
            "1A1B26", "2F3549", "C0CAF5", "232433", "C0CAF5", "7B85A8", "565F89", "2F3549",
            "FFFFFF", "C0CAF5", "A9B1D6", "7AA2F7", "BB9AF7",
        ])
        .file(Some("Light")),
        "Monokai" => Pal([
            "272822", "49483E", "F8F8F2", "3B3A32", "F8F8F2", "9E9D92", "75715E", "49483E",
            "FFFFFF", "D8D8CF", "B8B8A8", "66D9EF", "A6E22E",
        ])
        .file(Some("Light")),
        "Catppuccin Latte" => Pal([
            "EFF1F5", "CCD0DA", "4C4F69", "DCE0E8", "4C4F69", "8C8FA1", "9CA0B0", "DDE1EA",
            "303446", "5C5F77", "6C6F85", "1E66F5", "04A5E5",
        ])
        .file(Some("Light")),
        "Catppuccin Mocha" => Pal([
            "1E1E2E", "45475A", "CDD6F4", "313244", "CDD6F4", "A6ADC8", "7F849C", "45475A",
            "FFFFFF", "CDD6F4", "BAC2DE", "89B4FA", "CBA6F7",
        ])
        .file(Some("Light")),
        "GitHub Light" => Pal([
            "FFFFFF", "D0D7DE", "1F2328", "D8DEE4", "1F2328", "656D76", "8C959F", "DDF4FF",
            "1F2328", "57606A", "6E7781", "0969DA", "0550AE",
        ])
        .file(Some("Light")),
        "GitHub Dark" => Pal([
            "0D1117", "30363D", "E6EDF3", "21262D", "E6EDF3", "8B949E", "6E7681", "163356",
            "FFFFFF", "C9D1D9", "A9B3BF", "58A6FF", "79C0FF",
        ])
        .file(Some("Light")),
        "High Contrast" => Pal([
            "FFFFFF", "000000", "000000", "000000", "000000", "303030", "303030", "000000",
            "FFFFFF", "F0F0F0", "E0E0E0", "0000CC", "FFFF00",
        ])
        .file(Some("Light")),
        "Sepia" => Pal([
            "F4ECD8", "D8C9A5", "4A3B2A", "E2D5B8", "4A3B2A", "7A6A52", "9A8A70", "E2D5B8",
            "2E2418", "5A4A36", "6E5C44", "8B5E00", "6B4A00",
        ])
        .file(Some("Light")),
        _ => return None,
    };
    Some(f)
}

// ---------------------------------------------------------------- 解析与加载

/// 便携目录下的 `Themes\` 目录。
pub fn user_dir() -> PathBuf {
    crate::source::application::data_dir().join(THEMES_DIR)
}

/// 读用户主题目录。坏文件只记日志跳过——主题问题不许挡住启动。
pub fn load_user_files(dir: &std::path::Path) -> HashMap<String, ThemeFile> {
    let mut out = HashMap::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return out;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path
            .extension()
            .and_then(|e| e.to_str())
            .is_none_or(|e| !e.eq_ignore_ascii_case("json"))
        {
            continue;
        }
        let Some(name) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        let text = match std::fs::read_to_string(&path) {
            Ok(t) => t,
            Err(e) => {
                eprintln!("theme: 主题 {:?} 读不出来（{e}），忽略", path.file_name());
                continue;
            }
        };
        match serde_json::from_str::<ThemeFile>(&text) {
            Ok(f) => {
                out.insert(name.to_string(), f);
            }
            Err(e) => eprintln!("theme: 主题「{name}」解析失败（{e}），已忽略"),
        }
    }
    out
}

/// 系统是否深色：`AppsUseLightTheme` 是 DWORD，0 = 深色。
/// 读不到（旧系统、权限）按浅色处理——与 `Light` 基准一致。
pub fn system_is_dark() -> bool {
    use windows::Win32::Foundation::ERROR_SUCCESS;
    use windows::Win32::System::Registry::{HKEY_CURRENT_USER, RRF_RT_REG_DWORD, RegGetValueW};
    let subkey = "Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize";
    let subkey: Vec<u16> = subkey.encode_utf16().chain(std::iter::once(0)).collect();
    let value: Vec<u16> = "AppsUseLightTheme"
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    let mut data = 1u32;
    let mut size = std::mem::size_of::<u32>() as u32;
    let rc = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            windows::core::PCWSTR(subkey.as_ptr()),
            windows::core::PCWSTR(value.as_ptr()),
            RRF_RT_REG_DWORD,
            None,
            Some(&mut data as *mut u32 as *mut core::ffi::c_void),
            Some(&mut size),
        )
    };
    rc == ERROR_SUCCESS && data == 0
}

/// 启动 / 每次呼出时用：读一遍磁盘，同时给出解析结果与下拉选项。
/// 两者原先各读一次 `Themes\`——坏文件会在日志里出现两遍，改为共用同一份。
pub fn load(selection: &str) -> (Theme, Vec<String>) {
    let files = load_user_files(&user_dir());
    (
        resolve_with(selection, &files, system_is_dark()),
        all_names(&files),
    )
}

/// 纯解析（单测走这条，不碰磁盘与注册表）。
pub fn resolve_with(
    selection: &str,
    files: &HashMap<String, ThemeFile>,
    system_dark: bool,
) -> Theme {
    let name = if selection == SYSTEM_THEME {
        if system_dark { "Dark" } else { "Light" }
    } else {
        selection
    };
    let mut t = resolve_name(name, files, 0, &mut Vec::new());
    t.name = if selection == SYSTEM_THEME {
        SYSTEM_THEME.to_string()
    } else {
        t.name
    };
    t
}

fn resolve_name(
    name: &str,
    files: &HashMap<String, ThemeFile>,
    depth: usize,
    stack: &mut Vec<String>,
) -> Theme {
    let mut theme = Theme::light();
    let file = files.get(name).cloned().or_else(|| builtin_file(name));
    let Some(file) = file else {
        if name != FALLBACK_THEME {
            eprintln!("theme: 找不到主题「{name}」，按 {FALLBACK_THEME} 处理");
        }
        return theme;
    };
    if depth >= MAX_DEPTH || stack.iter().any(|s| s == name) {
        eprintln!(
            "theme: 继承链在「{name}」这里断开（深度 {depth} / 成环），按 {FALLBACK_THEME} 处理"
        );
        return theme;
    }
    stack.push(name.to_string());
    // 「base 与自身同名」= 改这个内置主题 ⇒ 先解析内置版本自己的 base，再叠内置值，
    // 最后叠用户文件。不能只叠用户文件：用户没写的键必须仍是内置主题的值，
    // 而不是落回 Light（否则「Dark.json 写 base:Dark + 一个背景色」会丢掉整套 Dark 配色）。
    if file.base.as_deref() == Some(name) {
        let builtin = builtin_file(name).unwrap_or_default();
        let builtin_base = builtin
            .base
            .clone()
            .unwrap_or_else(|| FALLBACK_THEME.to_string());
        if name != FALLBACK_THEME {
            theme = resolve_name(&builtin_base, files, depth + 1, stack);
        }
        theme.overlay(&builtin);
        theme.overlay(&file);
        theme.name = name.to_string();
        stack.pop();
        return theme;
    }
    let base_name = file
        .base
        .clone()
        .unwrap_or_else(|| FALLBACK_THEME.to_string());
    if name != FALLBACK_THEME {
        theme = resolve_name(&base_name, files, depth + 1, stack);
    }
    theme.overlay(&file);
    theme.name = name.to_string();
    stack.pop();
    theme
}

/// 设置页下拉用的全部选项：`System` + 19 套内置 + 用户主题（同名已被内置覆盖，不重复列）。
fn all_names(files: &HashMap<String, ThemeFile>) -> Vec<String> {
    let mut names = vec![SYSTEM_THEME.to_string()];
    names.extend(BUILTIN_NAMES.iter().map(|s| s.to_string()));
    let mut user: Vec<String> = files
        .keys()
        .filter(|n| !BUILTIN_NAMES.contains(&n.as_str()) && *n != SYSTEM_THEME)
        .cloned()
        .collect();
    user.sort_by_key(|a| a.to_lowercase());
    names.extend(user);
    names
}

/// 把主题字体装进 egui：`auto` = 只装 CJK 回退；其它名字先在 `C:\Windows\Fonts`
/// 里按原名 / 补扩展名找，找到就放到 Proportional 族首位，CJK 回退仍然追加在后面。
pub fn install_fonts(ctx: &egui::Context, font_name: &str) {
    let mut fonts = egui::FontDefinitions::default();
    let mut preferred = None;
    if !font_name.eq_ignore_ascii_case("auto") {
        preferred = find_font(font_name);
        if preferred.is_none() {
            eprintln!("theme: 找不到字体「{font_name}」，按 auto 处理");
        }
    }
    if let Some((label, bytes, index)) = preferred {
        let mut data = egui::FontData::from_owned(bytes);
        data.index = index;
        fonts
            .font_data
            .insert("theme".to_owned(), std::sync::Arc::new(data));
        fonts
            .families
            .entry(egui::FontFamily::Proportional)
            .or_default()
            .insert(0, "theme".to_owned());
        eprintln!("theme: 字体改用 {label}");
    }
    for (file, index) in [("msyh.ttc", 0u32), ("simhei.ttf", 0), ("STXIHEI.TTF", 0)] {
        let Ok(bytes) = std::fs::read(format!(r"C:\Windows\Fonts\{file}")) else {
            continue;
        };
        if skrifa::FontRef::from_index(&bytes, index).is_err() {
            continue;
        }
        let mut data = egui::FontData::from_owned(bytes);
        data.index = index;
        fonts
            .font_data
            .insert("cjk".to_owned(), std::sync::Arc::new(data));
        for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
            fonts
                .families
                .entry(family)
                .or_default()
                .push("cjk".to_owned());
        }
        break;
    }
    // 符号回退（2026-10-26 沙箱截图抓到）：egui 自带那三套字体与微软雅黑都没有 U+2713 `✓`
    // 和 U+2139 `ℹ`（自带的 emoji 字体只有 `⚠` 那一类），提示条里「✓ 已复制…」整行开头
    // 显示成方框。Segoe UI Symbol 这几个码位齐全（本机 skrifa 逐字问过），排在链尾——
    // 只有前面都缺字时才被问到，已有的字形一个都不变。
    if let Ok(bytes) = std::fs::read(r"C:\Windows\Fonts\seguisym.ttf") {
        if skrifa::FontRef::from_index(&bytes, 0).is_ok() {
            let mut data = egui::FontData::from_owned(bytes);
            data.index = 0;
            fonts
                .font_data
                .insert("symbol".to_owned(), std::sync::Arc::new(data));
            for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
                fonts
                    .families
                    .entry(family)
                    .or_default()
                    .push("symbol".to_owned());
            }
        }
    }
    ctx.set_fonts(fonts);
}

fn find_font(name: &str) -> Option<(String, Vec<u8>, u32)> {
    let dir = std::path::Path::new(r"C:\Windows\Fonts");
    let mut candidates = vec![dir.join(name)];
    for ext in ["ttf", "ttc", "otf"] {
        candidates.push(dir.join(format!("{name}.{ext}")));
    }
    for path in candidates {
        let Ok(bytes) = std::fs::read(&path) else {
            continue;
        };
        if skrifa::FontRef::from_index(&bytes, 0).is_err() {
            continue;
        }
        let label = path
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or(name)
            .to_string();
        return Some((label, bytes, 0));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn files(entries: &[(&str, ThemeFile)]) -> HashMap<String, ThemeFile> {
        entries
            .iter()
            .map(|(n, f)| (n.to_string(), f.clone()))
            .collect()
    }

    #[test]
    fn light_matches_the_spec_table() {
        let t = Theme::light();
        // 批次 F6：基准输入字号 20 ⇒ 22（用户点名的「比 AHK 再大气一档」，见 `Theme::light` 注释）。
        assert_eq!(t.input_font_size, 22.0);
        assert_eq!(t.title_font_size, 13.0);
        assert_eq!(t.subtitle_font_size, 9.5);
        assert_eq!(t.shortcut_font_size, 9.0);
        assert_eq!(t.padding, 14.0);
        assert_eq!(t.row_height, 54.0);
        assert_eq!(t.icon_size, 32.0);
        assert_eq!(t.selected_radius, 0.0);
        assert_eq!(t.opacity, 255);
        assert_eq!(t.background, egui::Color32::from_rgb(0xFA, 0xFA, 0xFA));
        assert_eq!(
            t.selected_highlight,
            egui::Color32::from_rgb(0x00, 0x47, 0xB8)
        );
        assert!(!t.is_dark());
    }

    #[test]
    fn nineteen_builtins_resolve_to_their_own_name() {
        assert_eq!(BUILTIN_NAMES.len(), 19);
        let none = HashMap::new();
        for name in BUILTIN_NAMES {
            let t = resolve_with(name, &none, false);
            assert_eq!(t.name, name);
            // Compact 与 Light 同色系（它的特色是尺寸），其余主题都必须有自己的底色。
            if name != "Light" && name != "Compact" {
                assert_ne!(
                    t.background,
                    Theme::light().background,
                    "「{name}」没有自己的底色"
                );
            }
        }
        let compact = resolve_with("Compact", &none, false);
        assert!(
            compact.row_height < Theme::light().row_height,
            "Compact 的行高要更小"
        );
        assert!(
            compact.icon_size < Theme::light().icon_size,
            "Compact 的图标要更小"
        );
        assert!(
            compact.padding < Theme::light().padding,
            "Compact 的内边距要更小"
        );
    }

    #[test]
    fn a_partial_theme_inherits_everything_else_from_its_base() {
        let f: ThemeFile = serde_json::from_str(
            r##"{"base":"Dark","selected_background":"1D4ED8","selected_radius":8,"opacity":240}"##,
        )
        .unwrap();
        let t = resolve_with("Mine", &files(&[("Mine", f)]), false);
        let dark = resolve_with("Dark", &HashMap::new(), false);
        assert_eq!(t.background, dark.background, "没写的颜色必须跟 base 走");
        assert_eq!(t.title, dark.title);
        assert_eq!(
            t.selected_background,
            egui::Color32::from_rgb(0x1D, 0x4E, 0xD8)
        );
        assert_eq!(t.selected_radius, 8.0);
        assert_eq!(t.opacity, 240);
        assert_eq!(t.name, "Mine");
    }

    #[test]
    fn self_base_edits_the_builtin_instead_of_looping() {
        let mut f = ThemeFile {
            base: Some("Dark".into()),
            ..Default::default()
        };
        f.background = Some("101010".into());
        let t = resolve_with("Dark", &files(&[("Dark", f)]), false);
        let builtin = resolve_with("Dark", &HashMap::new(), false);
        assert_eq!(t.background, egui::Color32::from_rgb(0x10, 0x10, 0x10));
        assert_eq!(t.title, builtin.title, "其余键仍来自内置 Dark");
    }

    #[test]
    fn a_cycle_falls_back_to_light_without_recursing_forever() {
        let a = ThemeFile {
            base: Some("B".into()),
            ..Default::default()
        };
        let b = ThemeFile {
            base: Some("A".into()),
            ..Default::default()
        };
        let t = resolve_with("A", &files(&[("A", a), ("B", b)]), false);
        assert_eq!(t.background, Theme::light().background);
    }

    #[test]
    fn depth_is_capped_at_eight() {
        // X0 ← X1 ← … ← X9；第 9 层已超上限 ⇒ 断在 Light。
        let mut list: Vec<(String, ThemeFile)> = Vec::new();
        for i in 0..10 {
            list.push((
                format!("X{i}"),
                ThemeFile {
                    base: Some(format!("X{}", i + 1)),
                    ..Default::default()
                },
            ));
        }
        let borrowed: Vec<(&str, ThemeFile)> =
            list.iter().map(|(n, f)| (n.as_str(), f.clone())).collect();
        let t = resolve_with("X0", &files(&borrowed), false);
        assert_eq!(t.background, Theme::light().background);
    }

    #[test]
    fn a_bad_color_keeps_the_inherited_value() {
        let mut f = ThemeFile::default();
        f.background = Some("GGGGGG".into());
        let t = resolve_with("Broken", &files(&[("Broken", f)]), false);
        assert_eq!(t.background, Theme::light().background);
    }

    #[test]
    fn hex_accepts_both_spellings_and_rejects_bad_widths() {
        assert_eq!(
            parse_hex("005FE0"),
            Some(egui::Color32::from_rgb(0x00, 0x5F, 0xE0))
        );
        assert_eq!(
            parse_hex("#005FE0"),
            Some(egui::Color32::from_rgb(0x00, 0x5F, 0xE0))
        );
        assert_eq!(parse_hex("005FE"), None);
        assert_eq!(parse_hex("005FE00"), None);
        assert_eq!(parse_hex("005FEG"), None);
    }

    #[test]
    fn system_picks_dark_or_light_from_the_os_flag() {
        let none = HashMap::new();
        assert_eq!(
            resolve_with(SYSTEM_THEME, &none, true).background,
            resolve_with("Dark", &none, false).background
        );
        assert_eq!(
            resolve_with(SYSTEM_THEME, &none, false).background,
            resolve_with("Light", &none, false).background
        );
    }

    #[test]
    fn an_unknown_selection_falls_back_to_light() {
        let t = resolve_with("No Such Theme", &HashMap::new(), false);
        assert_eq!(t.name, "Light");
        assert_eq!(t.background, Theme::light().background);
    }

    #[test]
    fn window_fill_carries_the_configured_opacity() {
        let mut f = ThemeFile::default();
        f.opacity = Some(200);
        let t = resolve_with("Translucent", &files(&[("Translucent", f)]), false);
        assert_eq!(t.window_fill().a(), 200);
        assert_eq!(Theme::light().window_fill().a(), 255);
    }

    #[test]
    fn dark_theme_is_recognised_by_luminance() {
        for name in [
            "Dark",
            "Nord",
            "GitHub Dark",
            "Catppuccin Mocha",
            "Tokyo Night",
        ] {
            assert!(
                resolve_with(name, &HashMap::new(), false).is_dark(),
                "「{name}」应判为深色"
            );
        }
        for name in ["Light", "Solarized Light", "GitHub Light", "Sepia"] {
            assert!(
                !resolve_with(name, &HashMap::new(), false).is_dark(),
                "「{name}」应判为浅色"
            );
        }
    }
}
