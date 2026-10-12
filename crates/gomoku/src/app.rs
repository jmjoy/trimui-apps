use crate::{
    ai::{Difficulty, JobKind, Reply, Worker},
    audio::{Audio, Sound},
    game::{Outcome, Position, SIZE, Stone},
    session::{Applied, Session},
};
use std::{sync::mpsc, time::Instant};

pub const SETUP_ROWS: usize = 9;
pub const MENU_ROWS: usize = 12;

#[derive(Clone, Copy, Debug)]
pub enum Action {
    Direction([i8; 2]),
    Confirm,
    Cancel,
    Menu,
    Undo,
    Hint,
}

pub struct App {
    pub session: Session,
    pub setup: bool,
    pub menu: bool,
    pub menu_row: usize,
    pub selected_human: Stone,
    pub selected_difficulty: Difficulty,
    pub cursor: Position,
    pub notice: Option<String>,
    pub audio: Audio,
    pub last_move: Instant,
    worker: Worker,
    replies: mpsc::Receiver<Reply>,
}

impl App {
    pub fn new() -> Self {
        Self::with_audio(Audio::new())
    }

    fn with_audio(audio: Audio) -> Self {
        let (sender, replies) = mpsc::channel();
        Self {
            session: Session::default(),
            setup: true,
            menu: false,
            menu_row: 0,
            selected_human: Stone::Black,
            selected_difficulty: Difficulty::Normal,
            cursor: Position::new(7, 7),
            notice: None,
            audio,
            last_move: Instant::now() - std::time::Duration::from_secs(1),
            worker: Worker::new(move |reply| {
                let _ = sender.send(reply);
            }),
            replies,
        }
    }

    pub fn start_game(&mut self) {
        self.worker.cancel();
        self.session
            .restart(self.selected_human, self.selected_difficulty);
        self.setup = false;
        self.menu = false;
        self.menu_row = 0;
        self.notice = None;
        self.cursor = Position::new(
            if self.selected_human == Stone::Black {
                7
            } else {
                8
            },
            7,
        );
        log::info!(
            "Game started: {:?}, {:?}",
            self.session.human,
            self.session.difficulty
        );
        self.request(JobKind::Move);
    }

    fn return_to_setup(&mut self) {
        self.worker.cancel();
        self.session
            .restart(self.selected_human, self.selected_difficulty);
        self.setup = true;
        self.menu = false;
        self.menu_row = 0;
        self.notice = None;
        self.cursor = Position::new(7, 7);
        log::info!("Returned to setup; AI and hints cancelled");
    }

    pub fn action(&mut self, action: Action) -> bool {
        if matches!(action, Action::Menu) {
            if self.setup {
                self.start_game();
            } else {
                self.menu = !self.menu;
                self.menu_row = 0;
            }
            return false;
        }
        if self.setup || self.menu {
            match action {
                Action::Direction([dx, dy]) => {
                    let count = if self.setup { SETUP_ROWS } else { MENU_ROWS };
                    if dy != 0 {
                        self.menu_row = (self.menu_row as i32 + i32::from(dy))
                            .rem_euclid(count as i32)
                            as usize;
                    }
                    if dx != 0 {
                        self.adjust_row(dx);
                    }
                }
                Action::Confirm => {
                    if !self.menu_row_enabled(self.menu_row) {
                        return false;
                    }
                    if self.setup {
                        match self.menu_row {
                            7 => self.start_game(),
                            8 => return true,
                            _ => self.adjust_row(1),
                        }
                    } else {
                        match self.menu_row {
                            0 => self.menu = false,
                            1 => {
                                self.undo();
                                self.menu = false;
                            }
                            2 => {
                                self.request(JobKind::Hint);
                                self.menu = false;
                            }
                            10 => self.return_to_setup(),
                            11 => return true,
                            _ => self.adjust_row(1),
                        }
                    }
                }
                Action::Cancel if self.menu => self.menu = false,
                _ => {}
            }
            return false;
        }
        match action {
            Action::Direction([dx, dy]) => {
                self.cursor.x =
                    (self.cursor.x as i32 + i32::from(dx)).clamp(0, SIZE as i32 - 1) as usize;
                self.cursor.y =
                    (self.cursor.y as i32 + i32::from(dy)).clamp(0, SIZE as i32 - 1) as usize;
                self.notice = None;
            }
            Action::Confirm => {
                if self.session.human_move(self.cursor) {
                    self.worker.cancel();
                    self.played();
                    self.request(JobKind::Move);
                } else {
                    self.notice = Some(if self.session.game.outcome != Outcome::Playing {
                        "对局已结束，可悔棋或开始新局".into()
                    } else if !self.session.human_turn() {
                        "请等待电脑落子".into()
                    } else {
                        "这里已有棋子，请选择空交叉点".into()
                    });
                }
            }
            Action::Cancel => self.notice = None,
            Action::Undo => self.undo(),
            Action::Hint => self.request(JobKind::Hint),
            Action::Menu => unreachable!(),
        }
        false
    }

