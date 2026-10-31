//! FR-UIX-12 主题缩略图画廊（批次 F11，2026-10-30）。
//!
//! 外观页的主题选择不再是一个下拉：每一格按主题自己的颜色画一只迷你搜索窗
//! （背景 / 边框 / 输入框 / 分隔线 / 选中行 / 标题 / 副标题 / 高亮段 + 彩色图标），
//! `System` 画成左浅右深。配色**全部取自解析后的 [`Theme`]**——「画廊与实窗观感一致」
//! 靠这一条，而不是另抄一份色表。
//!
//! 这里只做「画」与「量」：点中哪一格由 `main.rs` 决定（走既有的 save → log → sync_theme）。

use crate::theme::{GalleryItem, Preview, Theme};

/// 一格宽（缩略图与名字同宽）。
pub const CELL_W: f32 = 104.0;
/// 一格高：缩略图 66 + 名字 18。
pub const CELL_H: f32 = 84.0;
const THUMB_H: f32 = 66.0;
const NAME_H: f32 = CELL_H - THUMB_H;

/// 彩色图标的四色。**有意不跟主题走**：真实应用图标本来就不随皮肤改色，
/// 迷你窗里那块小方块是"这是一行带图标的结果"的记号，不是主题色板的一部分。
const ICON_COLORS: [egui::Color32; 4] = [
    egui::Color32::from_rgb(0xE8, 0x45, 0x3C),
    egui::Color32::from_rgb(0xF9, 0xAB, 0x00),
    egui::Color32::from_rgb(0x34, 0xA8, 0x53),
    egui::Color32::from_rgb(0x42, 0x85, 0xF4),
];

/// 一行结果（预览里那条"选中行"或普通行）要用的四个颜色。
/// 抽成纯函数是为了单测能钉住"取的是哪几个主题键"。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RowPalette {
    pub background: egui::Color32,
    pub title: egui::Color32,
    pub highlight: egui::Color32,
    pub subtitle: egui::Color32,
}

pub fn row_palette(t: &Theme, selected: bool) -> RowPalette {
    if selected {
        RowPalette {
            background: t.selected_background,
            title: t.selected_title,
            highlight: t.selected_highlight,
            subtitle: t.selected_subtitle,
        }
    } else {
        RowPalette {
            background: egui::Color32::TRANSPARENT,
            title: t.title,
            highlight: t.highlight,
            subtitle: t.subtitle,
        }
    }
}

/// 一行放得下几格（`avail` = 内容区可用宽，`spacing` = 格间距）。至少 1 格。
pub fn columns(avail: f32, cell_w: f32, spacing: f32) -> usize {
    if cell_w <= 0.0 {
        return 1;
    }
    let spacing = spacing.max(0.0);
    (((avail + spacing) / (cell_w + spacing)).floor() as usize).max(1)
}

/// 画一格并返回它的响应。点击与键盘激活（聚焦后 Enter / Space）由调用方判。
pub fn cell(
    ui: &mut egui::Ui,
    item: &GalleryItem,
    selected: bool,
    accent: egui::Color32,
) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(CELL_W, CELL_H), egui::Sense::click());
    if !ui.is_rect_visible(rect) {
        return resp;
    }
    let painter = ui.painter_at(rect);
    if resp.hovered() {
        painter.rect_filled(
            rect,
            egui::CornerRadius::same(6),
            ui.visuals().widgets.hovered.weak_bg_fill,
        );
    }
    let thumb = egui::Rect::from_min_size(rect.min, egui::vec2(CELL_W, THUMB_H));
    match &item.preview {
        Preview::Single(t) => paint_window(&painter, thumb, t),
        Preview::System { light, dark } => {
            // 左浅右深：与当前系统是哪一档无关（需求书 FR-UIX-12）。
            let half = (CELL_W - 3.0) / 2.0;
            let left = egui::Rect::from_min_size(thumb.min, egui::vec2(half, THUMB_H));
            let right = egui::Rect::from_min_size(
                egui::pos2(thumb.min.x + half + 3.0, thumb.min.y),
                egui::vec2(half, THUMB_H),
            );
            paint_window(&painter, left, light);
            paint_window(&painter, right, dark);
        }
    }
    let ring = if selected {
        Some(accent)
    } else if resp.has_focus() {
        Some(ui.visuals().selection.stroke.color)
    } else {
        None
    };
    if let Some(color) = ring {
        painter.rect_stroke(
            thumb,
            egui::CornerRadius::same(5),
            egui::Stroke::new(2.0, color),
            egui::StrokeKind::Inside,
        );
    }
    paint_name(
        &painter,
        rect,
        &item.name,
        selected,
        accent,
        ui.visuals().text_color(),
    );
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
        .on_hover_text(crate::i18n::tfmt(
            "gallery.apply_hint",
            &[&item.name],
        ))
}

