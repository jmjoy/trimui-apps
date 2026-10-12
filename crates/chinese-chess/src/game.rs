use std::collections::HashMap;

pub const BOARD_WIDTH: usize = 9;
pub const BOARD_HEIGHT: usize = 10;
pub const BOARD_SIZE: usize = BOARD_WIDTH * BOARD_HEIGHT;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Color {
    Red,
    Black,
}

impl Color {
    pub fn other(self) -> Self {
        match self {
            Self::Red => Self::Black,
            Self::Black => Self::Red,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Red => "红方",
            Self::Black => "黑方",
        }
    }

    fn forward(self) -> i32 {
        match self {
            Self::Red => -1,
            Self::Black => 1,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PieceKind {
    King,
    Advisor,
    Elephant,
    Horse,
    Rook,
    Cannon,
    Pawn,
}

impl PieceKind {
    pub fn chinese(self) -> &'static str {
        match self {
            Self::King => "将",
            Self::Advisor => "士",
            Self::Elephant => "象",
            Self::Horse => "马",
            Self::Rook => "车",
            Self::Cannon => "炮",
            Self::Pawn => "卒",
        }
    }

    fn code(self) -> u8 {
        match self {
            Self::King => 1,
            Self::Advisor => 2,
            Self::Elephant => 3,
            Self::Horse => 4,
            Self::Rook => 5,
            Self::Cannon => 6,
            Self::Pawn => 7,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Piece {
    pub color: Color,
    pub kind: PieceKind,
}

impl Piece {
    pub fn chinese(self) -> &'static str {
        match (self.color, self.kind) {
            (Color::Red, PieceKind::King) => "帅",
            (Color::Red, PieceKind::Advisor) => "仕",
            (Color::Red, PieceKind::Elephant) => "相",
            (Color::Red, PieceKind::Pawn) => "兵",
            _ => self.kind.chinese(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Square {
    pub x: usize,
    pub y: usize,
}

impl Square {
    pub const fn new(x: usize, y: usize) -> Self {
        Self { x, y }
    }

    pub fn index(self) -> usize {
        self.y * BOARD_WIDTH + self.x
    }

    pub fn in_bounds(x: i32, y: i32) -> bool {
        (0..BOARD_WIDTH as i32).contains(&x) && (0..BOARD_HEIGHT as i32).contains(&y)
    }

    pub fn from_index(index: usize) -> Self {
        Self::new(index % BOARD_WIDTH, index / BOARD_WIDTH)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Move {
    pub from: Square,
    pub to: Square,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GameStatus {
    Playing,
    Checkmate { winner: Color },
    Stalemate { winner: Color },
    DrawRepetition,
    PerpetualCheckLoss { loser: Color },
}

impl GameStatus {
    pub fn is_finished(self) -> bool {
        !matches!(self, Self::Playing)
    }

    pub fn description(self) -> String {
        match self {
            Self::Playing => "对局进行中".to_owned(),
            Self::Checkmate { winner } => format!("{}将死获胜", winner.label()),
            Self::Stalemate { winner } => format!("{}困毙获胜", winner.label()),
            Self::DrawRepetition => "三次重复，和棋".to_owned(),
            Self::PerpetualCheckLoss { loser } => format!("{}长将，判负", loser.label()),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Difficulty {
    Easy,
    Normal,
    Hard,
}

impl Difficulty {
    pub fn depth(self) -> u8 {
        match self {
            Self::Easy => 2,
            Self::Normal => 3,
            Self::Hard => 4,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Easy => "简单",
            Self::Normal => "普通",
            Self::Hard => "困难",
        }
    }
}

#[derive(Clone, Debug)]
pub struct MoveRecord {
    pub mv: Move,
    pub mover: Color,
    pub captured: Option<Piece>,
    pub notation: String,
    pub gave_check: bool,
    before_board: [Option<Piece>; BOARD_SIZE],
    before_side: Color,
    before_status: GameStatus,
}

type PositionKey = [u8; BOARD_SIZE + 1];

#[derive(Clone)]
pub struct GameState {
    board: [Option<Piece>; BOARD_SIZE],
    side_to_move: Color,
    status: GameStatus,
    history: Vec<MoveRecord>,
    keys: Vec<PositionKey>,
    repetitions: HashMap<PositionKey, usize>,
}

impl GameState {
    pub fn new() -> Self {
        let mut state = Self {
            board: [None; BOARD_SIZE],
            side_to_move: Color::Red,
            status: GameStatus::Playing,
            history: Vec::new(),
            keys: Vec::new(),
            repetitions: HashMap::new(),
        };
        state.setup_initial_position();
        let key = state.position_key();
        state.keys.push(key);
        state.repetitions.insert(key, 1);
        state
    }

    pub fn piece_at(&self, square: Square) -> Option<Piece> {
        self.board.get(square.index()).copied().flatten()
    }

    pub fn side_to_move(&self) -> Color {
        self.side_to_move
    }

    pub fn status(&self) -> GameStatus {
        self.status
    }

    pub fn history(&self) -> &[MoveRecord] {
        &self.history
    }

    pub fn last_move(&self) -> Option<Move> {
        self.history.last().map(|record| record.mv)
    }

    pub fn is_in_check(&self, color: Color) -> bool {
        let king = self.board.iter().enumerate().find_map(|(index, piece)| {
            (*piece
                == Some(Piece {
                    color,
                    kind: PieceKind::King,
                }))
            .then(|| Square::from_index(index))
        });
        king.is_some_and(|square| self.is_square_attacked(square, color.other()))
    }

    pub fn legal_moves(&self) -> Vec<Move> {
        if self.status.is_finished() {
            return Vec::new();
        }
        self.legal_moves_for(self.side_to_move)
    }

    pub fn legal_moves_for(&self, color: Color) -> Vec<Move> {
        let mut legal = Vec::new();
        for mv in self.pseudo_moves_for(color) {
            let mut next = self.search_position();
            next.apply_for_search(mv);
            if !next.is_in_check(color) {
                legal.push(mv);
            }
        }
        legal
    }

    pub fn apply_move(&mut self, mv: Move) -> Result<(), String> {
        if self.status.is_finished() {
            return Err("对局已经结束".to_owned());
        }
        if !self.legal_moves().contains(&mv) {
            return Err("这步棋不合法".to_owned());
        }
        self.apply_unchecked(mv, true);
        Ok(())
    }

    pub(crate) fn apply_for_search(&mut self, mv: Move) {
        let moving = self
            .piece_at(mv.from)
            .expect("search move must have a piece");
        self.board[mv.from.index()] = None;
        self.board[mv.to.index()] = Some(moving);
        self.side_to_move = self.side_to_move.other();
        self.status = GameStatus::Playing;
    }

    // Search needs only the board and side; avoid copying an entire game's
    // history and repetition map for every node on the Cortex-A53.
    pub(crate) fn search_position(&self) -> Self {
        Self {
            board: self.board,
            side_to_move: self.side_to_move,
            status: self.status,
            history: Vec::new(),
            keys: Vec::new(),
            repetitions: HashMap::new(),
        }
    }

    pub fn undo_round(&mut self) -> bool {
        let target = self.history.len().saturating_sub(2);
        let Some(record) = self.history.get(target) else {
            if target == 0 && self.history.is_empty() {
                return false;
            }
            return false;
        };

        self.board = record.before_board;
        self.side_to_move = record.before_side;
        self.status = record.before_status;
        self.history.truncate(target);
        self.keys.truncate(target + 1);
        self.rebuild_repetitions();
        true
    }

    pub fn reset(&mut self) {
        *self = Self::new();
    }

    pub fn format_square(square: Square) -> String {
        format!("{}{}", (b'a' + square.x as u8) as char, 10 - square.y)
    }

    fn setup_initial_position(&mut self) {
        let back_rank = [
            PieceKind::Rook,
            PieceKind::Horse,
            PieceKind::Elephant,
            PieceKind::Advisor,
            PieceKind::King,
            PieceKind::Advisor,
            PieceKind::Elephant,
            PieceKind::Horse,
            PieceKind::Rook,
        ];
        for (x, kind) in back_rank.into_iter().enumerate() {
            self.board[Square::new(x, 0).index()] = Some(Piece {
                color: Color::Black,
                kind,
            });
            self.board[Square::new(x, 9).index()] = Some(Piece {
                color: Color::Red,
                kind,
            });
        }
        for x in [1, 7] {
            self.board[Square::new(x, 2).index()] = Some(Piece {
                color: Color::Black,
                kind: PieceKind::Cannon,
            });
            self.board[Square::new(x, 7).index()] = Some(Piece {
                color: Color::Red,
                kind: PieceKind::Cannon,
            });
        }
        for x in [0, 2, 4, 6, 8] {
            self.board[Square::new(x, 3).index()] = Some(Piece {
                color: Color::Black,
                kind: PieceKind::Pawn,
            });
            self.board[Square::new(x, 6).index()] = Some(Piece {
                color: Color::Red,
                kind: PieceKind::Pawn,
            });
        }
    }

    fn apply_unchecked(&mut self, mv: Move, record_history: bool) {
        let moving = self.piece_at(mv.from).expect("moving piece must exist");
        let captured = self.piece_at(mv.to);
        let before_board = self.board;
        let before_side = self.side_to_move;
        let before_status = self.status;

        self.board[mv.from.index()] = None;
        self.board[mv.to.index()] = Some(moving);
        self.side_to_move = self.side_to_move.other();
        self.status = GameStatus::Playing;

        let gave_check = self.is_in_check(self.side_to_move);
        let key = self.position_key();
        self.keys.push(key);
        *self.repetitions.entry(key).or_insert(0) += 1;

        if record_history {
            self.history.push(MoveRecord {
                mv,
                mover: moving.color,
                captured,
                notation: self.format_move(mv, moving, captured),
                gave_check,
                before_board,
                before_side,
                before_status,
            });
            self.update_status_after_move();
        }
    }

    fn update_status_after_move(&mut self) {
        let key = *self.keys.last().expect("initial key exists");
        let repetition_count = self.repetitions.get(&key).copied().unwrap_or(0);
        if repetition_count >= 3 {
            if let Some(loser) = self.perpetual_check_loser() {
                self.status = GameStatus::PerpetualCheckLoss { loser };
            } else {
                self.status = GameStatus::DrawRepetition;
            }
            return;
        }

        let replies = self.legal_moves_for(self.side_to_move);
        if replies.is_empty() {
            self.status = if self.is_in_check(self.side_to_move) {
                GameStatus::Checkmate {
                    winner: self.side_to_move.other(),
                }
            } else {
                GameStatus::Stalemate {
                    winner: self.side_to_move.other(),
                }
            };
        }
    }

    fn perpetual_check_loser(&self) -> Option<Color> {
        let current = self.keys.last()?;
        let previous = self
            .keys
            .iter()
            .enumerate()
            .rev()
            .skip(1)
            .find(|(_, key)| *key == current)
            .map(|(index, _)| index)?;
        let cycle = &self.history[previous..];
        let checking_color = cycle.iter().find(|record| record.gave_check)?.mover;
        let all_checks_same = cycle
            .iter()
            .filter(|record| record.mover == checking_color)
            .all(|record| record.gave_check);
        all_checks_same.then_some(checking_color)
    }

    fn rebuild_repetitions(&mut self) {
        self.repetitions.clear();
        for key in &self.keys {
            *self.repetitions.entry(*key).or_insert(0) += 1;
        }
    }

    fn format_move(&self, mv: Move, piece: Piece, captured: Option<Piece>) -> String {
        let capture = captured.is_some_and(|target| target.color != piece.color);
        format!(
            "{} {} {}{}",
            piece.chinese(),
            Self::format_square(mv.from),
            if capture { "吃" } else { "到" },
            Self::format_square(mv.to)
        )
    }

    fn position_key(&self) -> PositionKey {
        let mut key = [0u8; BOARD_SIZE + 1];
        for (index, piece) in self.board.iter().enumerate() {
            key[index] = piece.map_or(0, |piece| {
                piece.kind.code() + if piece.color == Color::Red { 8 } else { 0 }
            });
        }
        key[BOARD_SIZE] = if self.side_to_move == Color::Red {
            1
        } else {
            2
        };
        key
    }

    fn pseudo_moves_for(&self, color: Color) -> Vec<Move> {
        let mut moves = Vec::new();
        for (index, piece) in self.board.iter().enumerate() {
            let Some(piece) = piece else { continue };
            if piece.color != color {
                continue;
            }
            let from = Square::from_index(index);
            match piece.kind {
                PieceKind::King => self.push_king_moves(from, *piece, &mut moves),
                PieceKind::Advisor => self.push_advisor_moves(from, *piece, &mut moves),
                PieceKind::Elephant => self.push_elephant_moves(from, *piece, &mut moves),
                PieceKind::Horse => self.push_horse_moves(from, *piece, &mut moves),
                PieceKind::Rook => self.push_slider_moves(from, *piece, false, &mut moves),
                PieceKind::Cannon => self.push_slider_moves(from, *piece, true, &mut moves),
                PieceKind::Pawn => self.push_pawn_moves(from, *piece, &mut moves),
            }
        }
        moves
    }

    fn can_land(&self, square: Square, color: Color) -> bool {
        self.piece_at(square)
            .is_none_or(|piece| piece.color != color && piece.kind != PieceKind::King)
    }

    fn push_king_moves(&self, from: Square, piece: Piece, moves: &mut Vec<Move>) {
        for (dx, dy) in [(1, 0), (-1, 0), (0, 1), (0, -1)] {
            let x = from.x as i32 + dx;
            let y = from.y as i32 + dy;
            if Square::in_bounds(x, y) {
                let to = Square::new(x as usize, y as usize);
                if in_palace(to, piece.color) && self.can_land(to, piece.color) {
                    moves.push(Move { from, to });
                }
            }
        }
    }

    fn push_advisor_moves(&self, from: Square, piece: Piece, moves: &mut Vec<Move>) {
        for (dx, dy) in [(1, 1), (1, -1), (-1, 1), (-1, -1)] {
            let x = from.x as i32 + dx;
            let y = from.y as i32 + dy;
            if Square::in_bounds(x, y) {
                let to = Square::new(x as usize, y as usize);
                if in_palace(to, piece.color) && self.can_land(to, piece.color) {
                    moves.push(Move { from, to });
                }
            }
        }
    }

    fn push_elephant_moves(&self, from: Square, piece: Piece, moves: &mut Vec<Move>) {
        for (dx, dy) in [(2, 2), (2, -2), (-2, 2), (-2, -2)] {
            let x = from.x as i32 + dx;
            let y = from.y as i32 + dy;
            let mx = from.x as i32 + dx / 2;
            let my = from.y as i32 + dy / 2;
            if !Square::in_bounds(x, y)
                || self
                    .piece_at(Square::new(mx as usize, my as usize))
                    .is_some()
            {
                continue;
            }
            let to = Square::new(x as usize, y as usize);
            if elephant_side(to, piece.color) && self.can_land(to, piece.color) {
                moves.push(Move { from, to });
            }
        }
    }

    fn push_horse_moves(&self, from: Square, piece: Piece, moves: &mut Vec<Move>) {
        for (dx, dy, lx, ly) in [
            (2, 1, 1, 0),
            (2, -1, 1, 0),
            (-2, 1, -1, 0),
            (-2, -1, -1, 0),
            (1, 2, 0, 1),
            (-1, 2, 0, 1),
            (1, -2, 0, -1),
            (-1, -2, 0, -1),
        ] {
            let x = from.x as i32 + dx;
            let y = from.y as i32 + dy;
            let leg_x = from.x as i32 + lx;
            let leg_y = from.y as i32 + ly;
            if !Square::in_bounds(x, y)
                || !Square::in_bounds(leg_x, leg_y)
                || self
                    .piece_at(Square::new(leg_x as usize, leg_y as usize))
                    .is_some()
            {
                continue;
            }
            let to = Square::new(x as usize, y as usize);
            if self.can_land(to, piece.color) {
                moves.push(Move { from, to });
            }
        }
    }

    fn push_slider_moves(&self, from: Square, piece: Piece, cannon: bool, moves: &mut Vec<Move>) {
        for (dx, dy) in [(1, 0), (-1, 0), (0, 1), (0, -1)] {
            let mut x = from.x as i32 + dx;
            let mut y = from.y as i32 + dy;
            let mut screen = false;
            while Square::in_bounds(x, y) {
                let to = Square::new(x as usize, y as usize);
                match self.piece_at(to) {
                    None if !screen => moves.push(Move { from, to }),
                    None => {}
                    Some(target) if !screen => {
                        if cannon {
                            screen = true;
                        } else if target.color != piece.color && target.kind != PieceKind::King {
                            moves.push(Move { from, to });
                            break;
                        } else {
                            break;
                        }
                    }
                    Some(target) if cannon && screen => {
                        if target.color != piece.color && target.kind != PieceKind::King {
                            moves.push(Move { from, to });
                        }
                        break;
                    }
                    Some(_) => break,
                }
                x += dx;
                y += dy;
            }
        }
    }

    fn push_pawn_moves(&self, from: Square, piece: Piece, moves: &mut Vec<Move>) {
        let forward_y = from.y as i32 + piece.color.forward();
        if Square::in_bounds(from.x as i32, forward_y) {
            let to = Square::new(from.x, forward_y as usize);
            if self.can_land(to, piece.color) {
                moves.push(Move { from, to });
            }
        }
        if crossed_river(from, piece.color) {
            for dx in [-1, 1] {
                let x = from.x as i32 + dx;
                if Square::in_bounds(x, from.y as i32) {
                    let to = Square::new(x as usize, from.y);
                    if self.can_land(to, piece.color) {
                        moves.push(Move { from, to });
                    }
                }
            }
        }
    }

    fn is_square_attacked(&self, target: Square, by_color: Color) -> bool {
        for (index, piece) in self.board.iter().enumerate() {
            let Some(piece) = piece else { continue };
            if piece.color != by_color {
                continue;
            }
            let from = Square::from_index(index);
            let dx = target.x as i32 - from.x as i32;
            let dy = target.y as i32 - from.y as i32;
            let attacked = match piece.kind {
                PieceKind::King => {
                    (dx.abs() + dy.abs() == 1) || (dx == 0 && clear_between(self, from, target))
                }
                PieceKind::Advisor => dx.abs() == 1 && dy.abs() == 1 && in_palace(target, by_color),
                PieceKind::Elephant => {
                    dx.abs() == 2
                        && dy.abs() == 2
                        && elephant_side(target, by_color)
                        && self
                            .piece_at(Square::new(
                                ((from.x as i32 + target.x as i32) / 2) as usize,
                                ((from.y as i32 + target.y as i32) / 2) as usize,
                            ))
                            .is_none()
                }
                PieceKind::Horse => horse_attacks(self, from, target),
                PieceKind::Rook => (dx == 0 || dy == 0) && clear_between(self, from, target),
                PieceKind::Cannon => (dx == 0 || dy == 0) && count_between(self, from, target) == 1,
                PieceKind::Pawn => {
                    (dx == 0 && dy == by_color.forward())
                        || (crossed_river(from, by_color) && dy == 0 && dx.abs() == 1)
                }
            };
            if attacked {
                return true;
            }
        }
        false
    }
}

impl Default for GameState {
    fn default() -> Self {
        Self::new()
    }
}

fn in_palace(square: Square, color: Color) -> bool {
    (3..=5).contains(&square.x)
        && match color {
            Color::Black => (0..=2).contains(&square.y),
            Color::Red => (7..=9).contains(&square.y),
        }
}

fn elephant_side(square: Square, color: Color) -> bool {
    match color {
        Color::Black => square.y <= 4,
        Color::Red => square.y >= 5,
    }
}

fn crossed_river(square: Square, color: Color) -> bool {
    match color {
        Color::Black => square.y >= 5,
        Color::Red => square.y <= 4,
    }
}

fn count_between(state: &GameState, from: Square, to: Square) -> usize {
    let dx = (to.x as i32 - from.x as i32).signum();
    let dy = (to.y as i32 - from.y as i32).signum();
    let mut x = from.x as i32 + dx;
    let mut y = from.y as i32 + dy;
    let mut count = 0;
    while x != to.x as i32 || y != to.y as i32 {
        if state
            .piece_at(Square::new(x as usize, y as usize))
            .is_some()
        {
            count += 1;
        }
        x += dx;
        y += dy;
    }
    count
}

fn clear_between(state: &GameState, from: Square, to: Square) -> bool {
    count_between(state, from, to) == 0
}

fn horse_attacks(state: &GameState, from: Square, target: Square) -> bool {
    let dx = target.x as i32 - from.x as i32;
    let dy = target.y as i32 - from.y as i32;
    let (leg_x, leg_y) = match (dx.abs(), dy.abs()) {
        (2, 1) => (from.x as i32 + dx.signum(), from.y as i32),
        (1, 2) => (from.x as i32, from.y as i32 + dy.signum()),
        _ => return false,
    };
    state
        .piece_at(Square::new(leg_x as usize, leg_y as usize))
        .is_none()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initial_position_has_all_pieces() {
        let game = GameState::new();
        assert_eq!(
            game.board.iter().filter(|piece| piece.is_some()).count(),
            32
        );
        assert_eq!(game.side_to_move(), Color::Red);
        assert_eq!(game.legal_moves().len(), 44);
    }

    #[test]
    fn pawn_crosses_river_and_moves_sideways() {
        let mut game = GameState::new();
        let mv = Move {
            from: Square::new(0, 6),
            to: Square::new(0, 5),
        };
        game.apply_move(mv).unwrap();
        let mv = Move {
            from: Square::new(0, 3),
            to: Square::new(0, 4),
        };
        game.apply_move(mv).unwrap();
        for _ in 0..3 {
            game.apply_move(Move {
                from: Square::new(0, 5),
                to: Square::new(0, 4),
            })
            .ok();
        }
        assert!(
            game.legal_moves_for(Color::Red)
                .iter()
                .any(|mv| mv.from == Square::new(0, 4) && mv.to == Square::new(1, 4))
        );
    }

    #[test]
    fn undo_round_restores_initial_position() {
        let mut game = GameState::new();
        game.apply_move(Move {
            from: Square::new(0, 6),
            to: Square::new(0, 5),
        })
        .unwrap();
        game.apply_move(Move {
            from: Square::new(0, 3),
            to: Square::new(0, 4),
        })
        .unwrap();
        assert!(game.undo_round());
        assert_eq!(game.board, GameState::new().board);
        assert_eq!(game.side_to_move(), Color::Red);
        assert!(game.history().is_empty());
    }

    #[test]
    fn horse_leg_is_blocked_by_adjacent_piece() {
        let game = GameState::new();
        let moves = game.legal_moves();
        assert!(!moves.contains(&Move {
            from: Square::new(1, 9),
            to: Square::new(3, 8),
        }));
        assert!(moves.contains(&Move {
            from: Square::new(1, 9),
            to: Square::new(0, 7),
        }));
    }

    #[test]
    fn cannon_needs_a_screen_to_capture() {
        let mut game = GameState::new();
        let capture = Move {
            from: Square::new(1, 7),
            to: Square::new(1, 2),
        };
        assert!(!game.legal_moves().contains(&capture));
        game.board[Square::new(1, 4).index()] = Some(Piece {
            color: Color::Red,
            kind: PieceKind::Pawn,
        });
        assert!(game.legal_moves().contains(&capture));
    }

    #[test]
    fn kings_cannot_face_each_other() {
        let mut game = GameState {
            board: [None; BOARD_SIZE],
            side_to_move: Color::Red,
            status: GameStatus::Playing,
            history: Vec::new(),
            keys: Vec::new(),
            repetitions: HashMap::new(),
        };
        game.board[Square::new(4, 0).index()] = Some(Piece {
            color: Color::Black,
            kind: PieceKind::King,
        });
        game.board[Square::new(4, 9).index()] = Some(Piece {
            color: Color::Red,
            kind: PieceKind::King,
        });
        let moves = game.legal_moves();
        assert!(!moves.contains(&Move {
            from: Square::new(4, 9),
            to: Square::new(4, 8),
        }));
    }
}
