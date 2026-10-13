use crate::{
    app::{App, Screen},
    system::{CpuState, Telemetry},
    world::block,
};
use egui::{Align2, Color32, FontId, Rect, Stroke, StrokeKind};
use glam::{Mat4, Vec3};
const WHITE: Color32 = Color32::from_rgb(239, 244, 233);
const GOLD: Color32 = Color32::from_rgb(238, 202, 108);
const MUTED: Color32 = Color32::from_rgb(162, 188, 166);

/// Values refreshed once per second for the HUD and debug overlay.
#[derive(Clone, Debug)]
pub struct Diagnostics {
    pub fps: f64,
    pub frame_avg_ms: f32,
    pub frame_worst_ms: f32,
    pub telemetry: Telemetry,
    pub cpu_state: CpuState,
    pub workers_active: usize,
    pub workers_total: usize,
    pub workers_running: usize,
    pub queued: [usize; 4],
    pub loaded: usize,
    pub pending: usize,
    pub columns: usize,
    pub sections_mb: f32,
    pub streaming: usize,
    pub gen_ms: f32,
    pub light_ms: f32,
    pub lighting: usize,
    pub audio: crate::audio::AudioStats,
}
impl Default for Diagnostics {
    fn default() -> Self {
        Self {
            fps: 0.0,
            frame_avg_ms: 0.0,
            frame_worst_ms: 0.0,
            telemetry: Telemetry::default(),
            cpu_state: CpuState::Default,
            workers_active: 1,
            workers_total: 1,
            workers_running: 0,
            queued: [0; 4],
            loaded: 0,
            pending: 0,
            columns: 0,
            sections_mb: 0.0,
            streaming: 0,
            gen_ms: 0.0,
            light_ms: 0.0,
            lighting: 0,
            audio: Default::default(),
        }
    }
}
impl Diagnostics {
    /// Overlay lines; kept free of egui so the format is unit-testable.
    pub fn lines(&self, app: &App) -> Vec<String> {
        let t = &self.telemetry;
        let p = app.player.position;
        vec![
            format!(
                "FPS {:.1}   帧 平均 {:.1} ms / 最差 {:.1} ms",
                self.fps, self.frame_avg_ms, self.frame_worst_ms
            ),
            format!(
                "CPU 在线 {}（{}）  线程 {}/{} 忙 {}",
                t.online_label(),
                self.cpu_state.label(),
                self.workers_active,
                self.workers_total,
                self.workers_running
            ),
            format!("频率 {} MHz   调频 {}", t.freq_label(), t.governor),
            format!(
                "温度 CPU/GPU {} °C   内存 {:.1} MB",
                t.temp_label(),
                t.rss_mb
            ),
            format!(
                "区块柱 {}（{:.1} MB）加载中 {}  生成 {:.1} ms  网格段 {} 构建中 {}",
                self.columns,
                self.sections_mb,
                self.streaming,
                self.gen_ms,
                self.loaded,
                self.pending
            ),
            format!(
                "队列 生成 {}  光照 {}  网格 {}",
                self.queued[0], self.queued[1], self.queued[2]
            ),
            format!(
                "音频 回调 平均 {:.0} µs / 最大 {:.0} µs  负载 {:.1}%  声道峰值 {}",
                self.audio.avg_us,
                self.audio.max_us,
                self.audio.load * 100.0,
                self.audio.voices_peak
            ),
            {
                let (day, hour, minute) = crate::render::sky::clock(app.time);
                let eye = app.player.eye().floor().as_ivec3();
                let (sky, block) = app.world.light(eye).unwrap_or((0, 0));
                format!(
                    "第 {day} 天 {hour:02}:{minute:02}   光照 天空 {sky} 方块 {block}   光照计算 {:.1} ms 待算 {}",
                    self.light_ms, self.lighting
                )
            },
            format!(
                "坐标 {:.1} / {:.1} / {:.1}   朝向 {:.0}°",
                p.x,
                p.y,
                p.z,
                app.player.yaw.to_degrees()
            ),
        ]
    }
}

fn debug_overlay(p: &egui::Painter, app: &App, diag: &Diagnostics) {
    let lines = diag.lines(app);
    let height = 14.0 + lines.len() as f32 * 22.0;
    let rect = Rect::from_min_size(egui::pos2(12.0, 64.0), egui::vec2(560.0, height));
    p.rect_filled(rect, 6, Color32::from_black_alpha(170));
    for (i, line) in lines.iter().enumerate() {
        p.text(
            rect.min + egui::vec2(10.0, 8.0 + i as f32 * 22.0),
            Align2::LEFT_TOP,
            line,
            FontId::monospace(16.0),
            WHITE,
        );
    }
}
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
/// Block icons uploaded to egui by the renderer.
pub struct Icons {
    pub texture: egui::TextureId,
    pub uv: Vec<Rect>,
}
impl Icons {
    fn draw(&self, p: &egui::Painter, rect: Rect, id: u8) {
        if let Some(uv) = self.uv.get(usize::from(id))
            && id != block::AIR
        {
            p.image(self.texture, rect, *uv, Color32::WHITE);
        }
    }
}
/// Columns of the creative palette grid.
pub const PALETTE_COLUMNS: usize = 9;

