use crate::{
    ai::JobKind,
    app::App,
    board::BoardLayout,
    game::{Outcome, SIZE},
};
use egui::{Align2, Color32, FontId, Rect, Stroke, StrokeKind};

const INK: Color32 = Color32::from_rgb(42, 56, 47);
const MUTED: Color32 = Color32::from_rgb(121, 124, 108);
const GREEN: Color32 = Color32::from_rgb(48, 91, 72);

pub fn configure(ctx: &egui::Context) {
    let mut style = egui::Style {
        visuals: egui::Visuals::light(),
        ..Default::default()
    };
    style.visuals.override_text_color = Some(INK);
    style
        .text_styles
        .insert(egui::TextStyle::Body, FontId::proportional(22.0));
    ctx.set_theme(egui::Theme::Light);
    ctx.set_style_of(egui::Theme::Light, style);
}

pub fn draw(ui: &mut egui::Ui, app: &App, fps: f64) -> BoardLayout {
    let screen = ui.max_rect();
    let side = (screen.height() - 32.0).min(screen.width() * 0.56);
    let layout = BoardLayout {
        rect: Rect::from_min_size(screen.min + egui::vec2(16.0, 16.0), egui::Vec2::splat(side)),
    };
    let painter = ui.painter();
    let origin = layout.origin();
    let spacing = layout.spacing();
    for i in 0..SIZE {
        painter.text(
            egui::pos2(
                origin.x + i as f32 * spacing,
                layout.rect.bottom() - side * 0.037,
            ),
            Align2::CENTER_CENTER,
            char::from(b'A' + i as u8),
            FontId::proportional(16.0),
            Color32::from_rgb(109, 78, 43),
        );
        painter.text(
            egui::pos2(
                layout.rect.left() + side * 0.037,
                origin.y + i as f32 * spacing,
            ),
            Align2::CENTER_CENTER,
            SIZE - i,
            FontId::proportional(16.0),
            Color32::from_rgb(109, 78, 43),
        );
    }
    if !app.setup {
        let center = origin + egui::vec2(app.cursor.x as f32, app.cursor.y as f32) * spacing;
        painter.rect_stroke(
            Rect::from_center_size(center, egui::Vec2::splat(spacing * 0.96)),
            5,
            Stroke::new(3.0, Color32::from_rgb(43, 125, 200)),
            StrokeKind::Outside,
        );
    }
    let panel = Rect::from_min_max(
        egui::pos2(layout.rect.right() + 24.0, screen.top() + 16.0),
        screen.max - egui::vec2(16.0, 16.0),
    );
    painter.rect_filled(panel, 12, Color32::from_rgb(250, 249, 242));
    let left = panel.left() + 24.0;
    let text = |y: f32, value: &str, size: f32, color: Color32| {
        painter.text(
            egui::pos2(left, y),
            Align2::LEFT_TOP,
            value,
            FontId::proportional(size),
            color,
        );
    };
    text(32.0, "五子棋", 36.0, GREEN);
    text(82.0, "木纹之间 · 黑白对弈", 19.0, MUTED);
    if app.setup || app.menu {
        text(
            126.0,
            if app.setup {
                "新对局"
            } else {
                "对局菜单"
            },
            25.0,
            GREEN,
        );
        let row_height = if app.setup { 42.0 } else { 35.0 };
        for (i, label) in app.menu_labels().iter().enumerate() {
            let y = 170.0 + i as f32 * row_height;
            let selected = i == app.menu_row;
            let enabled = app.menu_row_enabled(i);
            if selected {
                painter.rect_filled(
                    Rect::from_min_size(
                        egui::pos2(left - 8.0, y - 4.0),
                        egui::vec2(panel.width() - 32.0, row_height - 2.0),
                    ),
                    5,
                    Color32::from_rgb(213, 228, 211),
                );
            }
            text(
                y,
                label,
                22.0,
                if !enabled {
                    MUTED
                } else if selected {
                    GREEN
                } else {
                    INK
                },
            );
        }
        if let Some(notice) = &app.notice {
            text(603.0, notice, 18.0, Color32::from_rgb(158, 80, 53));
        }
        text(636.0, "↑↓ 选择   ←→ 调整   A 确认", 19.0, MUTED);
        text(
            666.0,
            if app.setup {
                "START 开始   SELECT + START 退出"
            } else {
                "B / START 返回   SELECT + START 退出"
            },
            18.0,
            MUTED,
        );
    } else {
        let status = match &app.session.game.outcome {
            Outcome::Playing if app.session.human_turn() => "轮到你落子",
            Outcome::Playing => "电脑正在思考…",
            Outcome::Won { stone, .. } if *stone == app.session.human => "你赢了！",
            Outcome::Won { .. } => "电脑获胜",
            Outcome::Draw => "和棋",
        };
        text(139.0, status, 30.0, GREEN);
        text(
            185.0,
            &format!(
                "你执{}  ·  {}难度",
                app.session.human.name(),
                app.session.difficulty.name()
            ),
            22.0,
            INK,
        );
        text(
            223.0,
            &format!(
                "第 {} 手  ·  光标 {}",
                app.session.game.history.len(),
                app.cursor
            ),
            22.0,
            MUTED,
        );
        if app.session.pending.is_some_and(|p| p.kind == JobKind::Hint) {
            text(265.0, "正在寻找推荐落点…", 22.0, GREEN);
        } else if let Some(hint) = app.session.hint {
            text(265.0, &format!("推荐 {hint}，绿圈已标记"), 22.0, GREEN);
        } else {
            text(265.0, "蓝框为光标，小点标记上一手", 20.0, MUTED);
        }
        text(322.0, "最近落子", 20.0, GREEN);
        let history = &app.session.game.history;
        for (row, (index, mv)) in history
            .iter()
            .enumerate()
            .skip(history.len().saturating_sub(5))
            .enumerate()
        {
            text(
                356.0 + row as f32 * 30.0,
                &format!("{:>3}   {}   {}", index + 1, mv.stone.name(), mv.pos),
                21.0,
                INK,
            );
        }
        if let Some(notice) = &app.notice {
            text(520.0, notice, 18.0, Color32::from_rgb(158, 80, 53));
        } else if !app.audio.available() {
            text(520.0, "音频暂不可用，对局可正常进行", 18.0, MUTED);
        }
        text(564.0, "方向键 / 左摇杆移动   A 落子", 20.0, INK);
        text(598.0, "X 悔棋   Y 提示   START 菜单", 20.0, INK);
        text(636.0, "黑棋先手，五子或长连即胜，无禁手", 18.0, MUTED);
        text(
            668.0,
            &format!("SELECT + START 退出                  {fps:.0} FPS"),
            17.0,
            MUTED,
        );
    }
    layout
}
