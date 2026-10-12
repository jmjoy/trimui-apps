use egui::{Align2, Color32, FontId, Painter, Pos2, Rect, Stroke, StrokeKind, Vec2};

use crate::{
    app::App,
    game::{Color, Move, Square},
};

const INK: Color32 = Color32::from_rgb(237, 229, 209);
const GOLD: Color32 = Color32::from_rgb(233, 192, 105);
const MUTED: Color32 = Color32::from_rgb(164, 168, 160);

fn text(painter: &Painter, pos: Pos2, size: f32, color: Color32, value: impl ToString) {
    painter.text(
        pos,
        Align2::LEFT_TOP,
        value.to_string(),
        FontId::proportional(size),
        color,
    );
}

pub fn draw(ui: &mut egui::Ui, app: &App, legal_moves: &[Move], fps: f64) {
    let rect = ui.max_rect();
    let painter = ui.painter();
    let scale = (rect.height() / 720.0).min(rect.width() / 1280.0);
    let pos = |x: f32, y: f32| rect.min + Vec2::new(x * scale, y * scale);
    let sidebar = Rect::from_min_max(pos(762.0, 20.0), pos(1260.0, 700.0));
    painter.rect_filled(sidebar, 18.0 * scale, Color32::from_rgb(31, 40, 41));
    text(painter, pos(78.0, 18.0), 30.0 * scale, GOLD, "中国象棋");
    text(
        painter,
        pos(277.0, 26.0),
        17.0 * scale,
        MUTED,
        "楚河汉界 · 人机对弈",
    );
    draw_board(painter, app, legal_moves, pos(386.0, 385.0), 62.0 * scale);
    text(
        painter,
        pos(789.0, 44.0),
        29.0 * scale,
        GOLD,
        if app.setup {
            "新对局"
        } else if app.menu {
            "对局菜单"
        } else {
            "对局状态"
        },
    );
    if app.setup {
        text(
            painter,
            pos(791.0, 91.0),
            19.0 * scale,
            MUTED,
            "上下选择，左右调整，A 确认",
        );
        for (index, label) in [
            format!(
                "执子       {}",
                if app.player_color == Color::Red {
                    "红方 · 先手"
                } else {
                    "黑方 · 后手"
                }
            ),
            format!("难度       {}", app.difficulty.label()),
            format!("音效       {}", sound_label(app)),
            "开始对局".into(),
        ]
        .iter()
        .enumerate()
        {
            row(
                painter,
                pos(782.0, 148.0 + index as f32 * 66.0),
                Vec2::new(458.0, 54.0) * scale,
                label,
                app.menu_row == index,
                scale,
            );
        }
        text(
            painter,
            pos(791.0, 462.0),
            20.0 * scale,
            INK,
            "红方在下，黑方在上",
        );
        text(
            painter,
            pos(791.0, 498.0),
            19.0 * scale,
            MUTED,
            "方向键或左摇杆移动光标",
        );
        text(
            painter,
            pos(791.0, 532.0),
            19.0 * scale,
            MUTED,
            "A 选子 / 落子，B 取消选择",
        );
        text(
            painter,
            pos(791.0, 566.0),
            19.0 * scale,
            MUTED,
            "X 悔棋，Y 跳到合法落点",
        );
    } else if app.menu {
        for (index, label) in [
            "继续对局".into(),
            "悔棋".into(),
            format!("难度       {}", app.difficulty.label()),
            format!("音效       {}", sound_label(app)),
            "重新开始".into(),
            "退出游戏".into(),
        ]
        .iter()
        .enumerate()
        {
            row(
                painter,
                pos(782.0, 128.0 + index as f32 * 65.0),
                Vec2::new(458.0, 54.0) * scale,
                label,
                app.menu_row == index,
                scale,
            );
        }
        text(
            painter,
            pos(791.0, 554.0),
            19.0 * scale,
            MUTED,
            "上下选择，A 确认，B 返回",
        );
    } else {
        let status = if app.ai_thinking() {
            "电脑思考中…".into()
        } else if app.game.status().is_finished() {
            app.game.status().description()
        } else {
            format!("轮到{}", app.game.side_to_move().label())
        };
        text(painter, pos(791.0, 99.0), 24.0 * scale, INK, status);
        text(
            painter,
            pos(791.0, 142.0),
            18.0 * scale,
            MUTED,
            format!(
                "你执{}  ·  {}  ·  音效{}",
                app.player_color.label(),
                app.difficulty.label(),
                sound_label(app)
            ),
        );
        if app.game.is_in_check(app.game.side_to_move()) && !app.game.status().is_finished() {
            text(
                painter,
                pos(791.0, 179.0),
                22.0 * scale,
                Color32::from_rgb(249, 133, 108),
                "将军！",
            );
        } else {
            text(
                painter,
                pos(791.0, 179.0),
                19.0 * scale,
                GOLD,
                if app.selected.is_some() {
                    "请选择落点，按 A 落子"
                } else {
                    "移动光标，按 A 选择己方棋子"
                },
            );
        }
        text(painter, pos(791.0, 234.0), 20.0 * scale, GOLD, "最近走子");
        let records = app.game.history();
        if records.is_empty() {
            text(painter, pos(791.0, 275.0), 20.0 * scale, MUTED, "暂无走子");
        }
        for (row_index, (index, record)) in records
            .iter()
            .enumerate()
            .skip(records.len().saturating_sub(6))
            .enumerate()
        {
            let tag = if record.gave_check {
                "  将军"
            } else if record.captured.is_some() {
                "  吃子"
            } else {
                ""
            };
            text(
                painter,
                pos(791.0, 276.0 + row_index as f32 * 33.0),
                19.0 * scale,
                if record.mover == Color::Red {
                    Color32::from_rgb(244, 162, 143)
                } else {
                    INK
                },
                format!(
                    "{:>2}. {}  {}{tag}",
                    index + 1,
                    record.mover.label(),
                    record.notation
                ),
            );
        }
        if let Some(notice) = &app.notice {
            text(painter, pos(791.0, 493.0), 20.0 * scale, GOLD, notice);
        }
        text(
            painter,
            pos(791.0, 552.0),
            19.0 * scale,
            MUTED,
            "方向键 / 左摇杆：移动光标",
        );
        text(
            painter,
            pos(791.0, 584.0),
            19.0 * scale,
            MUTED,
            "A：选子 / 落子    B：取消",
        );
        text(
            painter,
            pos(791.0, 616.0),
            19.0 * scale,
            MUTED,
            "X：悔棋    Y：下一个合法落点",
        );
    }
    text(
        painter,
        pos(791.0, 659.0),
        17.0 * scale,
        MUTED,
        "START 菜单   SELECT + START 退出",
    );
    text(
        painter,
        pos(563.0, 29.0),
        14.0 * scale,
        MUTED,
        format!("{fps:.0} FPS"),
    );
}

