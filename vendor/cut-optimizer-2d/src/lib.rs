//! cut-optimizer-2d is an optimizer library that attempts layout rectangular cut pieces from stock pieces in a
//! way that gives the least waste. It uses genetic algorithms and multiple heuristics to solve the problem.

#![deny(missing_docs)]

mod genetic;
mod guillotine;
mod maxrects;

#[cfg(test)]
mod tests;

use genetic::population::Population;
use genetic::unit::Unit;
use guillotine::GuillotineBin;
use maxrects::MaxRectsBin;

use fnv::FnvHashSet;
use rand::prelude::*;
use rand::seq::SliceRandom;

use std::borrow::Borrow;
use std::cmp;
use std::hash::{Hash, Hasher};

#[cfg(feature = "serialize")]
use serde::{Deserialize, Serialize};

/// Indicates the linear direction of a pattern, grain, etc.
#[cfg_attr(feature = "serialize", derive(Deserialize, Serialize))]
#[cfg_attr(feature = "serialize", serde(rename_all = "camelCase"))]
#[derive(Copy, Clone, Debug, Hash, PartialEq, Eq)]
pub enum PatternDirection {
    /// No pattern
    None,

    /// Linear pattern that runs parallel to the width
    ParallelToWidth,

    /// Linear pattern that runs parallel to the length
    ParallelToLength,
}

impl PatternDirection {
    /// Returns the opposite orientation of this `PatternDirection`.
    fn rotated(self) -> PatternDirection {
        match self {
            PatternDirection::None => PatternDirection::None,
            PatternDirection::ParallelToWidth => PatternDirection::ParallelToLength,
            PatternDirection::ParallelToLength => PatternDirection::ParallelToWidth,
        }
    }
}

impl Default for PatternDirection {
    fn default() -> Self {
        PatternDirection::None
    }
}

/// A rectangular piece that needs to be cut from a stock piece.
#[cfg_attr(feature = "serialize", derive(Deserialize, Serialize))]
#[cfg_attr(feature = "serialize", serde(rename_all = "camelCase"))]
#[derive(Clone, Debug)]
pub struct CutPiece {
    /// Quantity of this cut piece.
    pub quantity: usize,

    /// ID to be used by the caller to match up result cut pieces
    /// with the original cut piece. This ID has no meaning to the
    /// optimizer so it can be set to `None` if not needed.
    pub external_id: Option<usize>,

    /// Width of this rectangular cut piece.
    pub width: usize,

    /// Length of this rectangular cut piece.
    pub length: usize,

    /// Pattern direction of this cut piece.
    pub pattern_direction: PatternDirection,

    /// Whether or not the optimizer is allowed to rotate this piece to make it fit.
    pub can_rotate: bool,
}

#[derive(Clone, Debug)]
pub(crate) struct CutPieceWithId {
    pub(crate) id: usize,
    pub(crate) external_id: Option<usize>,
    pub(crate) width: usize,
    pub(crate) length: usize,
    pub(crate) pattern_direction: PatternDirection,
    pub(crate) can_rotate: bool,
}

impl Hash for CutPieceWithId {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.id.hash(state);
    }
}
impl PartialEq for CutPieceWithId {
    fn eq(&self, other: &CutPieceWithId) -> bool {
        self.id == other.id
    }
}
impl Eq for CutPieceWithId {}

#[derive(Clone, Debug)]
pub(crate) struct UsedCutPiece {
    pub(crate) id: usize,
    pub(crate) external_id: Option<usize>,
    pub(crate) rect: Rect,
    pub(crate) pattern_direction: PatternDirection,
    pub(crate) is_rotated: bool,
    pub(crate) can_rotate: bool,
}

impl PartialEq for UsedCutPiece {
    fn eq(&self, other: &UsedCutPiece) -> bool {
        self.id == other.id
    }
}
impl Eq for UsedCutPiece {}

impl From<&UsedCutPiece> for CutPieceWithId {
    fn from(used_cut_piece: &UsedCutPiece) -> Self {
        let (width, length, pattern_direction) = if used_cut_piece.is_rotated {
            (
                used_cut_piece.rect.length,
                used_cut_piece.rect.width,
                used_cut_piece.pattern_direction.rotated(),
            )
        } else {
            (
                used_cut_piece.rect.width,
                used_cut_piece.rect.length,
                used_cut_piece.pattern_direction,
            )
        };

        Self {
            id: used_cut_piece.id,
            external_id: used_cut_piece.external_id,
            width,
            length,
            can_rotate: used_cut_piece.can_rotate,
            pattern_direction,
        }
    }
}

impl From<&UsedCutPiece> for ResultCutPiece {
    fn from(used_cut_piece: &UsedCutPiece) -> Self {
        Self {
            external_id: used_cut_piece.external_id,
            x: used_cut_piece.rect.x,
            y: used_cut_piece.rect.y,
            width: used_cut_piece.rect.width,
            length: used_cut_piece.rect.length,
            pattern_direction: used_cut_piece.pattern_direction,
            is_rotated: used_cut_piece.is_rotated,
        }
    }
}

/// A cut piece that has been placed in a solution by the optimizer.
#[cfg_attr(feature = "serialize", derive(Deserialize, Serialize))]
#[cfg_attr(feature = "serialize", serde(rename_all = "camelCase"))]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResultCutPiece {
    /// ID that matches the one on the cut piece that was passed to the optimizer.
    pub external_id: Option<usize>,

    /// X location of the left side of this cut piece within the stock piece.
    pub x: usize,

    /// Y location of the (bottom or top) side of this cut piece within the stock piece.
    pub y: usize,

    /// Width of this cut piece.
    pub width: usize,

    /// Length of this cut piece.
    pub length: usize,

    /// Pattern direction of this cut piece.
    pub pattern_direction: PatternDirection,

    /// Whether or not this cut piece was rotated 90 degrees by the optimizer from it's original
    /// oriorientation.
    pub is_rotated: bool,
}

