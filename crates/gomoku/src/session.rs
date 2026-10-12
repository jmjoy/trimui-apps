use crate::{
    ai::{Difficulty, JobKind, Reply, Request},
    game::{Game, Outcome, Position, Stone},
};

#[derive(Clone, Copy, Debug)]
pub struct Pending {
    pub id: u64,
    pub revision: u64,
    pub kind: JobKind,
}

pub struct Session {
    pub game: Game,
    pub human: Stone,
    pub difficulty: Difficulty,
    pub hint: Option<Position>,
    pub pending: Option<Pending>,
    revision: u64,
    sequence: u64,
}
#[derive(Debug, PartialEq, Eq)]
pub enum Applied {
    Stale,
    Hint,
    Move,
    Failed,
}

impl Default for Session {
    fn default() -> Self {
        Self {
            game: Game::default(),
            human: Stone::Black,
            difficulty: Difficulty::Normal,
            hint: None,
            pending: None,
            revision: 0,
            sequence: 0,
        }
    }
}
impl Session {
    fn changed(&mut self) {
        self.revision += 1;
        self.hint = None;
        self.pending = None;
    }
    pub fn restart(&mut self, human: Stone, difficulty: Difficulty) {
        self.game = Game::default();
        self.human = human;
        self.difficulty = difficulty;
        self.changed();
    }
    pub fn human_turn(&self) -> bool {
        self.game.outcome == Outcome::Playing && self.game.turn() == self.human
    }
    pub fn human_move(&mut self, pos: Position) -> bool {
        if !self.human_turn() || self.game.play(pos).is_err() {
            return false;
        }
        self.changed();
        true
    }
    pub fn undo(&mut self) -> bool {
        if !self.game.undo(self.human) {
            return false;
        }
        self.changed();
        true
    }
    pub fn request(&mut self, kind: JobKind) -> Option<Request> {
        if self.game.outcome != Outcome::Playing || self.pending.is_some() {
            return None;
        }
        match kind {
            JobKind::Move if self.human_turn() => return None,
            JobKind::Hint if !self.human_turn() => return None,
            _ => {}
        }
        self.sequence += 1;
        self.pending = Some(Pending {
            id: self.sequence,
            revision: self.revision,
            kind,
        });
        Some(Request {
            id: self.sequence,
            revision: self.revision,
            kind,
            board: self.game.board.clone(),
            stone: self.game.turn(),
            difficulty: if kind == JobKind::Hint {
                Difficulty::Hard
            } else {
                self.difficulty
            },
        })
    }
    pub fn apply(&mut self, reply: Reply) -> Applied {
        let Some(pending) = self.pending else {
            return Applied::Stale;
        };
        if pending.id != reply.id
            || pending.revision != reply.revision
            || pending.kind != reply.kind
            || self.revision != reply.revision
        {
            return Applied::Stale;
        }
        self.pending = None;
        let Some(pos) = reply.position.filter(|&p| self.game.board.vacant(p)) else {
            return Applied::Failed;
        };
        match reply.kind {
            JobKind::Hint if self.human_turn() => {
                self.hint = Some(pos);
                Applied::Hint
            }
            JobKind::Move if !self.human_turn() && self.game.play(pos).is_ok() => {
                self.changed();
                Applied::Move
            }
            _ => Applied::Failed,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn reply(request: &Request, position: Position) -> Reply {
        Reply {
            id: request.id,
            revision: request.revision,
            kind: request.kind,
            position: Some(position),
        }
    }
    #[test]
    fn restart_and_undo_reject_old_results() {
        let mut session = Session::default();
        session.human_move(Position::new(7, 7));
        let old = session.request(JobKind::Move).unwrap();
        session.undo();
        assert_eq!(
            session.apply(reply(&old, Position::new(6, 6))),
            Applied::Stale
        );
        session.restart(Stone::White, Difficulty::Hard);
        let new = session.request(JobKind::Move).unwrap();
        assert_eq!(
            session.apply(reply(&old, Position::new(6, 6))),
            Applied::Stale
        );
        assert_eq!(
            session.apply(reply(&new, Position::new(7, 7))),
            Applied::Move
        );
        assert!(session.human_turn());
    }
    #[test]
    fn hints_never_place_stones_and_expire_when_player_moves() {
        let mut session = Session::default();
        let request = session.request(JobKind::Hint).unwrap();
        assert_eq!(
            session.apply(reply(&request, Position::new(7, 7))),
            Applied::Hint
        );
        assert!(session.game.board.empty());
        let old = session.request(JobKind::Hint).unwrap();
        assert!(session.human_move(Position::new(7, 7)));
        assert_eq!(session.hint, None);
        let ai = session.request(JobKind::Move).unwrap();
        assert_eq!(
            session.apply(reply(&old, Position::new(6, 6))),
            Applied::Stale
        );
        assert_eq!(
            session.apply(reply(&ai, Position::new(8, 7))),
            Applied::Move
        );
        assert!(!session.human_move(Position::new(8, 7)));
    }

    #[test]
    fn computer_win_stops_input_and_undo_restores_human_turn() {
        let mut session = Session::default();
        for x in 0..4 {
            session.game.play(Position::new(x * 2, 10)).unwrap();
            session.game.play(Position::new(x, 0)).unwrap();
        }
        session.human_move(Position::new(10, 10));
        let request = session.request(JobKind::Move).unwrap();
        let winning_reply = reply(&request, Position::new(4, 0));
        assert_eq!(session.apply(winning_reply.clone()), Applied::Move);
        assert!(matches!(
            session.game.outcome,
            Outcome::Won {
                stone: Stone::White,
                ..
            }
        ));
        assert!(!session.human_move(Position::new(7, 7)));
        assert!(session.request(JobKind::Hint).is_none());
        assert!(session.undo());
        assert!(session.human_turn());
        assert_eq!(session.game.history.len(), 8);
        assert_eq!(session.apply(winning_reply), Applied::Stale);
    }
}