    fn adjust_row(&mut self, delta: i8) {
        let row = if self.setup {
            self.menu_row
        } else {
            let Some(row) = self.menu_row.checked_sub(3) else {
                return;
            };
            row
        };
        let settings = &mut self.audio.settings;
        match row {
            0 => self.selected_human = self.selected_human.other(),
            1 => {
                let index = Difficulty::ALL
                    .iter()
                    .position(|&d| d == self.selected_difficulty)
                    .unwrap();
                self.selected_difficulty =
                    Difficulty::ALL[(index as i32 + i32::from(delta)).rem_euclid(3) as usize];
            }
            2 => settings.muted = !settings.muted,
            3 => settings.effects_enabled = !settings.effects_enabled,
            4 => {
                settings.effects_volume =
                    ((settings.effects_volume * 10.0).round() + f32::from(delta)).clamp(0.0, 10.0)
                        / 10.0
            }
            5 => settings.music_enabled = !settings.music_enabled,
            6 => {
                settings.music_volume = ((settings.music_volume * 10.0).round() + f32::from(delta))
                    .clamp(0.0, 10.0)
                    / 10.0
            }
            _ => return,
        }
        if row >= 2 {
            log::info!(
                "Audio settings: muted={}, effects={}, music={}, effects_volume={:.0}%, music_volume={:.0}%",
                settings.muted,
                settings.effects_enabled,
                settings.music_enabled,
                settings.effects_volume * 100.0,
                settings.music_volume * 100.0
            );
        }
        self.audio.update();
    }

    fn undo(&mut self) {
        if self.session.undo() {
            self.worker.cancel();
            self.notice = None;
            log::info!("Undo: {} moves remain", self.session.game.history.len());
        } else {
            self.notice = Some("还没有可以悔棋的回合".into());
        }
    }

    fn request(&mut self, kind: JobKind) {
        if let Some(request) = self.session.request(kind) {
            log::info!("AI requested: {kind:?}, revision {}", request.revision);
            self.notice = None;
            self.worker.submit(request);
        }
    }

    fn played(&mut self) {
        self.last_move = Instant::now();
        self.notice = None;
        if let Some(last) = self.session.game.history.last() {
            self.audio.play(Sound::Place(last.stone == Stone::Black));
            log::info!(
                "Move {}: {:?} {}",
                self.session.game.history.len(),
                last.stone,
                last.pos
            );
        }
        match &self.session.game.outcome {
            Outcome::Won { stone, .. } => {
                self.audio.play(if *stone == self.session.human {
                    Sound::Win
                } else {
                    Sound::Lose
                });
                log::info!("Game finished: {stone:?} won");
            }
            Outcome::Draw => {
                self.audio.play(Sound::Draw);
                log::info!("Game finished: draw");
            }
            Outcome::Playing => {}
        }
    }

    pub fn poll_ai(&mut self) {
        while let Ok(reply) = self.replies.try_recv() {
            match self.session.apply(reply) {
                Applied::Move => self.played(),
                Applied::Hint => log::info!("Hint ready: {}", self.session.hint.unwrap()),
                Applied::Failed => {
                    self.notice = Some("搜索未能完成，请悔棋或重新开始".into());
                    log::warn!("AI search failed");
                }
                Applied::Stale => {}
            }
        }
    }

    pub fn menu_row_enabled(&self, row: usize) -> bool {
        self.setup
            || match row {
                1 => self.session.game.can_undo(self.session.human),
                2 => self.session.human_turn() && self.session.pending.is_none(),
                _ => true,
            }
    }