/// A rectangular stock piece that is available to cut one or more
/// cut pieces from.
#[cfg_attr(feature = "serialize", derive(Deserialize, Serialize))]
#[cfg_attr(feature = "serialize", serde(rename_all = "camelCase"))]
#[derive(Hash, Copy, Clone, Debug, Eq, PartialEq)]
pub struct StockPiece {
    /// Width of rectangular stock piece.
    pub width: usize,

    /// Length of rectangular stock piece.
    pub length: usize,

    /// Pattern direction of stock piece.
    pub pattern_direction: PatternDirection,

    /// Price to use to optimize for price when not all stock pieces are the same price per unit
    /// area. If optimizing for less waste instead, price can be set to 0 for all stock pieces.
    pub price: usize,

    /// Quantity of this stock piece available for optimization. `None` means infinite quantity.
    pub quantity: Option<usize>,
}

impl StockPiece {
    /// Checks whether of not the cut piece fits within the bounds of this stock piece.
    fn fits_cut_piece(&self, cut_piece: &CutPieceWithId) -> bool {
        let rect = Rect {
            x: 0,
            y: 0,
            width: self.width,
            length: self.length,
        };

        rect.fit_cut_piece(self.pattern_direction, cut_piece, false) != Fit::None
    }

    /// Decrement the quantity of this stock piece. If quantity is `None` it will remain `None`.
    fn dec_quantity(&mut self) {
        if let Some(ref mut quantity) = self.quantity {
            *quantity -= 1;
        }
    }

    /// Increment the quantity of this stock piece. If quantity is `None` it will remain `None`.
    fn inc_quantity(&mut self) {
        if let Some(ref mut quantity) = self.quantity {
            *quantity += 1;
        }
    }
}

/// Stock piece that was used by the optimizer to get one or more cut pieces.
#[cfg_attr(feature = "serialize", derive(Deserialize, Serialize))]
#[cfg_attr(feature = "serialize", serde(rename_all = "camelCase"))]
#[derive(Clone, Debug)]
pub struct ResultStockPiece {
    /// Width of this stock piece.
    pub width: usize,

    /// Length of this stock piece.
    pub length: usize,

    /// Pattern direction of this stock piece.
    pub pattern_direction: PatternDirection,

    /// Cut pieces to cut from this stock piece.
    pub cut_pieces: Vec<ResultCutPiece>,

    /// Waste pieces that remain after cutting the cut pieces.
    pub waste_pieces: Vec<Rect>,

    /// Price of stock piece.
    pub price: usize,
}

/// A rectangle
#[cfg_attr(feature = "serialize", derive(Deserialize, Serialize))]
#[cfg_attr(feature = "serialize", serde(rename_all = "camelCase"))]
#[derive(Copy, Clone, Debug, Default)]
pub struct Rect {
    /// X location of this rectangle.
    x: usize,

    /// Y location of this rectangle.
    y: usize,

    /// Width of this rectangle.
    width: usize,

    /// Length of this rectangle.
    length: usize,
}

impl Rect {
    fn fit_cut_piece(
        &self,
        pattern_direction: PatternDirection,
        cut_piece: &CutPieceWithId,
        prefer_rotated: bool,
    ) -> Fit {
        let upright_fit = if cut_piece.pattern_direction == pattern_direction {
            if cut_piece.width == self.width && cut_piece.length == self.length {
                Some(Fit::UprightExact)
            } else if cut_piece.width <= self.width && cut_piece.length <= self.length {
                Some(Fit::Upright)
            } else {
                None
            }
        } else {
            None
        };

        let rotated_fit =
            if cut_piece.can_rotate && cut_piece.pattern_direction.rotated() == pattern_direction {
                if cut_piece.length == self.width && cut_piece.width == self.length {
                    Some(Fit::RotatedExact)
                } else if cut_piece.length <= self.width && cut_piece.width <= self.length {
                    Some(Fit::Rotated)
                } else {
                    None
                }
            } else {
                None
            };

        match (upright_fit, rotated_fit) {
            (Some(upright_fit), Some(rotated_fit)) => {
                if prefer_rotated {
                    rotated_fit
                } else {
                    upright_fit
                }
            }
            (Some(upright_fit), None) => upright_fit,
            (None, Some(rotated_fit)) => rotated_fit,
            (None, None) => Fit::None,
        }
    }

    fn contains(&self, rect: &Rect) -> bool {
        rect.x >= self.x
            && rect.x + rect.width <= self.x + self.width
            && rect.y >= self.y
            && rect.y + rect.length <= self.y + self.length
    }
}

impl From<&ResultCutPiece> for Rect {
    fn from(cut_piece: &ResultCutPiece) -> Self {
        Self {
            x: cut_piece.x,
            y: cut_piece.y,
            width: cut_piece.width,
            length: cut_piece.length,
        }
    }
}

#[derive(Copy, Clone, PartialEq, Eq)]
enum Fit {
    None,
    UprightExact,
    RotatedExact,
    Upright,
    Rotated,
}

impl Fit {
    fn is_none(self) -> bool {
        self == Fit::None
    }

    fn is_upright(self) -> bool {
        self == Fit::Upright || self == Fit::UprightExact
    }

    fn is_rotated(self) -> bool {
        self == Fit::Rotated || self == Fit::RotatedExact
    }
}

// ---------------------------------------------------------------------------
// V15: zones-aware GA fitness helpers.
//
// Connected-component counting on free rectangles gives a cheap approximation
// of waste zones without grid flood-fill.  Two free rects are "connected"
// when they touch or overlap (with a small tolerance for the kerf gap).
//
// Env-configurable parameters (read once, cached):
//   FREECUT_GA_ZONE_PENALTY  (default 0.3) — per-zone exponential penalty
//   FREECUT_GA_FILL_PENALTY  (default 0.1) — largest-component fill penalty
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug)]
/// Per-run overrides for zones-aware GA fitness.
pub struct GaFitnessConfig {
    /// Exponential penalty applied for each connected waste zone beyond the first.
    pub zone_penalty: f64,
    /// Exponential penalty applied when the largest waste component is small.
    pub fill_penalty: f64,
}

thread_local! {
    static GA_FITNESS_CONFIG: std::cell::Cell<Option<GaFitnessConfig>> = std::cell::Cell::new(None);
}

