//! Iterative alpha-beta search with a bounded thinking time and cancellation.
use std::{
    collections::HashMap,
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};

use crate::game::{BOARD_SIZE, Color, Difficulty, GameState, Move, PieceKind, Square};

const MATE_SCORE: i32 = 1_000_000;
const TABLE_LIMIT: usize = 40_000;

#[derive(Clone, Copy)]
enum Bound {
    Exact,
    Lower,
    Upper,
}

#[derive(Clone, Copy)]
struct TableEntry {
    depth: u8,
    score: i32,
    bound: Bound,
}

struct Search<'a> {
    root: Color,
    deadline: Instant,
    cancelled: &'a AtomicBool,
    table: HashMap<[u8; BOARD_SIZE + 1], TableEntry>,
}

pub fn search_best_move(
    state: &GameState,
    difficulty: Difficulty,
    cancelled: &AtomicBool,
) -> Option<Move> {
    let budget = match difficulty {
        Difficulty::Easy => Duration::from_millis(350),
        Difficulty::Normal => Duration::from_millis(1200),
        Difficulty::Hard => Duration::from_millis(3000),
    };
    search_with_budget(state, difficulty.depth(), budget, cancelled)
}

fn search_with_budget(
    state: &GameState,
    max_depth: u8,
    budget: Duration,
    cancelled: &AtomicBool,
) -> Option<Move> {
    if cancelled.load(Ordering::Relaxed) {
        return None;
    }
    let mut moves = state.legal_moves();
    order_moves(state, &mut moves);
    let mut best = moves.first().copied()?;
    let mut search = Search {
        root: state.side_to_move(),
        deadline: Instant::now() + budget,
        cancelled,
        table: HashMap::new(),
    };
    // Commit only complete iterations; a timeout still leaves a legal move.
    'deepening: for depth in 1..=max_depth {
        let mut candidate = best;
        let mut best_score = -MATE_SCORE * 2;
        let mut alpha = -MATE_SCORE * 2;
        moves.sort_by_key(|mv| *mv != best);
        for &mv in &moves {
            let mut next = state.search_position();
            next.apply_for_search(mv);
            let Some(score) = search.minimax(&next, depth - 1, alpha, MATE_SCORE * 2) else {
                break 'deepening;
            };
            if score > best_score {
                best_score = score;
                candidate = mv;
            }
            alpha = alpha.max(score);
        }
        best = candidate;
    }
    (!cancelled.load(Ordering::Relaxed)).then_some(best)
}

impl Search<'_> {
    fn minimax(
        &mut self,
        state: &GameState,
        depth: u8,
        mut alpha: i32,
        mut beta: i32,
    ) -> Option<i32> {
        if self.cancelled.load(Ordering::Relaxed) || Instant::now() >= self.deadline {
            return None;
        }
        // Even a leaf must detect mate/stalemate rather than just material.
        let mut moves = state.legal_moves();
        if moves.is_empty() {
            return Some(if state.side_to_move() == self.root {
                -MATE_SCORE - depth as i32
            } else {
                MATE_SCORE + depth as i32
            });
        }
        if depth == 0 {
            return Some(evaluate(state, self.root));
        }
        let key = position_key(state);
        let original_alpha = alpha;
        let original_beta = beta;
        if let Some(entry) = self.table.get(&key)
            && entry.depth == depth
        {
            match entry.bound {
                Bound::Exact => return Some(entry.score),
                Bound::Lower => alpha = alpha.max(entry.score),
                Bound::Upper => beta = beta.min(entry.score),
            }
            if alpha >= beta {
                return Some(entry.score);
            }
        }
        order_moves(state, &mut moves);
        let maximizing = state.side_to_move() == self.root;
        let mut value = if maximizing {
            -MATE_SCORE * 2
        } else {
            MATE_SCORE * 2
        };
        for mv in moves {
            let mut next = state.search_position();
            next.apply_for_search(mv);
            let score = self.minimax(&next, depth - 1, alpha, beta)?;
            if maximizing {
                value = value.max(score);
                alpha = alpha.max(value);
            } else {
                value = value.min(score);
                beta = beta.min(value);
            }
            if beta <= alpha {
                break;
            }
        }
        if self.table.len() < TABLE_LIMIT {
            self.table.insert(
                key,
                TableEntry {
                    depth,
                    score: value,
                    bound: if value <= original_alpha {
                        Bound::Upper
                    } else if value >= original_beta {
                        Bound::Lower
                    } else {
                        Bound::Exact
                    },
                },
            );
        }
        Some(value)
    }
}