fn paint_window(p: &egui::Painter, rect: egui::Rect, t: &Theme) {
    p.rect(
        rect,
        egui::CornerRadius::same(5),
        t.window_fill(),
        egui::Stroke::new(1.0, t.border),
        egui::StrokeKind::Inside,
    );
    let inner = rect.shrink(5.0);
    if inner.width() < 14.0 || inner.height() < 18.0 {
        return;
    }
    // 输入行：底色取 input_fill（与实窗同一算法），里面两截"字"取 input_text / subtitle。
    let input_h = (inner.height() * 0.2).max(7.0);
    let input = egui::Rect::from_min_size(inner.min, egui::vec2(inner.width(), input_h));
    p.rect_filled(input, egui::CornerRadius::same(2), t.input_fill());
    let bar = (input_h * 0.22).max(1.5);
    let ty = input.center().y - bar / 2.0;
    bar_at(p, input.left() + 3.0, ty, input.width() * 0.42, bar, t.input_text);
    bar_at(
        p,
        input.left() + 3.0 + input.width() * 0.46,
        ty,
        input.width() * 0.16,
        bar,
        t.subtitle,
    );
    let sep_y = input.bottom() + 2.5;
    p.line_segment(
        [
            egui::pos2(inner.left(), sep_y),
            egui::pos2(inner.right(), sep_y),
        ],
        egui::Stroke::new(1.0, t.separator),
    );
    let top = sep_y + 3.0;
    let row_h = ((inner.bottom() - top) / 2.0).max(7.0);
    paint_row(
        p,
        egui::Rect::from_min_size(egui::pos2(inner.left(), top), egui::vec2(inner.width(), row_h)),
        t,
        true,
    );
    paint_row(
        p,
        egui::Rect::from_min_size(
            egui::pos2(inner.left(), top + row_h),
            egui::vec2(inner.width(), row_h),
        ),
        t,
        false,
    );
}

fn paint_row(p: &egui::Painter, rect: egui::Rect, t: &Theme, selected: bool) {
    let pal = row_palette(t, selected);
    if selected {
        p.rect_filled(rect, egui::CornerRadius::same(2), pal.background);
    }
    let icon = (rect.height() - 4.0).clamp(5.0, 11.0);
    let icon_rect = egui::Rect::from_min_size(
        egui::pos2(rect.left() + 2.5, rect.center().y - icon / 2.0),
        egui::Vec2::splat(icon),
    );
    paint_icon(p, icon_rect);
    let text_left = icon_rect.right() + 3.0;
    let avail = (rect.right() - 3.0 - text_left).max(4.0);
    let bar = (rect.height() * 0.22).clamp(1.5, 4.0);
    let ty = rect.center().y - bar - 1.0;
    let title_w = avail * 0.44;
    bar_at(p, text_left, ty, title_w, bar, pal.title);
    // 高亮段：紧挨标题的一小截，与实窗「匹配段换个颜色」同一读法。
    bar_at(p, text_left + title_w + 1.5, ty, (avail * 0.16).max(3.0), bar, pal.highlight);
    let sy = ty + bar + 1.5;
    bar_at(p, text_left, sy, avail * 0.5, (bar * 0.6).max(1.0), pal.subtitle);
}

fn bar_at(p: &egui::Painter, x: f32, y: f32, w: f32, h: f32, color: egui::Color32) {
    p.rect_filled(
        egui::Rect::from_min_size(egui::pos2(x, y), egui::vec2(w.max(0.5), h.max(1.0))),
        egui::CornerRadius::ZERO,
        color,
    );
}