/// Execute `f` with a thread-local zones-aware GA fitness configuration.
///
/// This is used by service-level callers that run multiple optimizer profiles
/// in the same process. The override is scoped to the current thread, which
/// matches the optimizer's `spawn_blocking` execution model.
pub fn with_ga_fitness_config<T, F>(config: GaFitnessConfig, f: F) -> T
where
    F: FnOnce() -> T,
{
    GA_FITNESS_CONFIG.with(|cell| {
        let previous = cell.replace(Some(config));
        let result = f();
        cell.set(previous);
        result
    })
}

fn ga_zone_penalty() -> f64 {
    if let Some(config) = GA_FITNESS_CONFIG.with(|cell| cell.get()) {
        return config.zone_penalty;
    }
    static VAL: std::sync::OnceLock<f64> = std::sync::OnceLock::new();
    *VAL.get_or_init(|| {
        std::env::var("FREECUT_GA_ZONE_PENALTY")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(0.3)
    })
}

fn ga_fill_penalty() -> f64 {
    if let Some(config) = GA_FITNESS_CONFIG.with(|cell| cell.get()) {
        return config.fill_penalty;
    }
    static VAL: std::sync::OnceLock<f64> = std::sync::OnceLock::new();
    *VAL.get_or_init(|| {
        std::env::var("FREECUT_GA_FILL_PENALTY")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(0.1)
    })
}

/// Count connected components among `free_rects` using union-find.
/// Two rects are connected when their inflated bounding boxes touch
/// (tolerance = blade_width, so kerf-adjacent waste zones merge).
fn free_rect_connected_components(free_rects: &[Rect], blade_width: usize) -> (usize, u64) {
    let n = free_rects.len();
    if n == 0 {
        return (0, 0);
    }
    // Union-Find with path compression.
    let mut parent: Vec<usize> = (0..n).collect();
    let mut rank: Vec<usize> = vec![0; n];

    fn find(parent: &mut [usize], mut x: usize) -> usize {
        while parent[x] != x {
            parent[x] = parent[parent[x]];
            x = parent[x];
        }
        x
    }
    fn union(parent: &mut [usize], rank: &mut [usize], a: usize, b: usize) {
        let ra = find(parent, a);
        let rb = find(parent, b);
        if ra == rb {
            return;
        }
        if rank[ra] < rank[rb] {
            parent[ra] = rb;
        } else if rank[ra] > rank[rb] {
            parent[rb] = ra;
        } else {
            parent[rb] = ra;
            rank[ra] += 1;
        }
    }

    let tol = blade_width;
    for i in 0..n {
        let ri = &free_rects[i];
        let x1i = ri.x + ri.width;
        let y1i = ri.y + ri.length;
        for j in (i + 1)..n {
            let rj = &free_rects[j];
            let x1j = rj.x + rj.width;
            let y1j = rj.y + rj.length;
            // Touch/overlap with tolerance.
            if ri.x <= x1j + tol && x1i + tol >= rj.x && ri.y <= y1j + tol && y1i + tol >= rj.y {
                union(&mut parent, &mut rank, i, j);
            }
        }
    }

    // Count distinct roots and find the largest component area.
    let mut root_area: std::collections::HashMap<usize, u64> = std::collections::HashMap::new();
    for i in 0..n {
        let root = find(&mut parent, i);
        let r = &free_rects[i];
        let area = r.width as u64 * r.length as u64;
        *root_area.entry(root).or_insert(0) += area;
    }
    let n_components = root_area.len();
    let largest_area = root_area.values().copied().max().unwrap_or(0);
    (n_components, largest_area)
}


/// Represents a bin used for bin-packing.
trait Bin {
    /// Heuristic used for inserting `CutPiece`s.
    type Heuristic;

    /// Creates a new `Bin`.
    fn new(
        width: usize,
        length: usize,
        blade_width: usize,
        pattern_direction: PatternDirection,
        price: usize,
    ) -> Self;

    /// Computes the fitness of this `Bin` on a scale of 0.0 to 1.0, with 1.0 being the most fit.
    fn fitness(&self) -> f64;

    fn price(&self) -> usize;

    /// Removes `UsedCutPiece`s from this `Bin` and returns how many were removed.
    fn remove_cut_pieces<I>(&mut self, cut_pieces: I) -> usize
    where
        I: Iterator,
        I::Item: Borrow<UsedCutPiece>;

    /// Returns an iterator over the `UsedCutPiece`s in this `Bin`.
    fn cut_pieces(&self) -> std::slice::Iter<'_, UsedCutPiece>;

    /// Returns the possible heuristics that can be passed to `insert_cut_piece_with_heuristic`.
    fn possible_heuristics() -> Vec<Self::Heuristic>;

    /// Inserts the `CutPieceWithId` into this `Bin` using the specified heuristic. Returns whether
    /// the insert succeeded.
    fn insert_cut_piece_with_heuristic(
        &mut self,
        cut_piece: &CutPieceWithId,
        heuristic: &Self::Heuristic,
    ) -> bool;

    /// Inserts the `CutPieceWithId` into this `Bin` using a random heuristic. Returns whether
    /// the insert succeeded.
    fn insert_cut_piece_random_heuristic<R>(
        &mut self,
        cut_piece: &CutPieceWithId,
        rng: &mut R,
    ) -> bool
    where
        R: Rng + ?Sized;

    /// Returns whether the `StockPiece` is equivalent to this `Bin`.
    fn matches_stock_piece(&self, stock_piece: &StockPiece) -> bool;
}

struct OptimizerUnit<'a, B>
where
    B: Bin,
{
    bins: Vec<B>,

    // All of the possible stock pieces. It remains constant.
    possible_stock_pieces: &'a [StockPiece],

    // Stock pieces that are currently available to use for new bins.
    available_stock_pieces: Vec<StockPiece>,

    // Cut pieces that couldn't be added to bins.
    unused_cut_pieces: FnvHashSet<CutPieceWithId>,

    blade_width: usize,
}