fn evaluate(state: &GameState, root: Color) -> i32 {
    let mut score = 0;
    for square_index in 0..BOARD_SIZE {
        let square = Square::from_index(square_index);
        let Some(piece) = state.piece_at(square) else {
            continue;
        };
        let value = piece_value(piece.kind) + positional_bonus(piece.kind, piece.color, square);
        score += if piece.color == root { value } else { -value };
    }
    if state.is_in_check(root) {
        score -= 90;
    }
    if state.is_in_check(root.other()) {
        score += 90;
    }
    score
}

fn piece_value(kind: PieceKind) -> i32 {
    match kind {
        PieceKind::King => 10_000,
        PieceKind::Advisor | PieceKind::Elephant => 120,
        PieceKind::Horse => 400,
        PieceKind::Rook => 900,
        PieceKind::Cannon => 450,
        PieceKind::Pawn => 100,
    }
}

fn positional_bonus(kind: PieceKind, color: Color, square: Square) -> i32 {
    let forward = match color {
        Color::Red => 9 - square.y,
        Color::Black => square.y,
    } as i32;
    match kind {
        PieceKind::Pawn => forward * 7 + if square.x == 4 { 8 } else { 0 },
        PieceKind::Horse | PieceKind::Cannon => {
            16 - ((square.x as i32 - 4).abs() + (square.y as i32 - 4).abs()) * 2
        }
        PieceKind::Rook => 10 - (square.x as i32 - 4).abs() * 2,
        _ => 0,
    }
}

fn order_moves(state: &GameState, moves: &mut [Move]) {
    moves.sort_by_key(|mv| {
        -state
            .piece_at(mv.to)
            .map_or(0, |piece| piece_value(piece.kind))
    });
}

fn position_key(state: &GameState) -> [u8; BOARD_SIZE + 1] {
    let mut key = [0u8; BOARD_SIZE + 1];
    for (index, slot) in key.iter_mut().enumerate().take(BOARD_SIZE) {
        *slot = state
            .piece_at(Square::from_index(index))
            .map_or(0, |piece| {
                let code = match piece.kind {
                    PieceKind::King => 1,
                    PieceKind::Advisor => 2,
                    PieceKind::Elephant => 3,
                    PieceKind::Horse => 4,
                    PieceKind::Rook => 5,
                    PieceKind::Cannon => 6,
                    PieceKind::Pawn => 7,
                };
                code + if piece.color == Color::Red { 8 } else { 0 }
            });
    }
    key[BOARD_SIZE] = if state.side_to_move() == Color::Red {
        1
    } else {
        2
    };
    key
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_difficulty_returns_a_legal_move() {
        let state = GameState::new();
        for difficulty in [Difficulty::Easy, Difficulty::Normal, Difficulty::Hard] {
            let mv = search_best_move(&state, difficulty, &AtomicBool::new(false)).unwrap();
            assert!(state.legal_moves().contains(&mv));
        }
    }

    #[test]
    fn timeout_has_a_legal_fallback_and_cancellation_has_no_move() {
        let state = GameState::new();
        let cancelled = AtomicBool::new(false);
        let mv = search_with_budget(&state, 10, Duration::ZERO, &cancelled).unwrap();
        assert!(state.legal_moves().contains(&mv));
        cancelled.store(true, Ordering::Relaxed);
        assert!(search_best_move(&state, Difficulty::Hard, &cancelled).is_none());
    }
}
