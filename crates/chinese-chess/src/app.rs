use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
    mpsc::{self, Receiver},
};
use std::time::Instant;

use crate::{
    ai,
    audio::{AudioEngine, AudioEvent},
    game::{Color, Difficulty, GameState, GameStatus, Move, Square},
};

pub enum Action {
    Direction([i8; 2]),
    Confirm,
    Cancel,
    Menu,
    Undo,
    NextTarget,
}

struct AiTask {
    cancelled: Arc<AtomicBool>,
    result: Receiver<Option<Move>>,
}

impl Drop for AiTask {
    fn drop(&mut self) {
        self.cancelled.store(true, Ordering::Relaxed);
    }
}

pub struct App {
    pub game: GameState,
    pub setup: bool,
    pub menu: bool,
    pub menu_row: usize,
    pub player_color: Color,
    pub difficulty: Difficulty,
    pub cursor: Square,
    pub selected: Option<Square>,
    pub notice: Option<String>,
    pub audio: AudioEngine,
    ai: Option<AiTask>,
}

impl App {
    pub fn new() -> Self {
        Self {
            game: GameState::new(),
            setup: true,
            menu: false,
            menu_row: 0,
            player_color: Color::Red,
            difficulty: Difficulty::Normal,
            cursor: Square::new(4, 6),
            selected: None,
            notice: None,
            audio: AudioEngine::new(),
            ai: None,
        }
    }

    pub fn ai_thinking(&self) -> bool {
        self.ai.is_some()
    }

    fn change_difficulty(&mut self, delta: i8) {
        let index = match self.difficulty {
            Difficulty::Easy => 0,
            Difficulty::Normal => 1,
            Difficulty::Hard => 2,
        };
        self.difficulty = [Difficulty::Easy, Difficulty::Normal, Difficulty::Hard]
            [(index + delta as i32).rem_euclid(3) as usize];
    }

    pub fn start_game(&mut self) {
        self.ai = None;
        self.game.reset();
        self.setup = false;
        self.menu = false;
        self.selected = None;
        self.notice = None;
        self.cursor = if self.player_color == Color::Red {
            Square::new(4, 6)
        } else {
            Square::new(4, 3)
        };
        log::info!(
            "Game started: {:?}, {:?}",
            self.player_color,
            self.difficulty
        );
        self.maybe_start_ai();
    }