impl<'a, B> Clone for OptimizerUnit<'a, B>
where
    B: Bin + Clone,
{
    fn clone(&self) -> Self {
        Self {
            bins: self.bins.clone(),
            possible_stock_pieces: self.possible_stock_pieces,
            available_stock_pieces: self.available_stock_pieces.clone(),
            unused_cut_pieces: self.unused_cut_pieces.clone(),
            blade_width: self.blade_width,
        }
    }
}

impl<'a, B> OptimizerUnit<'a, B>
where
    B: Bin,
{
    fn with_random_heuristics<R>(
        possible_stock_pieces: &'a [StockPiece],
        cut_pieces: &[&CutPieceWithId],
        blade_width: usize,
        rng: &mut R,
    ) -> Result<OptimizerUnit<'a, B>>
    where
        R: Rng + ?Sized,
    {
        let mut unit = OptimizerUnit {
            bins: Vec::new(),
            possible_stock_pieces,
            available_stock_pieces: possible_stock_pieces.to_vec(),
            unused_cut_pieces: Default::default(),
            blade_width,
        };

        for cut_piece in cut_pieces {
            if !unit.first_fit_random_heuristics(cut_piece, rng) {
                unit.unused_cut_pieces.insert((*cut_piece).clone());
            }
        }

        Ok(unit)
    }

    fn with_heuristic<R>(
        possible_stock_pieces: &'a [StockPiece],
        cut_pieces: &[&CutPieceWithId],
        blade_width: usize,
        heuristic: &B::Heuristic,
        rng: &mut R,
    ) -> Result<OptimizerUnit<'a, B>>
    where
        R: Rng + ?Sized,
    {
        let mut unit = OptimizerUnit {
            bins: Vec::new(),
            possible_stock_pieces,
            available_stock_pieces: possible_stock_pieces.to_vec(),
            unused_cut_pieces: Default::default(),
            blade_width,
        };

        for cut_piece in cut_pieces {
            if !unit.first_fit_with_heuristic(cut_piece, heuristic, rng) {
                unit.unused_cut_pieces.insert((*cut_piece).clone());
            }
        }

        Ok(unit)
    }

    pub(crate) fn generate_initial_units(
        possible_stock_pieces: &'a [StockPiece],
        mut cut_pieces: Vec<&CutPieceWithId>,
        blade_width: usize,
        random_seed: u64,
    ) -> Result<Vec<OptimizerUnit<'a, B>>> {
        let mut set = FnvHashSet::default();
        for cut_piece in &cut_pieces {
            set.insert((
                cut_piece.width,
                cut_piece.length,
                cut_piece.can_rotate,
                cut_piece.pattern_direction,
            ));
        }
        let unique_cut_pieces = set.len();

        let possible_heuristics = B::possible_heuristics();

        let num_units = if cut_pieces.len() < 3 {
            possible_heuristics.len()
        } else {
            let denom = if cut_pieces.len() > 1 {
                (cut_pieces.len() as f64).log10()
            } else {
                1.0
            };

            cmp::max(
                possible_heuristics.len() * 3,
                (cut_pieces.len() as f64 / denom + ((unique_cut_pieces - 1) * 10) as f64) as usize,
            )
        };
        let mut units = Vec::with_capacity(num_units);
        let mut rng: StdRng = SeedableRng::seed_from_u64(random_seed);

        cut_pieces.sort_by_key(|p| cmp::Reverse((p.width, p.length)));
        for heuristic in &possible_heuristics {
            units.push(OptimizerUnit::with_heuristic(
                possible_stock_pieces,
                &cut_pieces,
                blade_width,
                heuristic,
                &mut rng,
            )?);
        }

        if cut_pieces.len() > 2 {
            for heuristic in &possible_heuristics {
                cut_pieces.shuffle(&mut rng);
                units.push(OptimizerUnit::with_heuristic(
                    possible_stock_pieces,
                    &cut_pieces,
                    blade_width,
                    heuristic,
                    &mut rng,
                )?);
            }

            for _ in 0..num_units - units.len() {
                cut_pieces.shuffle(&mut rng);
                units.push(OptimizerUnit::with_random_heuristics(
                    possible_stock_pieces,
                    &cut_pieces,
                    blade_width,
                    &mut rng,
                )?);
            }
        }
        Ok(units)
    }

    fn first_fit_random_heuristics<R>(&mut self, cut_piece: &CutPieceWithId, rng: &mut R) -> bool
    where
        R: Rng + ?Sized,
    {
        for bin in self.bins.iter_mut() {
            if bin.insert_cut_piece_random_heuristic(cut_piece, rng) {
                return true;
            }
        }

        self.add_to_new_bin(cut_piece, rng)
    }

    fn first_fit_with_heuristic<R>(
        &mut self,
        cut_piece: &CutPieceWithId,
        heuristic: &B::Heuristic,
        rng: &mut R,
    ) -> bool
    where
        R: Rng + ?Sized,
    {
        for bin in self.bins.iter_mut() {
            if bin.insert_cut_piece_with_heuristic(cut_piece, heuristic) {
                return true;
            }
        }

        self.add_to_new_bin(cut_piece, rng)
    }

    fn add_to_new_bin<R>(&mut self, cut_piece: &CutPieceWithId, rng: &mut R) -> bool
    where
        R: Rng + ?Sized,
    {
        let stock_pieces = self
            .available_stock_pieces
            .iter_mut()
            .filter(|stock_piece| {
                stock_piece.quantity != Some(0) && stock_piece.fits_cut_piece(cut_piece)
            });

        match stock_pieces.choose(rng) {
            Some(stock_piece) => {
                stock_piece.dec_quantity();

                let mut bin = B::new(
                    stock_piece.width,
                    stock_piece.length,
                    self.blade_width,
                    stock_piece.pattern_direction,
                    stock_piece.price,
                );
                if !bin.insert_cut_piece_random_heuristic(cut_piece, rng) {
                    return false;
                }
                self.bins.push(bin);
                true
            }
            None => false,
        }
    }

    fn crossover<R>(&self, other: &OptimizerUnit<'a, B>, rng: &mut R) -> OptimizerUnit<'a, B>
    where
        R: Rng + ?Sized,
        B: Clone,
    {
        // If there aren't multiple bins we can't do a crossover,
        // so just return a clone of this unit.
        if self.bins.len() < 2 && other.bins.len() < 2 {
            return self.clone();
        }

        // Randomly select insertion point and range of bins to inject into the new unit.
        let cross_dest = rng.gen_range(0..=self.bins.len());
        let cross_src_start = rng.gen_range(0..other.bins.len());
        let cross_src_end = rng.gen_range(cross_src_start + 1..=other.bins.len());

        // Create a new unit with the bins of `self` and some of the bins of `other` injected in.
        let mut new_unit = OptimizerUnit {
            bins: (&self.bins[..cross_dest])
                .iter()
                .chain((&other.bins[cross_src_start..cross_src_end]).iter())
                .chain((&self.bins[cross_dest..]).iter())
                .cloned()
                .collect(),
            possible_stock_pieces: self.possible_stock_pieces,
            // Make all possible stock pieces available initially. Quantities will be updated below.
            available_stock_pieces: self.possible_stock_pieces.to_vec(),
            // Start with no unused cut pieces, and update below.
            unused_cut_pieces: Default::default(),
            blade_width: self.blade_width,
        };

        let mut unused_cut_pieces = self.unused_cut_pieces.clone();

        // Update available stock piece quantities based on the injected bins.
        other.bins[cross_src_start..cross_src_end]
            .iter()
            .for_each(|bin| {
                if let Some(ref mut stock_piece) = new_unit
                    .available_stock_pieces
                    .iter_mut()
                    .find(|sp| bin.matches_stock_piece(sp))
                {
                    // Remove the injected cut pieces from the unused set.
                    for cut_piece in bin.cut_pieces() {
                        unused_cut_pieces.remove(&cut_piece.into());
                    }
                    stock_piece.dec_quantity();
                } else {
                    panic!("Attempt to inject invalid bin in crossover operation. This shouldn't happen, and means there is a bug in the code.");
                }
            });

        // Remove injected cut pieces from all non-injected bins,
        // and update the available stock piece quantities.
        for i in (0..cross_dest)
            .chain((cross_dest + cross_src_end - cross_src_start)..new_unit.bins.len())
            .rev()
        {
            let bin = &mut new_unit.bins[i];
            let stock_piece = new_unit
                .available_stock_pieces
                .iter_mut()
                .find(|sp| sp.quantity != Some(0) && bin.matches_stock_piece(sp));
            let injected_cut_pieces = (&other.bins[cross_src_start..cross_src_end])
                .iter()
                .flat_map(Bin::cut_pieces);
            let num_removed_cut_pieces = bin.remove_cut_pieces(injected_cut_pieces);

            if let (0, Some(stock_piece)) = (num_removed_cut_pieces, stock_piece) {
                // No cut pieces were removed from this bin, so we'll keep it
                // and decrement the quantity of the available stock piece.
                stock_piece.dec_quantity();
            } else {
                // Either there were no available stock pieces, or there
                // were cut pieces removed from this bin so we'll remove it
                // and add its cut pieces to the unused set.
                for cut_piece in bin.cut_pieces() {
                    unused_cut_pieces.insert(cut_piece.into());
                }
                new_unit.bins.remove(i);
            }
        }

        // Try to add all unused cut pieces.
        unused_cut_pieces.retain(|cut_piece| !new_unit.first_fit_random_heuristics(cut_piece, rng));
        new_unit.unused_cut_pieces = unused_cut_pieces;

        // Remove bins that don't have cut pieces.
        for i in (0..new_unit.bins.len()).rev() {
            if new_unit.bins[i].cut_pieces().next().is_none() {
                let bin = &mut new_unit.bins[i];
                if let Some(ref mut stock_piece) = new_unit
                    .available_stock_pieces
                    .iter_mut()
                    .find(|sp| sp.quantity != Some(0) && bin.matches_stock_piece(sp))
                {
                    // Make this stock piece available to use again.
                    stock_piece.inc_quantity();
                }
                new_unit.bins.remove(i);
            }
        }

        new_unit
    }

    // Randomly apply a mutation to this unit.
    fn mutate<R>(&mut self, rng: &mut R)
    where
        R: Rng + ?Sized,
    {
        if !self.bins.is_empty() && rng.gen_range(0..20) == 1 {
            self.inversion(rng)
        }
    }

    // Reverse order of a random range of bins.
    fn inversion<R>(&mut self, rng: &mut R)
    where
        R: Rng + ?Sized,
    {
        let start = rng.gen_range(0..self.bins.len());
        let end = rng.gen_range(start..self.bins.len());
        self.bins[start..end].reverse();
    }
}