/// `matrix` is the scene's view-projection relative to block `origin`.
pub fn draw(
    ui: &mut egui::Ui,
    app: &App,
    diag: &Diagnostics,
    icons: &Icons,
    matrix: Mat4,
    origin: glam::IVec3,
) {
    let fps = diag.fps;
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
                let v = (hit.block - origin).as_vec3()
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
                block::def(app.world.get(hit.block)).name,
                17.0,
                WHITE,
            );
        }
        let bar = Rect::from_center_size(
            egui::pos2(center.x, rect.bottom() - 78.0),
            egui::vec2(app.hotbar.len() as f32 * 62.0 + 12.0, 72.0),
        );
        p.rect_filled(bar, 8, Color32::from_black_alpha(160));
        for (i, &id) in app.hotbar.iter().enumerate() {
            let slot = Rect::from_min_size(
                bar.min + egui::vec2(8.0 + i as f32 * 62.0, 8.0),
                egui::vec2(56.0, 56.0),
            );
            p.rect_filled(slot, 4, Color32::from_black_alpha(70));
            if i == app.slot {
                p.rect_filled(slot, 4, Color32::from_white_alpha(28));
                p.rect_stroke(slot, 4, Stroke::new(2.5, GOLD), StrokeKind::Inside);
            }
            icons.draw(
                p,
                Rect::from_center_size(slot.center(), egui::vec2(48.0, 48.0)),
                id,
            );
        }
        text(
            p,
            egui::pos2(center.x, bar.top() - 10.0),
            Align2::CENTER_BOTTOM,
            app.block_name(),
            23.0,
            WHITE,
        );
        let mut mode = vec![if app.player.flying {
            "飞行"
        } else {
            "步行"
        }];
        if app.player.sneaking || app.sneak_latched && !app.player.flying {
            mode.push("潜行");
        }
        if app.player.sprinting {
            mode.push("疾跑");
        } else if app.sprint_latched {
            mode.push("疾跑开");
        }
        text(
            p,
            egui::pos2(24.0, 22.0),
            Align2::LEFT_TOP,
            &format!("方块世界   ·   {}", mode.join(" · ")),
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
            &app.hints(),
            17.0,
            WHITE,
        );
    } else if app.screen == Screen::Palette {
        draw_palette(p, app, icons, rect);
    } else {
        draw_menu(p, app, rect);
    }
    if app.show_overlay() {
        debug_overlay(p, app, diag);
    }
    if !app.status.is_empty() {
        // Menus fill the middle of the screen; show messages above them.
        let y = if app.screen == Screen::Playing {
            rect.bottom() - 178.0
        } else {
            rect.top() + 30.0
        };
        let toast = Rect::from_center_size(egui::pos2(center.x, y), egui::vec2(950.0, 44.0));
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

/// Rows visible at once; longer menus scroll with the cursor.
const VISIBLE_ROWS: usize = 9;

/// First visible row so that `cursor` stays inside the window.
pub fn scroll_offset(cursor: usize, len: usize, visible: usize) -> usize {
    if len <= visible {
        0
    } else {
        cursor.saturating_sub(visible / 2).min(len - visible)
    }
}

fn draw_menu(p: &egui::Painter, app: &App, rect: Rect) {
    let center = rect.center();
    let menu = app.menu();
    let title_screen = app.screen == Screen::Title;
    p.rect_filled(rect, 0, Color32::from_black_alpha(90));
    let spacing = if menu.items.len() > 5 { 50.0 } else { 55.0 };
    let visible = menu.items.len().min(VISIBLE_ROWS);
    let offset = scroll_offset(app.cursor, menu.items.len(), VISIBLE_ROWS);
    let wide = menu.items.iter().any(|i| i.value.is_some());
    let header = if title_screen { 170.0 } else { 140.0 };
    let panel = Rect::from_center_size(
        center,
        egui::vec2(
            if wide { 760.0 } else { 570.0 },
            (header + visible as f32 * spacing + 62.0).clamp(320.0, 700.0),
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
        egui::pos2(center.x, panel.top() + 36.0),
        Align2::CENTER_TOP,
        menu.title,
        if title_screen { 56.0 } else { 36.0 },
        WHITE,
    );
    text(
        p,
        egui::pos2(
            center.x,
            panel.top() + if title_screen { 112.0 } else { 88.0 },
        ),
        Align2::CENTER_TOP,
        &menu.subtitle,
        18.0,
        MUTED,
    );
    let top = panel.top() + header;
    for (slot, (i, item)) in menu
        .items
        .iter()
        .enumerate()
        .skip(offset)
        .take(visible)
        .enumerate()
    {
        let row = Rect::from_min_size(
            egui::pos2(panel.left() + 28.0, top + slot as f32 * spacing),
            egui::vec2(panel.width() - 56.0, spacing - 7.0),
        );
        let selected = app.cursor == i;
        if selected {
            p.rect_filled(row, 6, Color32::from_rgb(59, 88, 59));
            p.rect_stroke(row, 6, Stroke::new(1.5, GOLD), StrokeKind::Inside);
        }
        let color = if selected { GOLD } else { WHITE };
        if let Some(value) = &item.value {
            text(
                p,
                egui::pos2(row.left() + 18.0, row.center().y),
                Align2::LEFT_CENTER,
                &item.label,
                22.0,
                color,
            );
            text(
                p,
                egui::pos2(row.right() - 18.0, row.center().y),
                Align2::RIGHT_CENTER,
                value,
                22.0,
                color,
            );
        } else {
            text(
                p,
                row.center(),
                Align2::CENTER_CENTER,
                &item.label,
                24.0,
                color,
            );
        }
    }
    if offset > 0 {
        text(
            p,
            egui::pos2(panel.right() - 22.0, top - 6.0),
            Align2::RIGHT_BOTTOM,
            "▲",
            16.0,
            MUTED,
        );
    }
    if offset + visible < menu.items.len() {
        text(
            p,
            egui::pos2(panel.right() - 22.0, top + visible as f32 * spacing - 4.0),
            Align2::RIGHT_TOP,
            "▼",
            16.0,
            MUTED,
        );
    }
    text(
        p,
        egui::pos2(center.x, panel.bottom() - 22.0),
        Align2::CENTER_BOTTOM,
        "方向键选择   A 确认   B 返回   SELECT + START 退出",
        17.0,
        MUTED,
    );
}

/// Creative palette: a grid of block icons with the hovered name.
fn draw_palette(p: &egui::Painter, app: &App, icons: &Icons, rect: Rect) {
    let palette = block::palette();
    let menu = app.menu();
    let rows = palette.len().div_ceil(PALETTE_COLUMNS);
    let cell = 64.0;
    p.rect_filled(rect, 0, Color32::from_black_alpha(110));
    let grid = egui::vec2(PALETTE_COLUMNS as f32 * cell, rows as f32 * cell);
    let panel = Rect::from_center_size(rect.center(), grid + egui::vec2(48.0, 190.0));
    p.rect_filled(panel, 16, Color32::from_rgba_unmultiplied(22, 36, 34, 240));
    p.rect_stroke(
        panel,
        16,
        Stroke::new(1.0, Color32::from_rgb(74, 103, 77)),
        StrokeKind::Inside,
    );
    text(
        p,
        egui::pos2(panel.center().x, panel.top() + 22.0),
        Align2::CENTER_TOP,
        menu.title,
        32.0,
        WHITE,
    );
    text(
        p,
        egui::pos2(panel.center().x, panel.top() + 66.0),
        Align2::CENTER_TOP,
        &menu.subtitle,
        20.0,
        GOLD,
    );
    let origin = egui::pos2(panel.left() + 24.0, panel.top() + 104.0);
    for (i, &id) in palette.iter().enumerate() {
        let cell_rect = Rect::from_min_size(
            origin
                + egui::vec2(
                    (i % PALETTE_COLUMNS) as f32 * cell,
                    (i / PALETTE_COLUMNS) as f32 * cell,
                ),
            egui::vec2(cell - 4.0, cell - 4.0),
        );
        p.rect_filled(cell_rect, 6, Color32::from_black_alpha(80));
        if i == app.cursor {
            p.rect_filled(cell_rect, 6, Color32::from_rgb(59, 88, 59));
            p.rect_stroke(cell_rect, 6, Stroke::new(2.0, GOLD), StrokeKind::Inside);
        }
        icons.draw(
            p,
            Rect::from_center_size(cell_rect.center(), egui::vec2(48.0, 48.0)),
            id,
        );
    }
    text(
        p,
        egui::pos2(panel.center().x, panel.bottom() - 20.0),
        Align2::CENTER_BOTTOM,
        "方向键选择   A 放入快捷栏   B 返回",
        17.0,
        MUTED,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn menu_scroll_keeps_cursor_visible() {
        assert_eq!(scroll_offset(3, 5, 9), 0);
        assert_eq!(scroll_offset(0, 12, 9), 0);
        assert_eq!(scroll_offset(6, 12, 9), 2);
        assert_eq!(scroll_offset(11, 12, 9), 3);
        for cursor in 0..20 {
            let o = scroll_offset(cursor, 20, 9);
            assert!(o <= cursor && cursor < o + 9);
        }
    }
}
