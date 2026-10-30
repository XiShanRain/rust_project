//! 批次 F12（FR-UIX-14）：逻辑点 ↔ 物理像素的唯一换算层。
//!
//! egui 侧（布局、字号、行高、阴影带宽）全是逻辑点；Win32 侧（`GetWindowRect`、
//! `SetWindowPos`、`MonitorFromPoint`、`GetDpiForWindow`）全是物理像素。两边换算原先
//! 散在各处、取整策略也各写各的（有的 `round`、有的 `as i32` 截断）。收进来之后：
//! 点→像素一律四舍五入，像素→点一律直接除，改规则只需要改这里。
//!
//! `pixels_per_point` 是「原生 DPI 缩放 × egui 自己的 Ctrl+滚轮缩放」的乘积，
//! 所以这里不假设它只会是 1.0 / 1.25 / 1.5 / 2.0 这类整齐值。

use eframe::egui;

/// 逻辑点 → 物理像素（四舍五入）。允许负值（拖窗位移换算要用）。
pub fn px(logical: f32, ppp: f32) -> i32 {
    (logical * ppp).round() as i32
}

/// 物理像素矩形 → 逻辑点矩形。HUD / 大字窗拿到的是 Win32 的工作区（物理像素），
/// 而排版权限在 egui 里，必须先折回逻辑点。
pub fn rect_to_pt(r: egui::Rect, ppp: f32) -> egui::Rect {
    egui::Rect::from_min_max(
        egui::pos2(r.min.x / ppp, r.min.y / ppp),
        egui::pos2(r.max.x / ppp, r.max.y / ppp),
    )
}

/// 图标的目标提取边长（物理像素）。取上限是为了别为大图标把整张位图搬进内存，
/// 取整用 ceil：宁可多给一像素让 shell 缩，也不要少给导致贴图被放大发虚。
pub fn icon_px(icon_pt: f32, ppp: f32) -> u32 {
    (icon_pt * ppp).ceil().clamp(16.0, 96.0) as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn px_rounds_half_away_from_zero() {
        assert_eq!(px(12.0, 1.0), 12);
        assert_eq!(px(12.0, 1.5), 18);
        assert_eq!(px(12.0, 1.25), 15);
        assert_eq!(px(12.0, 2.0), 24);
        assert_eq!(px(0.5, 1.0), 1);
        assert_eq!(px(-0.5, 1.0), -1);
        assert_eq!(px(-12.0, 1.5), -18);
    }

    #[test]
    fn px_does_not_truncate() {
        assert_eq!(px(12.2, 1.0), 12);
        assert_eq!(px(12.6, 1.0), 13);
        // 同一逻辑宽度在 150% 下应是 18.45 → 18，而不是截成 18 之外的 18.3 → 18
        assert_eq!(px(12.3, 1.5), 18);
    }

    #[test]
    fn rect_to_pt_round_trips_on_common_scales() {
        for ppp in [1.0, 1.25, 1.5, 1.75, 2.0] {
            let logical = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(384.0, 300.0));
            let phys = egui::Rect::from_min_max(
                egui::pos2(px(logical.min.x, ppp) as f32, px(logical.min.y, ppp) as f32),
                egui::pos2(px(logical.max.x, ppp) as f32, px(logical.max.y, ppp) as f32),
            );
            assert_eq!(rect_to_pt(phys, ppp), logical, "ppp={ppp}");
        }
    }

    #[test]
    fn rect_to_pt_divides_all_four_edges() {
        let phys = egui::Rect::from_min_max(egui::pos2(192.0, 96.0), egui::pos2(2880.0, 1728.0));
        let got = rect_to_pt(phys, 2.0);
        assert_eq!(got.min, egui::pos2(96.0, 48.0));
        assert_eq!(got.max, egui::pos2(1440.0, 864.0));
        // 原点不动的矩形仍是同一块逻辑区域
        let zero = rect_to_pt(egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(500.0, 400.0)), 1.5);
        assert_eq!(zero.min, egui::Pos2::ZERO);
        assert_eq!(zero.max, egui::pos2(500.0 / 1.5, 400.0 / 1.5));
    }

    #[test]
    fn icon_px_scales_and_clamps() {
        assert_eq!(icon_px(32.0, 1.0), 32);
        assert_eq!(icon_px(32.0, 1.25), 40);
        assert_eq!(icon_px(32.0, 1.5), 48);
        assert_eq!(icon_px(32.0, 2.0), 64);
        assert_eq!(icon_px(16.0, 0.5), 16, "低于下限按 16 提");
        assert_eq!(icon_px(32.0, 5.0), 96, "高于上限按 96 提");
        assert_eq!(icon_px(32.0, 1.1), 36, "非整齐缩放向上取整");
    }
}