impl<'a, B> Unit for OptimizerUnit<'a, B>
where
    B: Bin + Send + Clone,
{
    fn fitness(&self) -> f64 {
        let fitness = if self.bins.is_empty() {
            0.0
        } else {
            self.bins.iter().fold(0.0, |acc, b| acc + b.fitness()) / self.bins.len() as f64
        };

        if self.unused_cut_pieces.is_empty() {
            fitness
        } else {
            // If there are unused cut pieces, the fitness is below 0 because it's not a valid
            // solution.
            fitness - 1.0
        }
    }

    fn breed_with<R>(&self, other: &OptimizerUnit<'a, B>, rng: &mut R) -> OptimizerUnit<'a, B>
    where
        R: Rng + ?Sized,
    {
        let mut new_unit = self.crossover(other, rng);
        new_unit.mutate(rng);
        new_unit
    }
}

/// Error while optimizing.
#[derive(Debug)]
pub enum Error {
    /// There was no stock piece that could contain this demand piece.
    NoFitForCutPiece(CutPiece),
}
fn no_fit_for_cut_piece_error(cut_piece: &CutPieceWithId) -> Error {
    Error::NoFitForCutPiece(CutPiece {
        quantity: 1,
        external_id: cut_piece.external_id,
        width: cut_piece.width,
        length: cut_piece.length,
        can_rotate: cut_piece.can_rotate,
        pattern_direction: cut_piece.pattern_direction,
    })
}
type Result<T> = std::result::Result<T, Error>;

/// A valid solution to an optimization.
#[cfg_attr(feature = "serialize", derive(Deserialize, Serialize))]
#[cfg_attr(feature = "serialize", serde(rename_all = "camelCase"))]
pub struct Solution {
    /// Fitness score for this solution.
    /// Ranges between 0.0 and 1.0 inclusive, with 1.0 being a perfect solution with no waste.
    pub fitness: f64,

    /// The stock pieces that were used for this solution, each containing the demand piece layout.
    pub stock_pieces: Vec<ResultStockPiece>,

    #[cfg_attr(feature = "serialize", serde(skip))]
    price: usize,
}

impl Solution {
    /// Build a `Solution` from explicit components.  Mostly used by callers
    /// that want to inject a hand-crafted (or heuristic) solution into the
    /// candidate pool, e.g. to seed the GA population with a known-good
    /// First-Fit-Decreasing layout.
    pub fn from_components(
        fitness: f64,
        stock_pieces: Vec<ResultStockPiece>,
        price: usize,
    ) -> Self {
        Self {
            fitness,
            stock_pieces,
            price,
        }
    }
}

/// Result set with multiple ranked solution candidates.
#[cfg_attr(feature = "serialize", derive(Deserialize, Serialize))]
#[cfg_attr(feature = "serialize", serde(rename_all = "camelCase"))]
pub struct SolutionSet {
    /// Ranked solution candidates. Index 0 is the best candidate.
    pub solutions: Vec<Solution>,
}

/// Optimizer for optimizing rectangular cut pieces from rectangular
/// stock pieces.
pub struct Optimizer {
    stock_pieces: Vec<StockPiece>,
    cut_pieces: Vec<CutPieceWithId>,
    cut_width: usize,
    random_seed: u64,
    allow_mixed_stock_sizes: bool,
    ga_epochs: u32,
    ga_breed_factor: f64,
    ga_survival_factor: f64,
}

impl Default for Optimizer {
    fn default() -> Self {
        Self {
            stock_pieces: Default::default(),
            cut_pieces: Default::default(),
            cut_width: Default::default(),
            random_seed: Default::default(),
            allow_mixed_stock_sizes: true,
            ga_epochs: 100,
            ga_breed_factor: 0.5,
            ga_survival_factor: 0.6,
        }
    }
}

impl Optimizer {
    /// Create a new optimizer.
    pub fn new() -> Self {
        Default::default()
    }

    /// Add a stock piece that the optimizer can use to optimize cut pieces.
    /// If the same stock piece is added multiple times, the quantities will be
    /// summed up. If any have a `None` quantity, the quantity on other equivalent
    /// pieces will be ignored.
    pub fn add_stock_piece(&mut self, stock_piece: StockPiece) -> &mut Self {
        let mut existing_stock_piece = self.stock_pieces.iter_mut().find(|sp| {
            sp.width == stock_piece.width
                && sp.length == stock_piece.length
                && sp.pattern_direction == stock_piece.pattern_direction
                && sp.price == stock_piece.price
        });

        if let Some(ref mut existing_stock_piece) = existing_stock_piece {
            match (&mut existing_stock_piece.quantity, stock_piece.quantity) {
                (Some(ref mut existing_quantity), Some(quantity)) => {
                    // If there is already a stock piece that is the same except the quantity, add
                    // to the quantity.
                    *existing_quantity += quantity;
                }
                _ => {
                    // `None` quantity means infinite, so if either is `None` we want it to be
                    // `None`.
                    existing_stock_piece.quantity = None;
                }
            }
        } else {
            // A stock piece like this hasn't yet been added so let's do it.
            self.stock_pieces.push(stock_piece);
        }

        self
    }

    /// Add a stock pieces that the optimizer can use to optimize cut pieces.
    /// If the same stock piece is added multiple times, the quantities will be
    /// summed up. If any have a `None` quantity, the quantity on other equivalent
    /// pieces will be ignored.
    pub fn add_stock_pieces<I>(&mut self, stock_pieces: I) -> &mut Self
    where
        I: IntoIterator<Item = StockPiece>,
    {
        stock_pieces.into_iter().for_each(|sp| {
            self.add_stock_piece(sp);
        });
        self
    }

    /// Add a desired cut piece that you need cut from a stock piece.
    pub fn add_cut_piece(&mut self, cut_piece: CutPiece) -> &mut Self {
        for _ in 0..cut_piece.quantity {
            let cut_piece = CutPieceWithId {
                id: self.cut_pieces.len(),
                external_id: cut_piece.external_id,
                width: cut_piece.width,
                length: cut_piece.length,
                pattern_direction: cut_piece.pattern_direction,
                can_rotate: cut_piece.can_rotate,
            };

            self.cut_pieces.push(cut_piece);
        }

        self
    }

    /// Add desired cut pieces that you need cut from a stock piece.
    pub fn add_cut_pieces<I>(&mut self, cut_pieces: I) -> &mut Self
    where
        I: IntoIterator<Item = CutPiece>,
    {
        cut_pieces.into_iter().for_each(|dp| {
            self.add_cut_piece(dp);
        });
        self
    }

    /// Set the width of the cut to use between cut pieces. This could
    /// represent blade or kerf thickness.
    pub fn set_cut_width(&mut self, cut_width: usize) -> &mut Self {
        self.cut_width = cut_width;
        self
    }

    /// Set the random seed used by the genetic algorithms in the optimizer. Using
    /// the same random seed will give you the same result for the same input.
    pub fn set_random_seed(&mut self, seed: u64) -> &mut Self {
        self.random_seed = seed;
        self
    }

    /// Set whether the optimizer should allow mixed sized stock pieces in the results.
    /// If set to false, and multiple stock sizes are given, only one stock size will be used in
    /// the results.
    pub fn allow_mixed_stock_sizes(&mut self, allow: bool) -> &mut Self {
        self.allow_mixed_stock_sizes = allow;
        self
    }

    /// Set number of epochs for the genetic search.
    pub fn set_ga_epochs(&mut self, epochs: u32) -> &mut Self {
        self.ga_epochs = epochs.max(1);
        self
    }

    /// Set GA breed factor in range (0, 1].
    pub fn set_ga_breed_factor(&mut self, breed_factor: f64) -> &mut Self {
        assert!(breed_factor > 0.0 && breed_factor <= 1.0);
        self.ga_breed_factor = breed_factor;
        self
    }

    /// Set GA survival factor in range [0, 1].
    pub fn set_ga_survival_factor(&mut self, survival_factor: f64) -> &mut Self {
        assert!((0.0..=1.0).contains(&survival_factor));
        self.ga_survival_factor = survival_factor;
        self
    }

    /// Optimize in a way where each cut piece can be cut out using only guillotine cuts,
    /// where each cut extends from one side to the other.
    ///
    /// This method is suitable for cutting with a panel saw.
    pub fn optimize_guillotine<F>(&self, progress_callback: F) -> Result<Solution>
    where
        F: Fn(f64),
    {
        self.optimize::<GuillotineBin, F>(progress_callback)
    }

    /// Optimize in a way where each cut piece can be cut out using only guillotine cuts and
    /// return top-K ranked candidates from the population.
    ///
    /// This method is suitable for cutting with a panel saw.
    pub fn optimize_guillotine_top_k<F>(
        &self,
        top_k: usize,
        progress_callback: F,
    ) -> Result<SolutionSet>
    where
        F: Fn(f64),
    {
        self.optimize_top_k::<GuillotineBin, F>(top_k, progress_callback)
    }

    /// Optimize without the requirement of guillotine cuts. Cuts can start and stop in the middle
    /// of the stock piece.
    ///
    /// This method is suitable for cutting on a CNC.
    pub fn optimize_nested<F>(&self, progress_callback: F) -> Result<Solution>
    where
        F: Fn(f64),
    {
        self.optimize::<MaxRectsBin, F>(progress_callback)
    }

    /// Optimize without the requirement of guillotine cuts and return top-K ranked candidates
    /// from the population.
    ///
    /// This method is suitable for cutting on a CNC.
    pub fn optimize_nested_top_k<F>(&self, top_k: usize, progress_callback: F) -> Result<SolutionSet>
    where
        F: Fn(f64),
    {
        self.optimize_top_k::<MaxRectsBin, F>(top_k, progress_callback)
    }

    /// Build a First-Fit-Decreasing heuristic solution for guillotine
    /// mode (no GA evolution).  Sorts cut pieces by area (largest first)
    /// and places each in the first available free rectangle using the
    /// first available heuristic.  Returns a Vec<Solution> with the
    /// resulting layout (or empty if pieces don't fit).
    pub fn build_guillotine_heuristic(&self) -> Vec<Solution> {
        // H2/V56: evaluate EVERY heuristic variant (not just the first) and
        // return one Solution per variant. The service-level pick_best_candidate
        // then selects the best construction, which raises the heuristic floor
        // that large timed-out jobs fall back to (see V55/H1).
        // Sort cut pieces by area, largest first.
        let mut cuts: Vec<&CutPieceWithId> = self.cut_pieces.iter().collect();
        cuts.sort_by_key(|c| {
            std::cmp::Reverse((c.width as u64).saturating_mul(c.length as u64))
        });
        let mut solutions = Vec::new();
        for heuristic in &GuillotineBin::possible_heuristics() {
            let mut rng = rand::rngs::StdRng::seed_from_u64(self.random_seed);
            // V7 fix: with_heuristic already places every cut piece via
            // first_fit_with_heuristic, so we must NOT iterate cuts a second
            // time (that would duplicate placements, N -> N*2).
            let Ok(mut unit) = OptimizerUnit::<GuillotineBin>::with_heuristic(
                &self.stock_pieces,
                &cuts,
                self.cut_width,
                heuristic,
                &mut rng,
            ) else {
                continue;
            };
            if unit.bins.is_empty() {
                continue;
            }
            let mut result_sheets: Vec<ResultStockPiece> =
                unit.bins.drain(..).map(Into::into).collect();
            result_sheets.sort_by_key(|p| std::cmp::Reverse((p.width, p.length)));
            let fitness = unit.fitness();
            let price = result_sheets.iter().map(|s| s.price).sum();
            solutions.push(Solution::from_components(fitness, result_sheets, price));
        }
        solutions
    }

    /// Build a First-Fit-Decreasing heuristic solution for nested mode
    /// (no GA evolution).  Same shape as `build_guillotine_heuristic`
    /// but uses `MaxRectsBin` heuristics.
    pub fn build_nested_heuristic(&self) -> Vec<Solution> {
        // H2/V56: evaluate EVERY MaxRects heuristic variant, one Solution each.
        let mut cuts: Vec<&CutPieceWithId> = self.cut_pieces.iter().collect();
        cuts.sort_by_key(|c| {
            std::cmp::Reverse((c.width as u64).saturating_mul(c.length as u64))
        });
        let mut solutions = Vec::new();
        for heuristic in &MaxRectsBin::possible_heuristics() {
            let mut rng = rand::rngs::StdRng::seed_from_u64(self.random_seed);
            // V7 fix: with_heuristic already places every cut piece; no 2nd pass.
            let Ok(mut unit) = OptimizerUnit::<MaxRectsBin>::with_heuristic(
                &self.stock_pieces,
                &cuts,
                self.cut_width,
                heuristic,
                &mut rng,
            ) else {
                continue;
            };
            if unit.bins.is_empty() {
                continue;
            }
            let mut result_sheets: Vec<ResultStockPiece> =
                unit.bins.drain(..).map(Into::into).collect();
            result_sheets.sort_by_key(|p| std::cmp::Reverse((p.width, p.length)));
            let fitness = unit.fitness();
            let price = result_sheets.iter().map(|s| s.price).sum();
            solutions.push(Solution::from_components(fitness, result_sheets, price));
        }
        solutions
    }

    fn optimize<B, F>(&self, progress_callback: F) -> Result<Solution>
    where
        B: Bin + Clone + Send + Into<ResultStockPiece>,
        F: Fn(f64),
    {
        let mut set = self.optimize_top_k::<B, F>(1, progress_callback)?;
        match set.solutions.pop() {
            Some(solution) => Ok(solution),
            None => Err(no_fit_for_cut_piece_error(
                self.cut_pieces
                    .first()
                    .expect("cut pieces should be non-empty when optimize() runs"),
            )),
        }
    }

    fn optimize_top_k<B, F>(&self, top_k: usize, progress_callback: F) -> Result<SolutionSet>
    where
        B: Bin + Clone + Send + Into<ResultStockPiece>,
        F: Fn(f64),
    {
        let top_k = top_k.max(1);
        // If there are no cut pieces, there's nothing to optimize.
        if self.cut_pieces.is_empty() {
            return Ok(SolutionSet {
                solutions: vec![Solution {
                    fitness: 1.0,
                    stock_pieces: Vec::new(),
                    price: 0,
                }],
            });
        }

        let size_set: FnvHashSet<(usize, usize)> = self
            .stock_pieces
            .iter()
            .map(|sp| (sp.width, sp.length))
            .collect();

        let num_runs = size_set.len() + if self.allow_mixed_stock_sizes { 1 } else { 0 };
        let callback = |progress| {
            progress_callback(progress / num_runs as f64);
        };

        let mut all_candidates: Vec<Solution> = Vec::new();
        if self.allow_mixed_stock_sizes {
            // Optimize with all stock sizes
            if let Ok(mut candidates) =
                self.optimize_with_stock_pieces_top_k::<B, _>(&self.stock_pieces.clone(), top_k, &callback)
            {
                all_candidates.append(&mut candidates);
            }
        }

        // Optimize each stock size separately and see if any have better result than
        // when optimizing with all stock sizes.
        for (i, (width, length)) in size_set.iter().enumerate() {
            let stock_pieces: Vec<StockPiece> = self
                .stock_pieces
                .iter()
                .filter(|sp| sp.width == *width && sp.length == *length)
                .cloned()
                .collect();

            let completed_runs = i + 1;
            if let Ok(mut candidates) =
                self.optimize_with_stock_pieces_top_k::<B, _>(&stock_pieces, top_k, &|progress| {
                    progress_callback((completed_runs as f64 + progress) / num_runs as f64);
                })
            {
                all_candidates.append(&mut candidates);
            }
        }

        if all_candidates.is_empty() {
            return Err(no_fit_for_cut_piece_error(&self.cut_pieces[0]));
        }

        all_candidates.sort_by(solution_ordering_desc);
        all_candidates.truncate(top_k);
        Ok(SolutionSet {
            solutions: all_candidates,
        })
    }

    fn optimize_with_stock_pieces<B, F>(
        &self,
        stock_pieces: &[StockPiece],
        progress_callback: &F,
    ) -> Result<Solution>
    where
        B: Bin + Clone + Send + Into<ResultStockPiece>,
        F: Fn(f64),
    {
        let mut candidates =
            self.optimize_with_stock_pieces_top_k::<B, F>(stock_pieces, 1, progress_callback)?;
        match candidates.pop() {
            Some(solution) => Ok(solution),
            None => Err(no_fit_for_cut_piece_error(
                self.cut_pieces
                    .first()
                    .expect("cut pieces should be non-empty when optimize_with_stock_pieces() runs"),
            )),
        }
    }

    fn optimize_with_stock_pieces_top_k<B, F>(
        &self,
        stock_pieces: &[StockPiece],
        top_k: usize,
        progress_callback: &F,
    ) -> Result<Vec<Solution>>
    where
        B: Bin + Clone + Send + Into<ResultStockPiece>,
        F: Fn(f64),
    {
        let top_k = top_k.max(1);
        let cut_pieces: Vec<&CutPieceWithId> = self.cut_pieces.iter().collect();

        let units: Vec<OptimizerUnit<B>> = OptimizerUnit::generate_initial_units(
            stock_pieces,
            cut_pieces,
            self.cut_width,
            self.random_seed,
        )?;

        let population_size = units.len();
        let mut result_units = Population::new(units)
            .set_size(population_size)
            .set_rand_seed(self.random_seed)
            .set_breed_factor(self.ga_breed_factor)
            .set_survival_factor(self.ga_survival_factor)
            .epochs(self.ga_epochs, progress_callback)
            .finish();

        if result_units.is_empty() {
            return Err(no_fit_for_cut_piece_error(&self.cut_pieces[0]));
        }

        if !result_units[0].unused_cut_pieces.is_empty() {
            return Err(no_fit_for_cut_piece_error(
                result_units[0].unused_cut_pieces.iter().next().unwrap(),
            ));
        }

        let mut solutions: Vec<Solution> = Vec::with_capacity(top_k);
        for mut unit in result_units.drain(..) {
            if !unit.unused_cut_pieces.is_empty() {
                break;
            }

            let fitness = unit.fitness();
            let price = unit.bins.iter().map(|bin| bin.price()).sum();

            let mut used_stock_pieces: Vec<ResultStockPiece> =
                unit.bins.drain(..).map(Into::into).collect();
            used_stock_pieces.sort_by_key(|p| cmp::Reverse((p.width, p.length)));

            solutions.push(Solution {
                fitness,
                stock_pieces: used_stock_pieces,
                price,
            });
            if solutions.len() >= top_k {
                break;
            }
        }

        if solutions.is_empty() {
            return Err(no_fit_for_cut_piece_error(&self.cut_pieces[0]));
        }

        Ok(solutions)
    }
}

fn solution_ordering_desc(a: &Solution, b: &Solution) -> std::cmp::Ordering {
    // Highest fitness first for invalid candidates. For valid candidates prefer lower price
    // and then higher fitness.
    if a.fitness < 0.0 || b.fitness < 0.0 {
        b.fitness
            .partial_cmp(&a.fitness)
            .unwrap_or(std::cmp::Ordering::Equal)
    } else {
        a.price
            .cmp(&b.price)
            .then_with(|| {
                b.fitness
                    .partial_cmp(&a.fitness)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
    }
}
