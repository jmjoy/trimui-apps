use crate::game::{Board, DIRECTIONS, Position, SIZE, Stone};
use std::{
    cmp::Reverse,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
        mpsc,
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Difficulty {
    Easy,
    #[default]
    Normal,
    Hard,
}
impl Difficulty {
    pub const ALL: [Self; 3] = [Self::Easy, Self::Normal, Self::Hard];
    pub fn name(self) -> &'static str {
        match self {
            Self::Easy => "简单",
            Self::Normal => "普通",
            Self::Hard => "困难",
        }
    }
    fn limits(self) -> (u8, Duration, u64) {
        match self {
            Self::Easy => (1, Duration::from_millis(100), 1),
            Self::Normal => (2, Duration::from_millis(250), 20_000),
            Self::Hard => (4, Duration::from_secs(1), 100_000),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JobKind {
    Move,
    Hint,
}
#[derive(Clone, Debug)]
pub struct Request {
    pub id: u64,
    pub revision: u64,
    pub kind: JobKind,
    pub board: Board,
    pub stone: Stone,
    pub difficulty: Difficulty,
}
#[derive(Clone, Debug)]
pub struct Reply {
    pub id: u64,
    pub revision: u64,
    pub kind: JobKind,
    pub position: Option<Position>,
}

pub struct Worker {
    sender: Option<mpsc::Sender<Request>>,
    active: Arc<AtomicU64>,
    handle: Option<JoinHandle<()>>,
}
impl Worker {
    pub fn new(notify: impl Fn(Reply) + Send + 'static) -> Self {
        let (sender, receiver) = mpsc::channel::<Request>();
        let active = Arc::new(AtomicU64::new(0));
        let token = Arc::clone(&active);
        let handle = thread::Builder::new()
            .name("gomoku-ai".into())
            .spawn(move || {
                while let Ok(request) = receiver.recv() {
                    if token.load(Ordering::Relaxed) != request.id {
                        continue;
                    }
                    let position = search(
                        &request.board,
                        request.stone,
                        request.difficulty,
                        &token,
                        request.id,
                    );
                    if token.load(Ordering::Relaxed) == request.id {
                        notify(Reply {
                            id: request.id,
                            revision: request.revision,
                            kind: request.kind,
                            position,
                        });
                    }
                }
            })
            .expect("cannot create AI worker");
        Self {
            sender: Some(sender),
            active,
            handle: Some(handle),
        }
    }
    pub fn submit(&self, request: Request) {
        self.active.store(request.id, Ordering::Relaxed);
        if let Some(sender) = &self.sender {
            let _ = sender.send(request);
        }
    }
    pub fn cancel(&self) {
        self.active.store(0, Ordering::Relaxed);
    }
}
impl Drop for Worker {
    fn drop(&mut self) {
        self.cancel();
        self.sender.take();
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

const MATE: i64 = 1_000_000_000;

// Count open runs plus five-cell windows, so split fours and threes also matter.
fn pattern_score(board: &Board, stone: Stone) -> i64 {
    let mut total = 0;
    for pos in Position::all().filter(|&p| board.get(p) == Some(stone)) {
        for (dx, dy) in DIRECTIONS {
            if pos
                .offset(-dx, -dy)
                .is_some_and(|p| board.get(p) == Some(stone))
            {
                continue;
            }
            let mut count = 1;
            let mut end = pos;
            while let Some(next) = end.offset(dx, dy) {
                if board.get(next) != Some(stone) {
                    break;
                }
                count += 1;
                end = next;
            }
            let open = usize::from(pos.offset(-dx, -dy).is_some_and(|p| board.vacant(p)))
                + usize::from(end.offset(dx, dy).is_some_and(|p| board.vacant(p)));
            total += match (count, open) {
                (5.., _) => 10_000_000,
                (4, 2) => 500_000,
                (4, 1) => 50_000,
                (3, 2) => 12_000,
                (3, 1) => 600,
                (2, 2) => 400,
                (2, 1) => 40,
                (1, 2) => 4,
                _ => 0,
            };
        }
    }
    for start in Position::all() {
        for (dx, dy) in DIRECTIONS {
            if start.offset(4 * dx, 4 * dy).is_none() {
                continue;
            }
            let mut count = 0;
            let mut blocked = false;
            for i in 0..5 {
                match board.get(start.offset(i * dx, i * dy).unwrap()) {
                    Some(s) if s == stone => count += 1,
                    Some(_) => {
                        blocked = true;
                        break;
                    }
                    None => {}
                }
            }
            if !blocked {
                total += [0, 1, 12, 150, 8_000, 100_000][count];
            }
        }
    }
    total
}

fn local_score(board: &Board, pos: Position, stone: Stone) -> i64 {
    let mut total = 0;
    for (dx, dy) in DIRECTIONS {
        let mut count = 1;
        let mut open = 0;
        for sign in [-1, 1] {
            let mut current = pos;
            while let Some(next) = current.offset(sign * dx, sign * dy) {
                if board.get(next) == Some(stone) {
                    count += 1;
                    current = next;
                } else {
                    open += usize::from(board.vacant(next));
                    break;
                }
            }
        }
        total += match (count, open) {
            (5.., _) => MATE,
            (4, 2) => 500_000,
            (4, 1) => 70_000,
            (3, 2) => 15_000,
            (3, 1) => 700,
            (2, 2) => 500,
            (2, 1) => 50,
            _ => 1,
        };
        // Five-cell windows through this move capture non-contiguous threats.
        for shift in 0..5 {
            let Some(start) = pos.offset(-shift * dx, -shift * dy) else {
                continue;
            };
            if start.offset(4 * dx, 4 * dy).is_none() {
                continue;
            }
            let mut pieces = 0;
            let mut blocked = false;
            for i in 0..5 {
                let p = start.offset(i * dx, i * dy).unwrap();
                if p == pos || board.get(p) == Some(stone) {
                    pieces += 1;
                } else if board.get(p).is_some() {
                    blocked = true;
                    break;
                }
            }
            if !blocked {
                total += [0, 1, 10, 120, 9_000, 100_000][pieces];
            }
        }
    }
    total
}

fn candidates(board: &Board, stone: Stone, limit: usize) -> Vec<Position> {
    if board.empty() {
        return vec![Position::new(SIZE / 2, SIZE / 2)];
    }
    let mut near = [false; SIZE * SIZE];
    for pos in Position::all().filter(|&p| board.get(p).is_some()) {
        for dx in -2..=2 {
            for dy in -2..=2 {
                if let Some(p) = pos.offset(dx, dy)
                    && board.vacant(p)
                {
                    near[p.index()] = true;
                }
            }
        }
    }
    let mut moves: Vec<_> = Position::all()
        .filter(|p| near[p.index()])
        .map(|p| {
            let attack = local_score(board, p, stone);
            let defense = local_score(board, p, stone.other());
            let score = attack + defense * 11 / 10;
            let distance = p.x.abs_diff(7) + p.y.abs_diff(7);
            (p, score, distance)
        })
        .collect();
    moves.sort_by_key(|&(p, score, distance)| (Reverse(score), distance, p));
    // Never prune an immediate winning move or a necessary block.
    let wins: Vec<_> = moves
        .iter()
        .filter(|&&(p, _, _)| local_score(board, p, stone) >= MATE)
        .map(|m| m.0)
        .collect();
    if !wins.is_empty() {
        return wins;
    }
    let blocks: Vec<_> = moves
        .iter()
        .filter(|&&(p, _, _)| local_score(board, p, stone.other()) >= MATE)
        .map(|m| m.0)
        .collect();
    if !blocks.is_empty() {
        return blocks;
    }
    moves.into_iter().take(limit).map(|m| m.0).collect()
}

struct Search<'a> {
    deadline: Instant,
    active: &'a AtomicU64,
    id: u64,
    nodes: u64,
    max_nodes: u64,
}
impl Search<'_> {
    fn stopped(&self) -> bool {
        self.nodes >= self.max_nodes
            || Instant::now() >= self.deadline
            || self.active.load(Ordering::Relaxed) != self.id
    }
    fn negamax(
        &mut self,
        board: &mut Board,
        stone: Stone,
        depth: u8,
        mut alpha: i64,
        beta: i64,
    ) -> Result<i64, ()> {
        if self.stopped() {
            return Err(());
        }
        self.nodes += 1;
        if depth == 0 {
            return Ok(pattern_score(board, stone) - pattern_score(board, stone.other()));
        }
        let moves = candidates(board, stone, 12);
        if moves.is_empty() {
            return Ok(0);
        }
        let mut best = -MATE * 2;
        for pos in moves {
            board.set(pos, Some(stone));
            let score = if board.wins_at(pos, stone) {
                Ok(MATE + i64::from(depth))
            } else {
                self.negamax(board, stone.other(), depth - 1, -beta, -alpha)
                    .map(|s| -s)
            };
            board.set(pos, None);
            let score = score?;
            best = best.max(score);
            alpha = alpha.max(score);
            if alpha >= beta {
                break;
            }
        }
        Ok(best)
    }
}

pub fn search(
    board: &Board,
    stone: Stone,
    difficulty: Difficulty,
    active: &AtomicU64,
    id: u64,
) -> Option<Position> {
    let (depth, budget, max_nodes) = difficulty.limits();
    search_with_limits(board, stone, depth, budget, max_nodes, active, id)
}

fn search_with_limits(
    board: &Board,
    stone: Stone,
    max_depth: u8,
    budget: Duration,
    max_nodes: u64,
    active: &AtomicU64,
    id: u64,
) -> Option<Position> {
    let mut context = Search {
        deadline: Instant::now() + budget,
        active,
        id,
        nodes: 0,
        max_nodes,
    };
    if context.stopped() {
        return None;
    }
    let mut moves = candidates(board, stone, 18);
    let mut best = *moves.first()?;
    if difficulty_fast_path(board, stone, best) || max_depth == 1 {
        return Some(best);
    }
    let mut work = board.clone();
    for depth in 1..=max_depth {
        let mut scores = Vec::with_capacity(moves.len());
        let mut alpha = -MATE * 2;
        for &pos in &moves {
            if context.stopped() {
                break;
            }
            work.set(pos, Some(stone));
            let result = if work.wins_at(pos, stone) {
                Ok(MATE + i64::from(depth))
            } else {
                context
                    .negamax(&mut work, stone.other(), depth - 1, -MATE * 2, -alpha)
                    .map(|s| -s)
            };
            work.set(pos, None);
            let Ok(score) = result else {
                break;
            };
            alpha = alpha.max(score);
            scores.push((pos, score));
        }
        if scores.len() != moves.len() {
            break;
        }
        scores.sort_by_key(|&(_, score)| Reverse(score));
        best = scores[0].0;
        if scores[0].1 >= MATE {
            break;
        }
        moves = scores.into_iter().map(|s| s.0).collect();
    }
    if active.load(Ordering::Relaxed) != id {
        None
    } else {
        Some(best)
    }
}
fn difficulty_fast_path(board: &Board, stone: Stone, pos: Position) -> bool {
    board.empty() || local_score(board, pos, stone) >= MATE
}

#[cfg(test)]
mod tests {
    use super::*;
    fn choose(board: &Board, stone: Stone, difficulty: Difficulty) -> Position {
        search(board, stone, difficulty, &AtomicU64::new(1), 1).unwrap()
    }
    #[test]
    fn all_levels_win_and_block_including_split_four() {
        for difficulty in Difficulty::ALL {
            for stone in [Stone::Black, Stone::White] {
                for offsets in [[0, 1, 2, 3], [0, 1, 3, 4]] {
                    let mut board = Board::default();
                    for x in offsets {
                        board.set(Position::new(x, 0), Some(stone));
                    }
                    let expected = if offsets[2] == 2 {
                        Position::new(4, 0)
                    } else {
                        Position::new(2, 0)
                    };
                    assert_eq!(choose(&board, stone, difficulty), expected);
                    assert_eq!(choose(&board, stone.other(), difficulty), expected);
                }
            }
        }
    }
    #[test]
    fn own_win_takes_priority_over_blocking() {
        let mut board = Board::default();
        for x in 0..4 {
            board.set(Position::new(x, 0), Some(Stone::Black));
            board.set(Position::new(x, 2), Some(Stone::White));
        }
        assert_eq!(
            choose(&board, Stone::Black, Difficulty::Hard),
            Position::new(4, 0)
        );
    }
    #[test]
    fn canceled_full_and_budget_limited_searches() {
        let mut board = Board::default();
        board.set(Position::new(7, 7), Some(Stone::Black));
        assert!(
            search(
                &board,
                Stone::White,
                Difficulty::Hard,
                &AtomicU64::new(2),
                1
            )
            .is_none()
        );
        let p = search_with_limits(
            &board,
            Stone::White,
            4,
            Duration::from_secs(1),
            1,
            &AtomicU64::new(1),
            1,
        )
        .unwrap();
        assert!(board.vacant(p));
        assert!(
            search_with_limits(
                &board,
                Stone::White,
                4,
                Duration::ZERO,
                100,
                &AtomicU64::new(1),
                1
            )
            .is_none()
        );
        board.cells.fill(1);
        assert!(
            search(
                &board,
                Stone::White,
                Difficulty::Normal,
                &AtomicU64::new(1),
                1
            )
            .is_none()
        );
    }
    #[test]
    fn opens_center_and_preserves_snapshot() {
        let board = Board::default();
        assert_eq!(
            choose(&board, Stone::Black, Difficulty::Hard),
            Position::new(7, 7)
        );
        assert!(board.empty());
    }

    #[test]
    fn complete_games_finish_and_all_searches_preserve_the_board() {
        for first in [Position::new(7, 7), Position::new(0, 0)] {
            let mut game = crate::game::Game::default();
            game.play(first).unwrap();
            while game.outcome == crate::game::Outcome::Playing {
                let before = game.board.clone();
                let pos = choose(&game.board, game.turn(), Difficulty::Easy);
                assert_eq!(game.board, before);
                game.play(pos).unwrap();
                assert!(game.history.len() <= SIZE * SIZE);
            }
            assert!(matches!(
                game.outcome,
                crate::game::Outcome::Won { .. } | crate::game::Outcome::Draw
            ));
        }
    }

    #[test]
    fn worker_delivers_latest_job_after_cancellation() {
        let (sender, receiver) = mpsc::channel();
        let worker = Worker::new(move |reply| {
            sender.send(reply).unwrap();
        });
        let mut board = Board::default();
        board.set(Position::new(7, 7), Some(Stone::Black));
        worker.submit(Request {
            id: 1,
            revision: 1,
            kind: JobKind::Hint,
            board,
            stone: Stone::White,
            difficulty: Difficulty::Hard,
        });
        worker.cancel();
        worker.submit(Request {
            id: 2,
            revision: 2,
            kind: JobKind::Move,
            board: Board::default(),
            stone: Stone::Black,
            difficulty: Difficulty::Hard,
        });
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            let reply = receiver
                .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                .unwrap();
            if reply.id == 2 {
                assert_eq!(reply.position, Some(Position::new(7, 7)));
                break;
            }
        }
    }
}
