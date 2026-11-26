// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors.

//! Failure certificates of path searches, a rustora addition without a counterpart in OpenRA.
//!
//! When a bidirectional search fails, one of its two searches ran out of open cells. Its closed cells form the
//! region `C`, the 8-connected component of the cells it could enter, and the cells next to `C` form the rim `R`.
//! Any path between a cell in `C` and a cell outside `C ∪ R` enters a cell of `R`. A later search of any actor,
//! locomotor and check without custom cost therefore fails if one of its endpoints is in `C`, the other is in neither
//! `C` nor `R`, and the movement cost check of that search rejects every cell of `R` at the time of the search. Cells
//! of `C` that became blocked since only make the region smaller, so the certificate needs no invalidation. The
//! check decides only whether a search that can only fail runs, so the results stay those of OpenRA.
//!
//! The rim covers the 8 neighbors on the ground layer only. Custom movement layers (tunnels, elevated bridges,
//! jumpjet and subterranean layers) and the terrain height rule of Tiberian Sun add edges between cells that are no
//! such neighbors or depend on both cells; when they are ported, the rim must become the set of graph edges that
//! leave `C`, built from the same neighbor function as the search.

use std::cell::{Cell, RefCell};

use rustora_core::world_types::{CPos, CVec};
use rustora_engine::actor::ActorRef;
use rustora_engine::map::Map;
use rustora_engine::traits::subcell::FULL_CELL;

use crate::locomotor::{BlockedByActor, Locomotor, MOVEMENT_COST_FOR_UNREACHABLE_CELL};
use crate::pathfinder::{CellStatus, PathSearch};

/// Number of certificates kept, the most recently used first.
const MAX_CERTIFICATES: usize = 64;

/// A set of cells of the map as one bit per cell.
struct CellBits {
    width: i32,
    height: i32,
    bits: Vec<u64>,
}

impl CellBits {
    fn new(width: i32, height: i32) -> Self {
        CellBits {
            width,
            height,
            bits: vec![0; ((width * height) as usize).div_ceil(64)],
        }
    }

    fn index(&self, c: CPos) -> Option<usize> {
        (c.x() >= 0 && c.y() >= 0 && c.x() < self.width && c.y() < self.height).then(|| (c.y() * self.width + c.x()) as usize)
    }

    fn contains(&self, c: CPos) -> bool {
        self.index(c).is_some_and(|i| self.bits[i / 64] & (1 << (i % 64)) != 0)
    }

    /// Adds `c` and returns whether it was missing.
    fn insert(&mut self, c: CPos) -> bool {
        let Some(i) = self.index(c) else { return false };
        let word = &mut self.bits[i / 64];
        let bit = 1 << (i % 64);
        let missing = *word & bit == 0;
        *word |= bit;
        missing
    }
}

/// The region of a failed search and its rim.
struct Certificate {
    region: CellBits,
    rim_cells: CellBits,
    /// The rim cells inside the map, the only ones that a movement cost check can accept.
    rim: Vec<CPos>,
    /// The rim index where the last failed check found an enterable cell, where the next check starts.
    open_hint: Cell<usize>,
}

impl Certificate {
    fn from_closed_cells(map: &Map, exhausted: &PathSearch) -> Self {
        let (width, height) = map.map_size;
        let mut region = CellBits::new(width, height);
        for y in 0..height {
            for x in 0..width {
                let c = CPos::new(x, y);
                if exhausted.graph.get(c).status == CellStatus::Closed {
                    region.insert(c);
                }
            }
        }
        let mut rim_cells = CellBits::new(width, height);
        let mut rim = Vec::new();
        for y in 0..height {
            for x in 0..width {
                let c = CPos::new(x, y);
                if !region.contains(c) {
                    continue;
                }
                for d in CVec::DIRECTIONS {
                    let n = c + d;
                    if !region.contains(n) && rim_cells.insert(n) && map.contains(n) {
                        rim.push(n);
                    }
                }
            }
        }
        Certificate {
            region,
            rim_cells,
            rim,
            open_hint: Cell::new(0),
        }
    }

    fn separates(&self, a: CPos, b: CPos) -> bool {
        self.region.contains(a) && !self.region.contains(b) && !self.rim_cells.contains(b)
    }

    /// Whether the cost check of the search rejects every rim cell.
    fn rim_is_closed(&self, rejects: impl Fn(CPos) -> bool) -> bool {
        let n = self.rim.len();
        let start = self.open_hint.get().min(n);
        for i in (start..n).chain(0..start) {
            if !rejects(self.rim[i]) {
                self.open_hint.set(i);
                return false;
            }
        }
        true
    }
}

/// The certificates of the recent failed searches of a world, for all locomotors and checks.
#[derive(Default)]
pub struct FailureCertificates {
    certificates: RefCell<Vec<Certificate>>,
}

impl FailureCertificates {
    /// Returns whether a certificate proves that no path between `source` and `target` exists for the search of
    /// `actor` with `locomotor`, `check` and `ignore_actor` and without custom cost.
    pub fn prove_no_path(
        &self,
        map: &Map,
        locomotor: &Locomotor,
        actor: &ActorRef,
        source: CPos,
        target: CPos,
        check: BlockedByActor,
        ignore_actor: Option<&ActorRef>,
    ) -> bool {
        let mut certificates = self.certificates.borrow_mut();
        let owner_mask = actor.with_owner(|o| o.player_mask);
        let rejects = |c: CPos| {
            locomotor.movement_cost_to_enter_cell_in(map, Some((actor, owner_mask)), c, check, ignore_actor, false, FULL_CELL)
                == MOVEMENT_COST_FOR_UNREACHABLE_CELL
        };
        let found = certificates
            .iter()
            .position(|c| (c.separates(source, target) || c.separates(target, source)) && c.rim_is_closed(rejects));
        if let Some(i) = found {
            certificates[..=i].rotate_right(1);
        }
        found.is_some()
    }

    /// Adds the certificate of a failed search whose side `exhausted` ran out of open cells.
    pub fn add(&self, map: &Map, exhausted: &PathSearch) {
        let mut certificates = self.certificates.borrow_mut();
        certificates.insert(0, Certificate::from_closed_cells(map, exhausted));
        certificates.truncate(MAX_CERTIFICATES);
    }
}