    fn new_game(&mut self) {
        self.ai = None;
        self.setup = true;
        self.menu = false;
        self.menu_row = 0;
        self.game.reset();
        self.selected = None;
        self.notice = None;
        log::info!("Returned to setup; AI cancelled");
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
            let row_count = if self.setup { 4 } else { 6 };
            match action {
                Action::Direction([dx, dy]) => {
                    if dy != 0 {
                        self.menu_row =
                            (self.menu_row as i32 + dy as i32).rem_euclid(row_count) as usize;
                    }
                    if dx != 0 {
                        self.adjust_row(dx);
                    }
                }
                Action::Confirm => {
                    if self.setup {
                        if self.menu_row == 3 {
                            self.start_game();
                        } else {
                            self.adjust_row(1);
                        }
                    } else {
                        match self.menu_row {
                            0 => self.menu = false,
                            1 => {
                                self.undo();
                                self.menu = false;
                            }
                            2 | 3 => self.adjust_row(1),
                            4 => self.new_game(),
                            5 => return true,
                            _ => unreachable!(),
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
                self.cursor.x = (self.cursor.x as i32 + dx as i32).clamp(0, 8) as usize;
                self.cursor.y = (self.cursor.y as i32 + dy as i32).clamp(0, 9) as usize;
                self.notice = None;
            }
            Action::Confirm => self.confirm_square(),
            Action::Cancel => {
                self.selected = None;
                self.notice = None;
            }
            Action::Undo => self.undo(),
            Action::NextTarget => {
                if let Some(selected) = self.selected {
                    let targets: Vec<_> = self
                        .game
                        .legal_moves()
                        .iter()
                        .filter(|mv| mv.from == selected)
                        .map(|mv| mv.to)
                        .collect();
                    if !targets.is_empty() {
                        let next = targets
                            .iter()
                            .position(|&s| s == self.cursor)
                            .map_or(0, |i| (i + 1) % targets.len());
                        self.cursor = targets[next];
                    }
                }
            }
            Action::Menu => unreachable!(),
        }
        false
    }

    fn adjust_row(&mut self, delta: i8) {
        match (self.setup, self.menu_row) {
            (true, 0) => self.player_color = self.player_color.other(),
            (true, 1) | (false, 2) => self.change_difficulty(delta),
            (true, 2) | (false, 3) => self.audio.set_enabled(!self.audio.enabled()),
            _ => {}
        }
    }

    fn confirm_square(&mut self) {
        if self.ai_thinking()
            || self.game.status().is_finished()
            || self.game.side_to_move() != self.player_color
        {
            return;
        }
        let own_piece = self
            .game
            .piece_at(self.cursor)
            .is_some_and(|p| p.color == self.player_color);
        if let Some(selected) = self.selected {
            if selected == self.cursor {
                self.selected = None;
                return;
            }
            let mv = Move {
                from: selected,
                to: self.cursor,
            };
            if self.game.legal_moves().contains(&mv) {
                self.commit_move(mv);
            } else if own_piece {
                self.selected = Some(self.cursor);
            } else {
                self.audio.play(AudioEvent::Invalid);
                self.notice = Some("这步棋不合法".into());
            }
        } else if own_piece {
            self.selected = Some(self.cursor);
        } else {
            self.audio.play(AudioEvent::Invalid);
            self.notice = Some("请选择己方棋子".into());
        }
    }

    fn commit_move(&mut self, mv: Move) {
        let captured = self.game.piece_at(mv.to).is_some();
        if let Err(error) = self.game.apply_move(mv) {
            self.notice = Some(error);
            return;
        }
        self.selected = None;
        self.notice = None;
        self.audio.play(if captured {
            AudioEvent::Capture
        } else {
            AudioEvent::Move
        });
        let record = self.game.history().last().unwrap();
        log::info!(
            "Move {}: {:?} {}",
            self.game.history().len(),
            record.mover,
            record.notation
        );
        if record.gave_check {
            self.audio.play(AudioEvent::Check);
        }
        match self.game.status() {
            GameStatus::Playing => {}
            GameStatus::DrawRepetition => self.audio.play(AudioEvent::Draw),
            GameStatus::Checkmate { winner } | GameStatus::Stalemate { winner } => {
                self.audio.play(if winner == self.player_color {
                    AudioEvent::Victory
                } else {
                    AudioEvent::Defeat
                })
            }
            GameStatus::PerpetualCheckLoss { loser } => {
                self.audio.play(if loser == self.player_color {
                    AudioEvent::Defeat
                } else {
                    AudioEvent::Victory
                })
            }
        }
        self.maybe_start_ai();
    }

    fn undo(&mut self) {
        if self.ai_thinking() {
            self.notice = Some("请等电脑落子后悔棋".into());
            return;
        }
        if self.game.side_to_move() != self.player_color {
            return;
        }
        if self.game.undo_round() {
            self.selected = None;
            self.notice = Some("已悔棋".into());
            log::info!("Undo: {} moves remain", self.game.history().len());
            self.maybe_start_ai();
        }
    }

    fn maybe_start_ai(&mut self) {
        if self.ai_thinking()
            || self.game.status().is_finished()
            || self.game.side_to_move() == self.player_color
        {
            return;
        }
        let state = self.game.search_position();
        let difficulty = self.difficulty;
        let cancelled = Arc::new(AtomicBool::new(false));
        let worker_cancelled = cancelled.clone();
        let (sender, result) = mpsc::channel();
        std::thread::spawn(move || {
            let start = Instant::now();
            let mv = ai::search_best_move(&state, difficulty, &worker_cancelled);
            log::info!("AI finished in {:.3}s", start.elapsed().as_secs_f64());
            let _ = sender.send(mv);
        });
        self.ai = Some(AiTask { cancelled, result });
    }

    pub fn poll_ai(&mut self) {
        let result = self.ai.as_ref().map(|task| task.result.try_recv());
        match result {
            Some(Ok(mv)) => {
                self.ai = None;
                if let Some(mv) = mv {
                    self.commit_move(mv);
                }
            }
            Some(Err(mpsc::TryRecvError::Disconnected)) => {
                self.ai = None;
                self.notice = Some("电脑计算中断，请重新开始".into());
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn controller_can_select_move_cancel_and_undo_a_round() {
        let mut app = App::new();
        app.difficulty = Difficulty::Easy;
        app.action(Action::Menu);
        app.action(Action::Confirm);
        app.action(Action::Cancel);
        assert!(app.selected.is_none());
        app.action(Action::Confirm);
        app.action(Action::Direction([0, -1]));
        app.action(Action::Confirm);
        assert_eq!(app.game.history().len(), 1);
        assert!(app.ai_thinking());
        let start = Instant::now();
        while app.ai_thinking() && start.elapsed().as_secs() < 3 {
            app.poll_ai();
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert_eq!(app.game.history().len(), 2);
        app.action(Action::Undo);
        assert!(app.game.history().is_empty());
        assert_eq!(app.game.side_to_move(), Color::Red);
    }

    #[test]
    fn restart_discards_a_pending_ai_result() {
        let mut app = App::new();
        app.player_color = Color::Black;
        app.start_game();
        let cancelled = app.ai.as_ref().unwrap().cancelled.clone();
        app.new_game();
        assert!(cancelled.load(Ordering::Relaxed));
        app.poll_ai();
        assert!(app.setup);
        assert!(app.game.history().is_empty());
    }
}