    pub fn menu_labels(&self) -> Vec<String> {
        let s = self.audio.settings;
        let toggle = |enabled| if enabled { "开启" } else { "关闭" };
        let mut labels = vec![
            format!(
                "{}执棋：{}",
                if self.setup { "" } else { "下一局" },
                self.selected_human.name()
            ),
            format!(
                "{}难度：{}",
                if self.setup { "" } else { "下一局" },
                self.selected_difficulty.name()
            ),
            format!("静音：{}", toggle(s.muted)),
            format!("音效：{}", toggle(s.effects_enabled)),
            format!("音效音量：{:.0}%", s.effects_volume * 100.0),
            format!("音乐：{}", toggle(s.music_enabled)),
            format!("音乐音量：{:.0}%", s.music_volume * 100.0),
        ];
        if self.setup {
            labels.extend(["开始对局  /  START".into(), "退出".into()]);
        } else {
            labels.splice(0..0, ["继续对局".into(), "悔棋".into(), "请求提示".into()]);
            labels.extend(["重新开始".into(), "退出".into()]);
        }
        labels
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn app() -> App {
        App::with_audio(Audio::silent())
    }

    fn wait_ai(app: &mut App) {
        let deadline = Instant::now() + Duration::from_secs(3);
        while app.session.pending.is_some() && Instant::now() < deadline {
            app.poll_ai();
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(app.session.pending.is_none());
    }

    #[test]
    fn controller_moves_hints_and_undoes_for_both_colors() {
        for human in [Stone::Black, Stone::White] {
            let mut app = app();
            app.selected_human = human;
            app.action(Action::Menu);
            wait_ai(&mut app);
            let opening = usize::from(human == Stone::White);
            assert_eq!(app.session.game.history.len(), opening);
            app.action(Action::Hint);
            wait_ai(&mut app);
            assert!(app.session.hint.is_some());
            assert_eq!(app.session.game.history.len(), opening);
            app.action(Action::Confirm);
            wait_ai(&mut app);
            assert_eq!(app.session.game.history.len(), opening + 2);
            app.action(Action::Undo);
            assert_eq!(app.session.game.history.len(), opening);
            assert!(app.session.human_turn());
            assert!(app.session.hint.is_none());
        }
    }

    #[test]
    fn undo_during_ai_and_restart_discard_pending_results() {
        let mut app = app();
        app.action(Action::Menu);
        app.action(Action::Confirm);
        assert!(app.session.pending.is_some());
        app.action(Action::Undo);
        app.poll_ai();
        assert!(app.session.game.history.is_empty());
        assert!(app.session.pending.is_none());
        app.action(Action::Hint);
        app.action(Action::Confirm);
        assert_eq!(app.session.game.history.len(), 1);
        assert_eq!(app.session.pending.unwrap().kind, JobKind::Move);
        app.action(Action::Menu);
        app.menu_row = 10;
        app.action(Action::Confirm);
        app.poll_ai();
        assert!(app.setup);
        assert!(app.session.game.history.is_empty());
        assert!(app.session.pending.is_none());
    }

    #[test]
    fn cursor_clamps_and_occupied_intersections_reject_moves() {
        let mut app = app();
        app.action(Action::Menu);
        for _ in 0..30 {
            app.action(Action::Direction([1, -1]));
        }
        assert_eq!(app.cursor, Position::new(14, 0));
        app.action(Action::Confirm);
        wait_ai(&mut app);
        app.action(Action::Confirm);
        assert_eq!(app.session.game.history.len(), 2);
        assert!(app.notice.is_some());
    }

    #[test]
    fn menus_apply_audio_now_and_game_settings_at_restart() {
        let mut app = app();
        assert_eq!(app.menu_labels().len(), SETUP_ROWS);
        app.action(Action::Menu);
        app.action(Action::Menu);
        assert_eq!(app.menu_labels().len(), MENU_ROWS);
        app.menu_row = 3;
        app.action(Action::Direction([1, 0]));
        assert_eq!(app.selected_human, Stone::White);
        assert_eq!(app.session.human, Stone::Black);
        app.menu_row = 7;
        for _ in 0..20 {
            app.action(Action::Direction([-1, 0]));
        }
        assert_eq!(app.audio.settings.effects_volume, 0.0);
        app.menu_row = 8;
        app.action(Action::Confirm);
        assert!(!app.audio.settings.music_enabled);
        app.action(Action::Cancel);
        assert!(!app.menu);
        app.action(Action::Menu);
        app.menu_row = 10;
        app.action(Action::Confirm);
        app.action(Action::Menu);
        wait_ai(&mut app);
        assert_eq!(app.session.human, Stone::White);
        app.action(Action::Menu);
        app.menu_row = 11;
        assert!(app.action(Action::Confirm));
    }
}
