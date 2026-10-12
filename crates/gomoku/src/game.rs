use std::fmt;

pub const SIZE: usize = 15;
pub const CELLS: usize = SIZE * SIZE;
pub const DIRECTIONS: [(isize, isize); 4] = [(1, 0), (0, 1), (1, 1), (1, -1)];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Stone {
    Black = 1,
    White = 2,
}

impl Stone {
    pub fn other(self) -> Self {
        match self {
            Self::Black => Self::White,
            Self::White => Self::Black,
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            Self::Black => "黑棋",
            Self::White => "白棋",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Position {
    pub x: usize,
    pub y: usize,
}

impl Position {
    pub const fn new(x: usize, y: usize) -> Self {
        Self { x, y }
    }
    pub fn index(self) -> usize {
        self.y * SIZE + self.x
    }
    pub fn offset(self, dx: isize, dy: isize) -> Option<Self> {
        let x = self.x.checked_add_signed(dx)?;
        let y = self.y.checked_add_signed(dy)?;
        (x < SIZE && y < SIZE).then_some(Self::new(x, y))
    }
    pub fn all() -> impl Iterator<Item = Self> {
        (0..CELLS).map(|i| Self::new(i % SIZE, i / SIZE))
    }
}

impl fmt::Display for Position {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}{}", (b'A' + self.x as u8) as char, SIZE - self.y)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Board {
    pub cells: [u8; CELLS],
}

impl Default for Board {
    fn default() -> Self {
        Self { cells: [0; CELLS] }
    }
}

impl Board {
    pub fn get(&self, pos: Position) -> Option<Stone> {
        if pos.x >= SIZE || pos.y >= SIZE {
            return None;
        }
        match self.cells[pos.index()] {
            1 => Some(Stone::Black),
            2 => Some(Stone::White),
            _ => None,
        }
    }
    pub fn vacant(&self, pos: Position) -> bool {
        pos.x < SIZE && pos.y < SIZE && self.cells[pos.index()] == 0
    }
    pub fn set(&mut self, pos: Position, stone: Option<Stone>) {
        self.cells[pos.index()] = stone.map_or(0, |s| s as u8);
    }
    pub fn full(&self) -> bool {
        self.cells.iter().all(|&s| s != 0)
    }
    pub fn empty(&self) -> bool {
        self.cells.iter().all(|&s| s == 0)
    }

    pub fn winning_line(&self, pos: Position, stone: Stone) -> Option<Vec<Position>> {
        if self.get(pos) != Some(stone) {
            return None;
        }
        for (dx, dy) in DIRECTIONS {
            let mut line = Vec::with_capacity(SIZE);
            let mut current = pos;
            while let Some(next) = current.offset(-dx, -dy) {
                if self.get(next) != Some(stone) {
                    break;
                }
                line.push(next);
                current = next;
            }
            line.reverse();
            line.push(pos);
            current = pos;
            while let Some(next) = current.offset(dx, dy) {
                if self.get(next) != Some(stone) {
                    break;
                }
                line.push(next);
                current = next;
            }
            if line.len() >= 5 {
                return Some(line);
            }
        }
        None
    }

    // Search hot path: no allocation, including for hypothetical moves.
    pub fn wins_at(&self, pos: Position, stone: Stone) -> bool {
        if self.get(pos) != Some(stone) {
            return false;
        }
        DIRECTIONS.iter().any(|&(dx, dy)| {
            let mut count = 1;
            for sign in [-1, 1] {
                let mut current = pos;
                while let Some(next) = current.offset(dx * sign, dy * sign) {
                    if self.get(next) != Some(stone) {
                        break;
                    }
                    count += 1;
                    current = next;
                }
            }
            count >= 5
        })
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Outcome {
    #[default]
    Playing,
    Won {
        stone: Stone,
        line: Vec<Position>,
    },
    Draw,
}

#[derive(Clone, Copy, Debug)]
pub struct Move {
    pub pos: Position,
    pub stone: Stone,
}

#[derive(Clone, Debug, Default)]
pub struct Game {
    pub board: Board,
    pub history: Vec<Move>,
    pub outcome: Outcome,
}

#[derive(Debug, PartialEq, Eq)]
pub enum MoveError {
    OutOfBounds,
    Occupied,
    Finished,
}

impl Game {
    pub fn turn(&self) -> Stone {
        if self.history.len().is_multiple_of(2) {
            Stone::Black
        } else {
            Stone::White
        }
    }
    pub fn play(&mut self, pos: Position) -> Result<(), MoveError> {
        if self.outcome != Outcome::Playing {
            return Err(MoveError::Finished);
        }
        if pos.x >= SIZE || pos.y >= SIZE {
            return Err(MoveError::OutOfBounds);
        }
        if !self.board.vacant(pos) {
            return Err(MoveError::Occupied);
        }
        let stone = self.turn();
        self.board.set(pos, Some(stone));
        self.history.push(Move { pos, stone });
        self.outcome = if let Some(line) = self.board.winning_line(pos, stone) {
            Outcome::Won { stone, line }
        } else if self.board.full() {
            Outcome::Draw
        } else {
            Outcome::Playing
        };
        Ok(())
    }
    pub fn can_undo(&self, human: Stone) -> bool {
        self.history.iter().any(|m| m.stone == human)
    }
    pub fn undo(&mut self, human: Stone) -> bool {
        let Some(index) = self.history.iter().rposition(|m| m.stone == human) else {
            return false;
        };
        self.history.truncate(index);
        self.board = Board::default();
        for m in &self.history {
            self.board.set(m.pos, Some(m.stone));
        }
        self.outcome = Outcome::Playing;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn four_directions_and_long_lines_at_edges() {
        for (start, dx, dy) in [
            (Position::new(0, 0), 1, 0),
            (Position::new(14, 0), 0, 1),
            (Position::new(0, 0), 1, 1),
            (Position::new(0, 14), 1, -1),
        ] {
            for length in [5, 6, 15] {
                let mut board = Board::default();
                for i in 0..length {
                    board.set(start.offset(dx * i, dy * i).unwrap(), Some(Stone::Black));
                }
                let middle = start.offset(dx * 2, dy * 2).unwrap();
                assert_eq!(
                    board.winning_line(middle, Stone::Black).unwrap().len(),
                    length as usize
                );
                assert!(board.wins_at(middle, Stone::Black));
                assert!(!board.wins_at(middle, Stone::White));
            }
        }
    }
    #[test]
    fn invalid_moves_preserve_state_and_game_stops_after_win() {
        let mut game = Game::default();
        assert_eq!(game.play(Position::new(15, 0)), Err(MoveError::OutOfBounds));
        game.play(Position::new(0, 0)).unwrap();
        assert_eq!(game.play(Position::new(0, 0)), Err(MoveError::Occupied));
        assert_eq!(game.turn(), Stone::White);
        for x in 1..5 {
            game.play(Position::new(x, 2)).unwrap();
            game.play(Position::new(x, 0)).unwrap();
        }
        assert!(matches!(
            game.outcome,
            Outcome::Won {
                stone: Stone::Black,
                ..
            }
        ));
        assert_eq!(game.play(Position::new(7, 7)), Err(MoveError::Finished));
        assert!(game.undo(Stone::Black));
        assert_eq!(game.outcome, Outcome::Playing);
        assert_eq!(game.turn(), Stone::Black);
    }
    #[test]
    fn undo_before_or_after_reply_for_both_colors() {
        for human in [Stone::Black, Stone::White] {
            for replied in [false, true] {
                let mut game = Game::default();
                if human == Stone::White {
                    game.play(Position::new(7, 7)).unwrap();
                    assert!(!game.undo(human));
                }
                let before = game.board.clone();
                game.play(Position::new(6, 7)).unwrap();
                if replied {
                    game.play(Position::new(8, 7)).unwrap();
                }
                assert!(game.undo(human));
                assert_eq!(game.board, before);
                assert_eq!(game.turn(), human);
            }
        }
    }
    #[test]
    fn full_board_without_five_is_draw() {
        let mut game = Game::default();
        // Alternating pairs shift each row: no horizontal, vertical or diagonal five.
        for pos in Position::all() {
            game.board.set(
                pos,
                Some(if (pos.x / 2 + pos.y) % 2 == 0 {
                    Stone::Black
                } else {
                    Stone::White
                }),
            );
        }
        let last = Position::new(14, 14);
        let last_stone = game.board.get(last).unwrap();
        game.board.set(last, None);
        game.history = vec![
            Move {
                pos: Position::new(0, 0),
                stone: Stone::Black
            };
            if last_stone == Stone::Black { 224 } else { 223 }
        ];
        game.play(last).unwrap();
        assert_eq!(game.outcome, Outcome::Draw);
    }
}