fn sound_label(app: &App) -> &'static str {
    if !app.audio.is_available() {
        "不可用"
    } else if app.audio.enabled() {
        "开"
    } else {
        "关"
    }
}

fn row(painter: &Painter, origin: Pos2, size: Vec2, label: &str, selected: bool, scale: f32) {
    let rect = Rect::from_min_size(origin, size);
    painter.rect_filled(
        rect,
        8.0 * scale,
        if selected {
            Color32::from_rgb(87, 73, 47)
        } else {
            Color32::from_rgb(40, 50, 50)
        },
    );
    if selected {
        painter.rect_stroke(
            rect,
            8.0 * scale,
            Stroke::new(2.0 * scale, GOLD),
            StrokeKind::Inside,
        );
    }
    text(
        painter,
        origin + Vec2::new(18.0, 12.0) * scale,
        23.0 * scale,
        if selected { GOLD } else { INK },
        label,
    );
}

fn draw_board(painter: &Painter, app: &App, legal_moves: &[Move], center: Pos2, cell: f32) {
    let origin = center - Vec2::new(4.0, 4.5) * cell;
    let point = |square: Square| origin + Vec2::new(square.x as f32, square.y as f32) * cell;
    let board_rect = Rect::from_min_max(
        origin - Vec2::splat(cell * 0.55),
        origin + Vec2::new(8.55, 9.55) * cell,
    );
    painter.rect_filled(
        board_rect.translate(Vec2::new(5.0, 6.0)),
        14.0,
        Color32::from_rgb(12, 21, 23),
    );
    painter.rect_filled(board_rect, 14.0, Color32::from_rgb(203, 155, 93));
    painter.rect_stroke(
        board_rect,
        14.0,
        Stroke::new(3.0, Color32::from_rgb(119, 74, 38)),
        StrokeKind::Inside,
    );
    let line = Stroke::new(1.3, Color32::from_rgb(84, 52, 31));
    for x in 0..9 {
        if x == 0 || x == 8 {
            painter.line_segment([point(Square::new(x, 0)), point(Square::new(x, 9))], line);
        } else {
            for (top, bottom) in [(0, 4), (5, 9)] {
                painter.line_segment(
                    [point(Square::new(x, top)), point(Square::new(x, bottom))],
                    line,
                );
            }
        }
    }
    for y in 0..10 {
        painter.line_segment([point(Square::new(0, y)), point(Square::new(8, y))], line);
    }
    for y in [0, 7] {
        painter.line_segment(
            [point(Square::new(3, y)), point(Square::new(5, y + 2))],
            line,
        );
        painter.line_segment(
            [point(Square::new(5, y)), point(Square::new(3, y + 2))],
            line,
        );
    }
    for (x, label) in [(2.0, "楚 河"), (6.0, "汉 界")] {
        painter.text(
            origin + Vec2::new(x, 4.5) * cell,
            Align2::CENTER_CENTER,
            label,
            FontId::proportional(cell * 0.44),
            line.color,
        );
    }
    if let Some(mv) = app.game.last_move() {
        painter.line_segment(
            [point(mv.from), point(mv.to)],
            Stroke::new(3.0, Color32::from_rgb(248, 217, 136)),
        );
        painter.circle_stroke(
            point(mv.from),
            cell * 0.23,
            Stroke::new(2.0, Color32::from_rgb(248, 217, 136)),
        );
    }
    for index in 0..90 {
        let square = Square::from_index(index);
        let Some(piece) = app.game.piece_at(square) else {
            continue;
        };
        let center = point(square);
        let ink = if piece.color == Color::Red {
            Color32::from_rgb(158, 35, 31)
        } else {
            Color32::from_rgb(38, 42, 40)
        };
        painter.circle_filled(
            center + Vec2::new(2.0, 3.0),
            cell * 0.4,
            Color32::from_rgb(123, 78, 41),
        );
        painter.circle_filled(center, cell * 0.4, Color32::from_rgb(249, 224, 174));
        painter.circle_stroke(center, cell * 0.34, Stroke::new(1.5, ink));
        let label = piece.chinese();
        painter.text(
            center,
            Align2::CENTER_CENTER,
            label,
            FontId::proportional(cell * 0.51),
            ink,
        );
    }
    if let Some(selected) = app.selected {
        painter.circle_stroke(
            point(selected),
            cell * 0.45,
            Stroke::new(3.0, Color32::from_rgb(255, 243, 147)),
        );
        for mv in legal_moves.iter().filter(|mv| mv.from == selected) {
            if app.game.piece_at(mv.to).is_some() {
                painter.circle_stroke(
                    point(mv.to),
                    cell * 0.43,
                    Stroke::new(3.0, Color32::from_rgb(177, 57, 37)),
                );
            } else {
                painter.circle_filled(point(mv.to), cell * 0.12, Color32::from_rgb(177, 57, 37));
            }
        }
    }
    if !app.setup && !app.menu {
        let cursor_rect = Rect::from_center_size(point(app.cursor), Vec2::splat(cell * 0.92));
        painter.rect_stroke(
            cursor_rect,
            6.0,
            Stroke::new(3.2, Color32::from_rgb(46, 115, 158)),
            StrokeKind::Outside,
        );
    }
}