fn paint_icon(p: &egui::Painter, rect: egui::Rect) {
    let half = rect.width() / 2.0;
    for (i, color) in ICON_COLORS.iter().enumerate() {
        let origin = egui::pos2(
            rect.left() + (i % 2) as f32 * half,
            rect.top() + (i / 2) as f32 * half,
        );
        p.rect_filled(
            egui::Rect::from_min_size(origin, egui::Vec2::splat(half - 0.4)),
            egui::CornerRadius::ZERO,
            *color,
        );
    }
}

/// 名字：先缩字号直到放得下，最小 8.5 号还放不下才按字符截断加 `…`。
fn paint_name(
    p: &egui::Painter,
    cell: egui::Rect,
    name: &str,
    selected: bool,
    accent: egui::Color32,
    normal: egui::Color32,
) {
    let color = if selected { accent } else { normal };
    let max_w = (cell.width() - 8.0).max(8.0);
    let mut size = 11.5f32;
    let font = loop {
        let f = egui::FontId::proportional(size);
        let w = p.layout_no_wrap(name.to_string(), f.clone(), color).size().x;
        if w <= max_w || size <= 8.5 {
            break f;
        }
        size -= 1.0;
    };
    let mut text = name.to_string();
    if p.layout_no_wrap(text.clone(), font.clone(), color).size().x > max_w {
        while !text.is_empty()
            && p
                .layout_no_wrap(format!("{text}…"), font.clone(), color)
                .size()
                .x
                > max_w
        {
            text.pop();
        }
        text.push('…');
    }
    p.text(
        egui::pos2(cell.center().x, cell.bottom() - NAME_H / 2.0 - 2.0),
        egui::Align2::CENTER_CENTER,
        text,
        font,
        color,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn theme(selected_bg: u32, title: u32, hl: u32, sub: u32) -> Theme {
        let mut t = Theme::light();
        t.selected_background = egui::Color32::from_rgb(
            (selected_bg >> 16) as u8,
            (selected_bg >> 8) as u8,
            selected_bg as u8,
        );
        t.selected_title = egui::Color32::from_rgb(
            (title >> 16) as u8,
            (title >> 8) as u8,
            title as u8,
        );
        t.selected_highlight =
            egui::Color32::from_rgb((hl >> 16) as u8, (hl >> 8) as u8, hl as u8);
        t.selected_subtitle =
            egui::Color32::from_rgb((sub >> 16) as u8, (sub >> 8) as u8, sub as u8);
        t
    }

    #[test]
    fn preview_row_uses_the_selected_keys_of_the_theme() {
        let t = theme(0x112233, 0x445566, 0x778899, 0xAABBCC);
        let p = row_palette(&t, true);
        assert_eq!(p.background, egui::Color32::from_rgb(0x11, 0x22, 0x33));
        assert_eq!(p.title, egui::Color32::from_rgb(0x44, 0x55, 0x66));
        assert_eq!(p.highlight, egui::Color32::from_rgb(0x77, 0x88, 0x99));
        assert_eq!(p.subtitle, egui::Color32::from_rgb(0xAA, 0xBB, 0xCC));
        // 普通行不涂底色，取的是非选中那四个键。
        let q = row_palette(&t, false);
        assert_eq!(q.background, egui::Color32::TRANSPARENT);
        assert_eq!(q.title, t.title);
        assert_eq!(q.highlight, t.highlight);
        assert_eq!(q.subtitle, t.subtitle);
    }

    #[test]
    fn columns_fit_as_many_cells_as_the_width_allows() {
        assert_eq!(columns(370.0, CELL_W, 8.0), 3);
        assert_eq!(columns(500.0, CELL_W, 8.0), 4);
        assert_eq!(columns(100.0, CELL_W, 8.0), 1, "再窄也留一格，不许除出 0");
        assert_eq!(columns(-5.0, CELL_W, 8.0), 1);
        assert_eq!(columns(1000.0, 0.0, 8.0), 1, "零宽格要兜住，不许除零");
    }
}
