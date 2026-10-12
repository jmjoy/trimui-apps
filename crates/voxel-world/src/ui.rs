use crate::{
    app::{App, Screen},
    world::{COLORS, NAMES},
};
use egui::{Align2, Color32, FontId, Rect, Stroke, StrokeKind};
use glam::{Mat4, Vec3};
const WHITE: Color32 = Color32::from_rgb(239, 244, 233);
const GOLD: Color32 = Color32::from_rgb(238, 202, 108);
fn text(
    p: &egui::Painter,
    position: egui::Pos2,
    align: Align2,
    s: &str,
    size: f32,
    color: Color32,
) {
    p.text(position, align, s, FontId::proportional(size), color);
}
fn cube(p: &egui::Painter, center: egui::Pos2, size: f32, block: usize) {
    let [r, g, b] = COLORS[block];
    let color = Color32::from_rgb(r, g, b);
    let top = center - egui::vec2(0.0, size * 0.6);
    let left = center - egui::vec2(size * 0.7, size * 0.2);
    let right = center + egui::vec2(size * 0.7, -size * 0.2);
    let mid = center + egui::vec2(0.0, size * 0.2);
    p.add(egui::Shape::convex_polygon(
        vec![top, right, mid, left],
        color.gamma_multiply(1.15),
        Stroke::NONE,
    ));
    p.add(egui::Shape::convex_polygon(
        vec![
            left,
            mid,
            mid + egui::vec2(0.0, size * 0.8),
            left + egui::vec2(0.0, size * 0.8),
        ],
        color.gamma_multiply(0.75),
        Stroke::NONE,
    ));
    p.add(egui::Shape::convex_polygon(
        vec![
            mid,
            right,
            right + egui::vec2(0.0, size * 0.8),
            mid + egui::vec2(0.0, size * 0.8),
        ],
        color.gamma_multiply(0.9),
        Stroke::NONE,
    ));
}
pub fn draw(ui: &mut egui::Ui, app: &App, fps: f64, matrix: Mat4) {
    let p = ui.painter();
    let rect = ui.max_rect();
    let center = rect.center();
    if app.screen == Screen::Playing {
        let point = center;
        for (a, b) in [
            (point - egui::vec2(9.0, 0.0), point + egui::vec2(9.0, 0.0)),
            (point - egui::vec2(0.0, 9.0), point + egui::vec2(0.0, 9.0)),
        ] {
            p.line_segment([a, b], Stroke::new(4.0, Color32::from_black_alpha(150)));
            p.line_segment([a, b], Stroke::new(2.0, WHITE));
        }
        if let Some(hit) = app.target() {
            let mut points = [egui::Pos2::ZERO; 8];
            let mut valid = true;
            for (i, point) in points.iter_mut().enumerate() {
                let v = hit.block.as_vec3()
                    + Vec3::new((i & 1) as f32, ((i >> 1) & 1) as f32, ((i >> 2) & 1) as f32);
                let q = matrix * v.extend(1.0);
                if q.w <= 0.0 {
                    valid = false;
                    break;
                }
                *point = egui::pos2(
                    (q.x / q.w * 0.5 + 0.5) * rect.width(),
                    (0.5 - q.y / q.w * 0.5) * rect.height(),
                );
            }
            if valid {
                for (a, b) in [
                    (0, 1),
                    (2, 3),
                    (4, 5),
                    (6, 7),
                    (0, 2),
                    (1, 3),
                    (4, 6),
                    (5, 7),
                    (0, 4),
                    (1, 5),
                    (2, 6),
                    (3, 7),
                ] {
                    p.line_segment([points[a], points[b]], Stroke::new(1.5, WHITE));
                }
            }
            text(
                p,
                egui::pos2(center.x, center.y + 24.0),
                Align2::CENTER_TOP,
                NAMES[app.world.get(hit.block) as usize],
                17.0,
                WHITE,
            );
        }
        let bar = Rect::from_center_size(
            egui::pos2(center.x, rect.bottom() - 78.0),
            egui::vec2(8.0 * 66.0 + 16.0, 76.0),
        );
        p.rect_filled(bar, 8, Color32::from_black_alpha(160));
        for block in 1..=8 {
            let slot = Rect::from_min_size(
                bar.min + egui::vec2(8.0 + (block - 1) as f32 * 66.0, 8.0),
                egui::vec2(60.0, 60.0),
            );
            if block == app.settings.selected as usize {
                p.rect_filled(slot, 4, Color32::from_white_alpha(25));
                p.rect_stroke(slot, 4, Stroke::new(2.0, GOLD), StrokeKind::Inside);
            }
            cube(p, slot.center() - egui::vec2(0.0, 5.0), 23.0, block);
        }
        text(
            p,
            egui::pos2(center.x, bar.top() - 10.0),
            Align2::CENTER_BOTTOM,
            app.block_name(),
            23.0,
            WHITE,
        );
        text(
            p,
            egui::pos2(24.0, 22.0),
            Align2::LEFT_TOP,
            &format!(
                "方块世界   ·   {}",
                if app.player.flying {
                    "飞行"
                } else {
                    "步行"
                }
            ),
            23.0,
            WHITE,
        );
        text(
            p,
            egui::pos2(rect.right() - 24.0, 24.0),
            Align2::RIGHT_TOP,
            &format!(
                "{fps:.0} FPS   |   {:.0} / {:.0} / {:.0}",
                app.player.position.x, app.player.position.y, app.player.position.z
            ),
            16.0,
            WHITE,
        );
        text(
            p,
            egui::pos2(center.x, rect.bottom() - 18.0),
            Align2::CENTER_BOTTOM,
            "L2 放置   R2 挖掘   L/R 切换方块   X 飞行   Y 方块库   START 菜单",
            18.0,
            WHITE,
        );
    } else {
        p.rect_filled(rect, 0, Color32::from_black_alpha(90));
        let (title, subtitle, items): (&str, String, Vec<String>) = match app.screen {
            Screen::Title => (
                "方块世界",
                "一块一块，造出你的世界".into(),
                vec![
                    if app.has_save {
                        "继续世界"
                    } else {
                        "开始游戏"
                    }
                    .into(),
                    "新建世界".into(),
                    "退出".into(),
                ],
            ),
            Screen::Pause => (
                "暂停",
                format!("种子 {} · 256 × 256 × 64", app.world.seed),
                vec![
                    "继续探索".into(),
                    "立即保存".into(),
                    format!(
                        "视距                     {} 区块",
                        app.settings.view_distance
                    ),
                    format!("视角灵敏度             {:.1}", app.settings.sensitivity),
                    format!(
                        "音效音量                 {:.0}%",
                        app.settings.volume * 100.0
                    ),
                    "重建世界…".into(),
                    "保存并退出".into(),
                ],
            ),
            Screen::ConfirmNew => (
                "新建世界",
                "这会替换当前建筑与存档。确认重新开始？".into(),
                vec!["保留当前世界".into(), "确认重建".into()],
            ),
            Screen::Palette => (
                "方块库",
                "创造模式 · 所有方块无限供应".into(),
                (1..=8).map(|b| NAMES[b].into()).collect(),
            ),
            Screen::Playing => unreachable!(),
        };
        let panel = Rect::from_center_size(
            center,
            egui::vec2(
                570.0,
                if app.screen == Screen::Title {
                    448.0
                } else if app.screen == Screen::ConfirmNew {
                    320.0
                } else {
                    632.0
                },
            ),
        );
        p.rect_filled(panel, 16, Color32::from_rgba_unmultiplied(22, 36, 34, 235));
        p.rect_stroke(
            panel,
            16,
            Stroke::new(1.0, Color32::from_rgb(74, 103, 77)),
            StrokeKind::Inside,
        );
        text(
            p,
            egui::pos2(center.x, panel.top() + 40.0),
            Align2::CENTER_TOP,
            title,
            if app.screen == Screen::Title {
                56.0
            } else {
                36.0
            },
            WHITE,
        );
        text(
            p,
            egui::pos2(
                center.x,
                panel.top()
                    + if app.screen == Screen::Title {
                        112.0
                    } else {
                        90.0
                    },
            ),
            Align2::CENTER_TOP,
            &subtitle,
            19.0,
            Color32::from_rgb(162, 188, 166),
        );
        let top = panel.top()
            + if app.screen == Screen::Title {
                170.0
            } else {
                140.0
            };
        let spacing = if app.screen == Screen::Palette {
            51.0
        } else {
            55.0
        };
        for (i, item) in items.iter().enumerate() {
            let row = Rect::from_min_size(
                egui::pos2(panel.left() + 28.0, top + i as f32 * spacing),
                egui::vec2(panel.width() - 56.0, spacing - 7.0),
            );
            if app.cursor == i {
                p.rect_filled(row, 6, Color32::from_rgb(59, 88, 59));
                p.rect_stroke(row, 6, Stroke::new(1.5, GOLD), StrokeKind::Inside);
            }
            if app.screen == Screen::Palette {
                cube(
                    p,
                    egui::pos2(row.left() + 30.0, row.center().y - 5.0),
                    19.0,
                    i + 1,
                );
                text(
                    p,
                    egui::pos2(row.left() + 63.0, row.center().y),
                    Align2::LEFT_CENTER,
                    item,
                    24.0,
                    WHITE,
                );
            } else {
                text(
                    p,
                    row.center(),
                    Align2::CENTER_CENTER,
                    item,
                    24.0,
                    if app.cursor == i { GOLD } else { WHITE },
                );
            }
        }
        text(
            p,
            egui::pos2(center.x, panel.bottom() - 24.0),
            Align2::CENTER_BOTTOM,
            "方向键选择   A 确认   B 返回   SELECT + START 退出",
            17.0,
            Color32::from_rgb(162, 188, 166),
        );
    }
    if !app.status.is_empty() {
        let toast = Rect::from_center_size(
            egui::pos2(center.x, rect.bottom() - 178.0),
            egui::vec2(950.0, 44.0),
        );
        p.rect_filled(toast, 6, Color32::from_black_alpha(210));
        text(
            p,
            toast.center(),
            Align2::CENTER_CENTER,
            &app.status,
            19.0,
            GOLD,
        );
    }
}
