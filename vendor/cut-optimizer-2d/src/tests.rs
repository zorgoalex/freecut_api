use super::*;

static STOCK_PIECES: &[StockPiece] = &[
    StockPiece {
        width: 48,
        length: 96,
        pattern_direction: PatternDirection::None,
        price: 0,
        quantity: None,
    },
    StockPiece {
        width: 48,
        length: 120,
        pattern_direction: PatternDirection::None,
        price: 0,
        quantity: None,
    },
];

static CUT_PIECES: &[CutPiece] = &[
    CutPiece {
        quantity: 1,
        external_id: Some(1),
        width: 10,
        length: 30,
        pattern_direction: PatternDirection::None,
        can_rotate: true,
    },
    CutPiece {
        quantity: 1,
        external_id: Some(2),
        width: 20,
        length: 30,
        pattern_direction: PatternDirection::None,
        can_rotate: true,
    },
    CutPiece {
        quantity: 1,
        external_id: Some(3),
        width: 30,
        length: 30,
        pattern_direction: PatternDirection::None,
        can_rotate: true,
    },
    CutPiece {
        quantity: 1,
        external_id: Some(4),
        width: 40,
        length: 30,
        pattern_direction: PatternDirection::None,
        can_rotate: true,
    },
];

fn sanity_check_solution(solution: &Solution, num_cut_pieces: usize) {
    let stock_pieces = &solution.stock_pieces;

    assert!(solution.fitness <= 1.0);

    // The number of result cut pieces should match the number of input cut pieces.
    assert_eq!(
        stock_pieces
            .iter()
            .map(|sp| sp.cut_pieces.len())
            .sum::<usize>(),
        num_cut_pieces
    );

    for stock_piece in stock_pieces {
        for cut_piece in &stock_piece.cut_pieces {
            assert_eq!(stock_piece.pattern_direction, cut_piece.pattern_direction);
            let stock_piece_area = stock_piece.width * stock_piece.length;
            let cut_piece_area = stock_piece
                .cut_pieces
                .iter()
                .map(|cp| cp.width * cp.length)
                .sum::<usize>();
            let waste_piece_area = stock_piece
                .waste_pieces
                .iter()
                .map(|wp| wp.width * wp.length)
                .sum::<usize>();

            // Make sure the stock piece is big enough for the cut pieces and waste pieces.
            assert!(stock_piece_area >= cut_piece_area + waste_piece_area);
        }

        let rects: Vec<Rect> = stock_piece
            .cut_pieces
            .iter()
            .map(|cp| cp.into())
            .chain(stock_piece.waste_pieces.iter().cloned())
            .collect();

        // Assert that all cut pieces and waste pieces are disjoint.
        for i in (0..rects.len()).rev() {
            for j in (i + 1..rects.len()).rev() {
                assert!(!rects[j].contains(&rects[i]));
                assert!(!rects[i].contains(&rects[j]));
            }
        }
    }
}

#[test]
fn guillotine() {
    let solution = Optimizer::new()
        .add_stock_pieces(STOCK_PIECES.iter().cloned().collect::<Vec<_>>())
        .add_cut_pieces(CUT_PIECES.iter().cloned().collect::<Vec<_>>())
        .set_cut_width(1)
        .set_random_seed(1)
        .optimize_guillotine(|_| {})
        .unwrap();

    sanity_check_solution(&solution, CUT_PIECES.len());
}

#[test]
fn best_fit_assignment_never_increases_sheet_count() {
    // V60-lite: build_guillotine_heuristic now evaluates BOTH first-fit and
    // best-fit bin assignment and keeps both candidate sets. Adding the best-fit
    // candidates must never raise the minimum sheet count vs first-fit alone
    // (pick_best_candidate selects the fewest sheets), and best-fit must place
    // every piece. This is the zero-regression guarantee behind V60-lite.
    use rand::SeedableRng;

    let mut pieces = Vec::new();
    let mut id = 0usize;
    for (w, l, qty) in [
        (20usize, 30usize, 6usize),
        (18, 18, 8),
        (40, 25, 4),
        (12, 50, 5),
    ] {
        for _ in 0..qty {
            id += 1;
            pieces.push(CutPiece {
                quantity: 1,
                external_id: Some(id),
                width: w,
                length: l,
                pattern_direction: PatternDirection::None,
                can_rotate: true,
            });
        }
    }

    let mut optimizer = Optimizer::new();
    optimizer
        .add_stock_piece(StockPiece {
            width: 48,
            length: 96,
            pattern_direction: PatternDirection::None,
            price: 0,
            quantity: None,
        })
        .add_cut_pieces(pieces)
        .set_cut_width(1)
        .set_random_seed(1);

    // Sort decreasing by area, mirroring build_guillotine_heuristic.
    let mut cuts: Vec<&CutPieceWithId> = optimizer.cut_pieces.iter().collect();
    cuts.sort_by_key(|c| std::cmp::Reverse((c.width as u64).saturating_mul(c.length as u64)));
    let total = cuts.len();

    let min_bins_for = |assignment: BinAssignment| -> usize {
        let mut min_bins = usize::MAX;
        for heuristic in &GuillotineBin::possible_heuristics() {
            let mut rng = rand::rngs::StdRng::seed_from_u64(optimizer.random_seed);
            let unit = OptimizerUnit::<GuillotineBin>::with_heuristic_assignment(
                &optimizer.stock_pieces,
                &cuts,
                optimizer.cut_width,
                heuristic,
                assignment,
                &mut rng,
            )
            .unwrap();
            // Both strategies must place every piece (no dropped cuts).
            assert!(unit.unused_cut_pieces.is_empty());
            assert_eq!(
                unit.bins.iter().map(|b| b.cut_pieces().count()).sum::<usize>(),
                total
            );
            min_bins = min_bins.min(unit.bins.len());
        }
        min_bins
    };

    let ff = min_bins_for(BinAssignment::FirstFit);
    let bf = min_bins_for(BinAssignment::BestFit);

    // Keeping both candidate sets can only lower (or equal) the sheet count.
    let combined = ff.min(bf);
    assert!(
        combined <= ff,
        "best-fit assignment must never regress the minimum sheet count"
    );

    // The construction builder exposes exactly that combined minimum.
    let builder_min = optimizer
        .build_guillotine_heuristic()
        .iter()
        .map(|s| s.stock_pieces.len())
        .min()
        .expect("builder returns at least one solution");
    assert_eq!(builder_min, combined);
}

#[test]
fn guillotine_rotate() {
    let solution = Optimizer::new()
        .add_stock_piece(StockPiece {
            width: 10,
            length: 11,
            pattern_direction: PatternDirection::None,
            price: 0,
            quantity: None,
        })
        .add_cut_piece(CutPiece {
            quantity: 1,
            external_id: Some(1),
            width: 11,
            length: 10,
            pattern_direction: PatternDirection::None,
            can_rotate: true,
        })
        .set_cut_width(1)
        .set_random_seed(1)
        .optimize_guillotine(|_| {})
        .unwrap();

    sanity_check_solution(&solution, 1);

    let stock_pieces = solution.stock_pieces;
    assert_eq!(stock_pieces.len(), 1);
    let cut_pieces = &stock_pieces[0].cut_pieces;
    assert_eq!(cut_pieces.len(), 1);
    assert_eq!(
        cut_pieces[0],
        ResultCutPiece {
            external_id: Some(1),
            x: 0,
            y: 0,
            width: 10,
            length: 11,
            pattern_direction: PatternDirection::None,
            is_rotated: true,
        }
    );
}

#[test]
fn guillotine_rotate_pattern() {
    let solution = Optimizer::new()
        .add_stock_piece(StockPiece {
            width: 10,
            length: 11,
            pattern_direction: PatternDirection::ParallelToWidth,
            price: 0,
            quantity: None,
        })
        .add_cut_piece(CutPiece {
            quantity: 1,
            external_id: Some(1),
            width: 11,
            length: 10,
            pattern_direction: PatternDirection::ParallelToLength,
            can_rotate: true,
        })
        .set_cut_width(1)
        .set_random_seed(1)
        .optimize_guillotine(|_| {})
        .unwrap();

    sanity_check_solution(&solution, 1);

    let stock_pieces = solution.stock_pieces;
    assert_eq!(stock_pieces.len(), 1);
    let cut_pieces = &stock_pieces[0].cut_pieces;
    assert_eq!(cut_pieces.len(), 1);
    assert_eq!(
        cut_pieces[0],
        ResultCutPiece {
            external_id: Some(1),
            x: 0,
            y: 0,
            width: 10,
            length: 11,
            pattern_direction: PatternDirection::ParallelToWidth,
            is_rotated: true,
        }
    );
}

#[test]
fn guillotine_non_fitting_cut_piece_can_rotate() {
    let result = Optimizer::new()
        .add_stock_piece(StockPiece {
            width: 10,
            length: 10,
            pattern_direction: PatternDirection::None,
            price: 0,
            quantity: None,
        })
        .add_cut_piece(CutPiece {
            quantity: 1,
            external_id: Some(1),
            width: 11,
            length: 10,
            pattern_direction: PatternDirection::None,
            can_rotate: true,
        })
        .set_cut_width(1)
        .set_random_seed(1)
        .optimize_guillotine(|_| {});

    assert!(
        matches!(result, Err(Error::NoFitForCutPiece(_))),
        "should have returned Error::NoFitForCutPiece"
    )
}

#[test]
fn guillotine_non_fitting_cut_piece_no_rotate() {
    let result = Optimizer::new()
        .add_stock_piece(StockPiece {
            width: 10,
            length: 11,
            pattern_direction: PatternDirection::None,
            price: 0,
            quantity: None,
        })
        .add_cut_piece(CutPiece {
            quantity: 1,
            external_id: Some(1),
            width: 11,
            length: 10,
            pattern_direction: PatternDirection::None,
            can_rotate: false,
        })
        .set_cut_width(1)
        .set_random_seed(1)
        .optimize_guillotine(|_| {});

    assert!(
        matches!(result, Err(Error::NoFitForCutPiece(_))),
        "should have returned Error::NoFitForCutPiece"
    )
}

#[test]
fn guillotine_non_fitting_cut_piece_no_rotate_pattern() {
    let result = Optimizer::new()
        .add_stock_piece(StockPiece {
            width: 10,
            length: 11,
            pattern_direction: PatternDirection::ParallelToWidth,
            price: 0,
            quantity: None,
        })
        .add_cut_piece(CutPiece {
            quantity: 1,
            external_id: Some(1),
            width: 11,
            length: 10,
            pattern_direction: PatternDirection::ParallelToLength,
            can_rotate: false,
        })
        .set_cut_width(1)
        .set_random_seed(1)
        .optimize_guillotine(|_| {});

    assert!(
        matches!(result, Err(Error::NoFitForCutPiece(_))),
        "should have returned Error::NoFitForCutPiece"
    )
}

#[test]
fn guillotine_non_fitting_cut_piece_mismatched_pattern() {
    let result = Optimizer::new()
        .add_stock_piece(StockPiece {
            width: 100,
            length: 100,
            pattern_direction: PatternDirection::None,
            price: 0,
            quantity: None,
        })
        .add_cut_piece(CutPiece {
            quantity: 1,
            external_id: Some(1),
            width: 11,
            length: 10,
            pattern_direction: PatternDirection::ParallelToWidth,
            can_rotate: true,
        })
        .set_cut_width(1)
        .set_random_seed(1)
        .optimize_guillotine(|_| {});

    assert!(
        matches!(result, Err(Error::NoFitForCutPiece(_))),
        "should have returned Error::NoFitForCutPiece"
    )
}

#[test]
fn guillotine_no_allow_mixed_stock_sizes() {
    let solution = Optimizer::new()
        .add_stock_pieces(STOCK_PIECES.iter().cloned().collect::<Vec<_>>())
        .add_cut_piece(CutPiece {
            quantity: 1,
            external_id: Some(1),
            width: 48,
            length: 96,
            pattern_direction: PatternDirection::None,
            can_rotate: false,
        })
        .add_cut_piece(CutPiece {
            quantity: 1,
            external_id: Some(2),
            width: 48,
            length: 120,
            pattern_direction: PatternDirection::None,
            can_rotate: false,
        })
        .set_cut_width(1)
        .set_random_seed(1)
        .allow_mixed_stock_sizes(false)
        .optimize_guillotine(|_| {})
        .unwrap();

    sanity_check_solution(&solution, 2);

    assert_eq!(solution.stock_pieces.len(), 2);
    for stock_piece in solution.stock_pieces {
        // Since we aren't allowing mixed sizes,
        // all stock pieces will need to be 120 long.
        assert_eq!(stock_piece.length, 120)
    }
}

#[test]
fn guillotine_different_stock_piece_prices() {
    let solution = Optimizer::new()
        .add_stock_piece(StockPiece {
            width: 48,
            length: 96,
            pattern_direction: PatternDirection::None,
            price: 1,
            quantity: None,
        })
        .add_stock_piece(StockPiece {
            width: 48,
            length: 120,
            pattern_direction: PatternDirection::None,
            // Maker the 48x120 stock piece more expensive than (2) 48x96 pieces.
            price: 3,
            quantity: None,
        })
        .add_cut_piece(CutPiece {
            quantity: 1,
            external_id: Some(1),
            width: 48,
            length: 50,
            pattern_direction: PatternDirection::None,
            can_rotate: false,
        })
        .add_cut_piece(CutPiece {
            quantity: 1,
            external_id: Some(2),
            width: 48,
            length: 50,
            pattern_direction: PatternDirection::None,
            can_rotate: false,
        })
        .set_cut_width(1)
        .set_random_seed(1)
        .allow_mixed_stock_sizes(false)
        .optimize_guillotine(|_| {})
        .unwrap();

    sanity_check_solution(&solution, 2);

    // A single 48x120 stock piece could be used, but since we've set (2) 48x96 pieces to
    // be a lower price than (1) 48x120, it should use (2) 48x96 pieces instead.
    assert_eq!(solution.stock_pieces.len(), 2);
    for stock_piece in solution.stock_pieces {
        assert_eq!(stock_piece.length, 96)
    }
}

#[test]
fn guillotine_same_stock_piece_prices() {
    let solution = Optimizer::new()
        .add_stock_piece(StockPiece {
            width: 48,
            length: 96,
            pattern_direction: PatternDirection::None,
            price: 0,
            quantity: None,
        })
        .add_stock_piece(StockPiece {
            width: 48,
            length: 120,
            pattern_direction: PatternDirection::None,
            price: 0,
            quantity: None,
        })
        .add_cut_piece(CutPiece {
            quantity: 1,
            external_id: Some(1),
            width: 48,
            length: 50,
            pattern_direction: PatternDirection::None,
            can_rotate: false,
        })
        .add_cut_piece(CutPiece {
            quantity: 1,
            external_id: Some(2),
            width: 48,
            length: 50,
            pattern_direction: PatternDirection::None,
            can_rotate: false,
        })
        .set_cut_width(1)
        .set_random_seed(1)
        .allow_mixed_stock_sizes(false)
        .optimize_guillotine(|_| {})
        .unwrap();

    sanity_check_solution(&solution, 2);

    assert_eq!(solution.stock_pieces.len(), 1);
    assert_eq!(solution.stock_pieces[0].length, 120)
}

#[test]
fn guillotine_stock_quantity_too_low() {
    let result = Optimizer::new()
        .add_stock_piece(StockPiece {
            width: 48,
            length: 96,
            pattern_direction: PatternDirection::None,
            price: 0,
            quantity: Some(1),
        })
        .add_cut_piece(CutPiece {
            quantity: 2,
            external_id: None,
            width: 48,
            length: 96,
            pattern_direction: PatternDirection::None,
            can_rotate: false,
        })
        .set_cut_width(1)
        .set_random_seed(1)
        .optimize_guillotine(|_| {});

    assert!(
        result.is_err(),
        "should fail because stock quantity is too low"
    );
}

#[test]
fn guillotine_stock_quantity_1() {
    let solution = Optimizer::new()
        .add_stock_piece(StockPiece {
            width: 48,
            length: 96,
            pattern_direction: PatternDirection::None,
            price: 0,
            quantity: Some(1),
        })
        .add_cut_piece(CutPiece {
            quantity: 1,
            external_id: None,
            width: 10,
            length: 96,
            pattern_direction: PatternDirection::None,
            can_rotate: false,
        })
        .set_cut_width(1)
        .set_random_seed(1)
        .optimize_guillotine(|_| {})
        .unwrap();

    sanity_check_solution(&solution, 1);
}

#[test]
fn guillotine_stock_quantity_2() {
    let solution = Optimizer::new()
        .add_stock_piece(StockPiece {
            width: 48,
            length: 96,
            pattern_direction: PatternDirection::None,
            price: 0,
            quantity: Some(2),
        })
        .add_cut_piece(CutPiece {
            quantity: 2,
            external_id: None,
            width: 10,
            length: 96,
            pattern_direction: PatternDirection::None,
            can_rotate: false,
        })
        .set_cut_width(1)
        .set_random_seed(1)
        .optimize_guillotine(|_| {})
        .unwrap();

    sanity_check_solution(&solution, 2);
}

#[test]
fn guillotine_stock_quantity_multiple() {
    let solution = Optimizer::new()
        .add_stock_piece(StockPiece {
            width: 48,
            length: 96,
            pattern_direction: PatternDirection::None,
            price: 0,
            quantity: Some(2),
        })
        .add_stock_piece(StockPiece {
            width: 64,
            length: 192,
            pattern_direction: PatternDirection::None,
            price: 0,
            quantity: Some(1),
        })
        .add_cut_piece(CutPiece {
            quantity: 2,
            external_id: None,
            width: 48,
            length: 96,
            pattern_direction: PatternDirection::None,
            can_rotate: false,
        })
        .add_cut_piece(CutPiece {
            quantity: 1,
            external_id: None,
            width: 64,
            length: 192,
            pattern_direction: PatternDirection::None,
            can_rotate: false,
        })
        .set_cut_width(0)
        .set_random_seed(1)
        .optimize_guillotine(|_| {})
        .unwrap();

    sanity_check_solution(&solution, 3);
}

#[test]
fn guillotine_one_stock_piece_several_cut_pieces() {
    let solution = Optimizer::new()
        .add_stock_piece(StockPiece {
            width: 48,
            length: 96,
            pattern_direction: PatternDirection::None,
            price: 0,
            quantity: Some(1),
        })
        .add_cut_piece(CutPiece {
            quantity: 1,
            external_id: None,
            width: 8,
            length: 96,
            pattern_direction: PatternDirection::None,
            can_rotate: false,
        })
        .add_cut_piece(CutPiece {
            quantity: 2,
            external_id: None,
            width: 40,
            length: 10,
            pattern_direction: PatternDirection::None,
            can_rotate: false,
        })
        .add_cut_piece(CutPiece {
            quantity: 4,
            external_id: None,
            width: 20,
            length: 20,
            pattern_direction: PatternDirection::None,
            can_rotate: false,
        })
        .add_cut_piece(CutPiece {
            quantity: 1,
            external_id: None,
            width: 40,
            length: 36,
            pattern_direction: PatternDirection::None,
            can_rotate: false,
        })
        .set_cut_width(0)
        .set_random_seed(1)
        .optimize_guillotine(|_| {})
        .unwrap();

    sanity_check_solution(&solution, 8);
}

#[test]
fn guillotine_stock_duplicate_cut_piece() {
    let solution = Optimizer::new()
        .add_stock_piece(StockPiece {
            width: 48,
            length: 96,
            pattern_direction: PatternDirection::None,
            price: 0,
            quantity: Some(1),
        })
        .add_stock_piece(StockPiece {
            width: 64,
            length: 192,
            pattern_direction: PatternDirection::None,
            price: 0,
            quantity: Some(1),
        })
        .add_cut_piece(CutPiece {
            quantity: 2,
            external_id: None,
            width: 48,
            length: 96,
            pattern_direction: PatternDirection::None,
            can_rotate: false,
        })
        .set_cut_width(1)
        .set_random_seed(1)
        .optimize_guillotine(|_| {})
        .unwrap();

    sanity_check_solution(&solution, 2);
}

#[test]
fn guillotine_32_cut_pieces_on_1_stock_piece() {
    let mut optimizer = Optimizer::new();
    optimizer.add_stock_piece(StockPiece {
        width: 48,
        length: 96,
        pattern_direction: PatternDirection::None,
        price: 0,
        quantity: None,
    });

    let num_cut_pieces = 32;

    optimizer.add_cut_piece(CutPiece {
        quantity: num_cut_pieces,
        external_id: Some(1),
        width: 10,
        length: 10,
        pattern_direction: PatternDirection::None,
        can_rotate: false,
    });

    let solution = optimizer
        .set_cut_width(1)
        .set_random_seed(1)
        .optimize_guillotine(|_| {})
        .unwrap();

    sanity_check_solution(&solution, num_cut_pieces);

    let stock_pieces = solution.stock_pieces;
    assert_eq!(stock_pieces.len(), 1);
    let cut_pieces = &stock_pieces[0].cut_pieces;
    assert_eq!(cut_pieces.len(), 32);
}

#[test]
fn guillotine_32_cut_pieces_on_2_stock_piece_zero_cut_width() {
    let mut optimizer = Optimizer::new();
    optimizer.add_stock_piece(StockPiece {
        width: 48,
        length: 96,
        pattern_direction: PatternDirection::None,
        price: 0,
        quantity: None,
    });

    let num_cut_pieces = 32;

    optimizer.add_cut_piece(CutPiece {
        quantity: num_cut_pieces,
        external_id: Some(1),
        width: 12,
        length: 12,
        pattern_direction: PatternDirection::None,
        can_rotate: false,
    });

    let solution = optimizer
        .set_cut_width(0)
        .set_random_seed(1)
        .optimize_guillotine(|_| {})
        .unwrap();

    sanity_check_solution(&solution, num_cut_pieces);

    let stock_pieces = solution.stock_pieces;
    assert_eq!(stock_pieces.len(), 1);
    let cut_pieces = &stock_pieces[0].cut_pieces;
    assert_eq!(cut_pieces.len(), 32);
}

#[test]
fn guillotine_32_cut_pieces_on_2_stock_piece() {
    let mut optimizer = Optimizer::new();
    optimizer.add_stock_piece(StockPiece {
        width: 48,
        length: 96,
        pattern_direction: PatternDirection::None,
        price: 0,
        quantity: None,
    });

    let num_cut_pieces = 32;

    optimizer.add_cut_piece(CutPiece {
        quantity: num_cut_pieces,
        external_id: Some(1),
        width: 12,
        length: 12,
        pattern_direction: PatternDirection::None,
        can_rotate: false,
    });

    let solution = optimizer
        .set_cut_width(1)
        .set_random_seed(1)
        .optimize_guillotine(|_| {})
        .unwrap();

    sanity_check_solution(&solution, num_cut_pieces);

    let stock_pieces = solution.stock_pieces;
    assert_eq!(stock_pieces.len(), 2);
}

#[test]
fn guillotine_64_cut_pieces_on_2_stock_pieces() {
    let mut optimizer = Optimizer::new();
    optimizer.add_stock_piece(StockPiece {
        width: 48,
        length: 96,
        pattern_direction: PatternDirection::None,
        price: 0,
        quantity: None,
    });

    let num_cut_pieces = 64;

    optimizer.add_cut_piece(CutPiece {
        quantity: num_cut_pieces,
        external_id: Some(1),
        width: 10,
        length: 10,
        pattern_direction: PatternDirection::None,
        can_rotate: false,
    });

    let solution = optimizer
        .set_cut_width(1)
        .set_random_seed(1)
        .optimize_guillotine(|_| {})
        .unwrap();

    sanity_check_solution(&solution, num_cut_pieces);

    let stock_pieces = solution.stock_pieces;
    assert_eq!(stock_pieces.len(), 2);
    assert_eq!(stock_pieces[0].cut_pieces.len(), 32);
    assert_eq!(stock_pieces[1].cut_pieces.len(), 32);
}

#[test]
fn guillotine_random_cut_pieces() {
    let mut optimizer = Optimizer::new();
    optimizer.add_stock_piece(StockPiece {
        width: 48,
        length: 96,
        pattern_direction: PatternDirection::ParallelToWidth,
        price: 0,
        quantity: None,
    });
    optimizer.add_stock_piece(StockPiece {
        width: 48,
        length: 96,
        pattern_direction: PatternDirection::ParallelToLength,
        price: 0,
        quantity: None,
    });
    optimizer.add_stock_piece(StockPiece {
        width: 48,
        length: 120,
        pattern_direction: PatternDirection::ParallelToWidth,
        price: 0,
        quantity: None,
    });
    optimizer.add_stock_piece(StockPiece {
        width: 48,
        length: 120,
        pattern_direction: PatternDirection::ParallelToLength,
        price: 0,
        quantity: None,
    });

    let mut rng: StdRng = SeedableRng::seed_from_u64(1);

    let num_cut_pieces = 30;

    optimizer.add_cut_piece(CutPiece {
        quantity: num_cut_pieces,
        external_id: Some(1),
        width: rng.gen_range(1..=48),
        length: rng.gen_range(1..=120),
        pattern_direction: if rng.gen_bool(0.5) {
            PatternDirection::ParallelToWidth
        } else {
            PatternDirection::ParallelToLength
        },
        can_rotate: true,
    });

    let solution = optimizer
        .set_cut_width(1)
        .set_random_seed(1)
        .optimize_guillotine(|_| {})
        .unwrap();

    sanity_check_solution(&solution, num_cut_pieces);
}

#[test]
fn nested() {
    let solution = Optimizer::new()
        .add_stock_pieces(STOCK_PIECES.iter().cloned().collect::<Vec<_>>())
        .add_cut_pieces(CUT_PIECES.iter().cloned().collect::<Vec<_>>())
        .set_cut_width(1)
        .set_random_seed(1)
        .optimize_nested(|_| {})
        .unwrap();

    sanity_check_solution(&solution, CUT_PIECES.len());

    let stock_pieces = solution.stock_pieces;
    assert_eq!(stock_pieces.len(), 1);
    let cut_pieces = &stock_pieces[0].cut_pieces;
    assert_eq!(cut_pieces.len(), CUT_PIECES.len());
}

#[test]
fn nested_rotate() {
    let solution = Optimizer::new()
        .add_stock_piece(StockPiece {
            width: 10,
            length: 11,
            pattern_direction: PatternDirection::None,
            price: 0,
            quantity: None,
        })
        .add_cut_piece(CutPiece {
            quantity: 1,
            external_id: Some(1),
            width: 11,
            length: 10,
            pattern_direction: PatternDirection::None,
            can_rotate: true,
        })
        .set_cut_width(1)
        .set_random_seed(1)
        .optimize_nested(|_| {})
        .unwrap();

    sanity_check_solution(&solution, 1);

    let stock_pieces = solution.stock_pieces;
    assert_eq!(stock_pieces.len(), 1);
    let cut_pieces = &stock_pieces[0].cut_pieces;
    assert_eq!(cut_pieces.len(), 1);
    assert_eq!(
        cut_pieces[0],
        ResultCutPiece {
            external_id: Some(1),
            x: 0,
            y: 0,
            width: 10,
            length: 11,
            pattern_direction: PatternDirection::None,
            is_rotated: true,
        }
    );
}

#[test]
fn nested_rotate_pattern() {
    let solution = Optimizer::new()
        .add_stock_piece(StockPiece {
            width: 10,
            length: 11,
            pattern_direction: PatternDirection::ParallelToWidth,
            price: 0,
            quantity: None,
        })
        .add_cut_piece(CutPiece {
            quantity: 1,
            external_id: Some(1),
            width: 11,
            length: 10,
            pattern_direction: PatternDirection::ParallelToLength,
            can_rotate: true,
        })
        .set_cut_width(1)
        .set_random_seed(1)
        .optimize_nested(|_| {})
        .unwrap();

    sanity_check_solution(&solution, 1);

    let stock_pieces = solution.stock_pieces;
    assert_eq!(stock_pieces.len(), 1);
    let cut_pieces = &stock_pieces[0].cut_pieces;
    assert_eq!(cut_pieces.len(), 1);
    assert_eq!(
        cut_pieces[0],
        ResultCutPiece {
            external_id: Some(1),
            x: 0,
            y: 0,
            width: 10,
            length: 11,
            pattern_direction: PatternDirection::ParallelToWidth,
            is_rotated: true,
        }
    );
}

#[test]
fn nested_non_fitting_cut_piece_can_rotate() {
    let result = Optimizer::new()
        .add_stock_piece(StockPiece {
            width: 10,
            length: 10,
            pattern_direction: PatternDirection::None,
            price: 0,
            quantity: None,
        })
        .add_cut_piece(CutPiece {
            quantity: 1,
            external_id: Some(1),
            width: 11,
            length: 10,
            pattern_direction: PatternDirection::None,
            can_rotate: true,
        })
        .set_cut_width(1)
        .set_random_seed(1)
        .optimize_nested(|_| {});

    assert!(
        matches!(result, Err(Error::NoFitForCutPiece(_))),
        "should have returned Error::NoFitForCutPiece"
    )
}

#[test]
fn nested_non_fitting_cut_piece_no_rotate() {
    let result = Optimizer::new()
        .add_stock_piece(StockPiece {
            width: 10,
            length: 11,
            pattern_direction: PatternDirection::None,
            price: 0,
            quantity: None,
        })
        .add_cut_piece(CutPiece {
            quantity: 1,
            external_id: Some(1),
            width: 11,
            length: 10,
            pattern_direction: PatternDirection::None,
            can_rotate: false,
        })
        .set_cut_width(1)
        .set_random_seed(1)
        .optimize_nested(|_| {});

    assert!(
        matches!(result, Err(Error::NoFitForCutPiece(_))),
        "should have returned Error::NoFitForCutPiece"
    )
}

#[test]
fn nested_non_fitting_cut_piece_no_rotate_pattern() {
    let result = Optimizer::new()
        .add_stock_piece(StockPiece {
            width: 10,
            length: 11,
            pattern_direction: PatternDirection::ParallelToWidth,
            price: 0,
            quantity: None,
        })
        .add_cut_piece(CutPiece {
            quantity: 1,
            external_id: Some(1),
            width: 11,
            length: 10,
            pattern_direction: PatternDirection::ParallelToLength,
            can_rotate: false,
        })
        .set_cut_width(1)
        .set_random_seed(1)
        .optimize_nested(|_| {});

    assert!(
        matches!(result, Err(Error::NoFitForCutPiece(_))),
        "should have returned Error::NoFitForCutPiece"
    )
}

#[test]
fn nested_non_fitting_cut_piece_mismatched_pattern() {
    let result = Optimizer::new()
        .add_stock_piece(StockPiece {
            width: 100,
            length: 100,
            pattern_direction: PatternDirection::None,
            price: 0,
            quantity: None,
        })
        .add_cut_piece(CutPiece {
            quantity: 1,
            external_id: Some(1),
            width: 11,
            length: 10,
            pattern_direction: PatternDirection::ParallelToWidth,
            can_rotate: true,
        })
        .set_cut_width(1)
        .set_random_seed(1)
        .optimize_nested(|_| {});

    assert!(
        matches!(result, Err(Error::NoFitForCutPiece(_))),
        "should have returned Error::NoFitForCutPiece"
    )
}

#[test]
fn nested_no_allow_mixed_stock_sizes() {
    let solution = Optimizer::new()
        .add_stock_pieces(STOCK_PIECES.iter().cloned().collect::<Vec<_>>())
        .add_cut_piece(CutPiece {
            quantity: 1,
            external_id: Some(1),
            width: 48,
            length: 96,
            pattern_direction: PatternDirection::None,
            can_rotate: false,
        })
        .add_cut_piece(CutPiece {
            quantity: 1,
            external_id: Some(2),
            width: 48,
            length: 120,
            pattern_direction: PatternDirection::None,
            can_rotate: false,
        })
        .set_cut_width(1)
        .set_random_seed(1)
        .allow_mixed_stock_sizes(false)
        .optimize_nested(|_| {})
        .unwrap();

    sanity_check_solution(&solution, 2);

    assert_eq!(solution.stock_pieces.len(), 2);
    for stock_piece in solution.stock_pieces {
        // Since we aren't allowing mixed sizes,
        // all stock pieces will need to be 120 long.
        assert_eq!(stock_piece.length, 120)
    }
}

#[test]
fn nested_different_stock_piece_prices() {
    let solution = Optimizer::new()
        .add_stock_piece(StockPiece {
            width: 48,
            length: 96,
            pattern_direction: PatternDirection::None,
            price: 1,
            quantity: None,
        })
        .add_stock_piece(StockPiece {
            width: 48,
            length: 120,
            pattern_direction: PatternDirection::None,
            // Maker the 48x120 stock piece more expensive than (2) 48x96 pieces.
            price: 3,
            quantity: None,
        })
        .add_cut_piece(CutPiece {
            quantity: 2,
            external_id: Some(1),
            width: 48,
            length: 50,
            pattern_direction: PatternDirection::None,
            can_rotate: false,
        })
        .set_cut_width(1)
        .set_random_seed(1)
        .allow_mixed_stock_sizes(false)
        .optimize_nested(|_| {})
        .unwrap();

    sanity_check_solution(&solution, 2);

    // A single 48x120 stock piece could be used, but since we've set (2) 48x96 pieces to
    // be a lower price than (1) 48x120, it should use (2) 48x96 pieces instead.
    assert_eq!(solution.stock_pieces.len(), 2);
    for stock_piece in solution.stock_pieces {
        assert_eq!(stock_piece.length, 96)
    }
}

#[test]
fn nested_same_stock_piece_prices() {
    let solution = Optimizer::new()
        .add_stock_piece(StockPiece {
            width: 48,
            length: 96,
            pattern_direction: PatternDirection::None,
            price: 0,
            quantity: None,
        })
        .add_stock_piece(StockPiece {
            width: 48,
            length: 120,
            pattern_direction: PatternDirection::None,
            price: 0,
            quantity: None,
        })
        .add_cut_piece(CutPiece {
            quantity: 2,
            external_id: Some(1),
            width: 48,
            length: 50,
            pattern_direction: PatternDirection::None,
            can_rotate: false,
        })
        .set_cut_width(1)
        .set_random_seed(1)
        .allow_mixed_stock_sizes(false)
        .optimize_nested(|_| {})
        .unwrap();

    sanity_check_solution(&solution, 2);

    assert_eq!(solution.stock_pieces.len(), 1);
    assert_eq!(solution.stock_pieces[0].length, 120)
}

#[test]
fn nested_stock_quantity_too_low() {
    let result = Optimizer::new()
        .add_stock_piece(StockPiece {
            width: 48,
            length: 96,
            pattern_direction: PatternDirection::None,
            price: 0,
            quantity: Some(1),
        })
        .add_cut_piece(CutPiece {
            quantity: 2,
            external_id: None,
            width: 48,
            length: 96,
            pattern_direction: PatternDirection::None,
            can_rotate: false,
        })
        .set_cut_width(1)
        .set_random_seed(1)
        .optimize_nested(|_| {});

    assert!(
        result.is_err(),
        "should fail because stock quantity is too low"
    );
}

#[test]
fn nested_stock_quantity_1() {
    let solution = Optimizer::new()
        .add_stock_piece(StockPiece {
            width: 48,
            length: 96,
            pattern_direction: PatternDirection::None,
            price: 0,
            quantity: Some(1),
        })
        .add_cut_piece(CutPiece {
            quantity: 1,
            external_id: None,
            width: 10,
            length: 96,
            pattern_direction: PatternDirection::None,
            can_rotate: false,
        })
        .set_cut_width(1)
        .set_random_seed(1)
        .optimize_nested(|_| {})
        .unwrap();

    sanity_check_solution(&solution, 1);
}

#[test]
fn nested_stock_quantity_2() {
    let solution = Optimizer::new()
        .add_stock_piece(StockPiece {
            width: 48,
            length: 96,
            pattern_direction: PatternDirection::None,
            price: 0,
            quantity: Some(2),
        })
        .add_cut_piece(CutPiece {
            quantity: 2,
            external_id: None,
            width: 10,
            length: 96,
            pattern_direction: PatternDirection::None,
            can_rotate: false,
        })
        .set_cut_width(1)
        .set_random_seed(1)
        .optimize_nested(|_| {})
        .unwrap();

    sanity_check_solution(&solution, 2);
}

#[test]
fn nested_32_cut_pieces_on_1_stock_piece() {
    let mut optimizer = Optimizer::new();
    optimizer.add_stock_piece(StockPiece {
        width: 48,
        length: 96,
        pattern_direction: PatternDirection::None,
        price: 0,
        quantity: None,
    });

    let num_cut_pieces = 32;

    optimizer.add_cut_piece(CutPiece {
        quantity: num_cut_pieces,
        external_id: Some(1),
        width: 10,
        length: 10,
        pattern_direction: PatternDirection::None,
        can_rotate: false,
    });

    let solution = optimizer
        .set_cut_width(1)
        .set_random_seed(1)
        .optimize_nested(|_| {})
        .unwrap();

    sanity_check_solution(&solution, num_cut_pieces);

    let stock_pieces = solution.stock_pieces;
    assert_eq!(stock_pieces.len(), 1);
    let cut_pieces = &stock_pieces[0].cut_pieces;
    assert_eq!(cut_pieces.len(), 32);
}

#[test]
fn nested_32_cut_pieces_on_2_stock_piece_zero_cut_width() {
    let mut optimizer = Optimizer::new();
    optimizer.add_stock_piece(StockPiece {
        width: 48,
        length: 96,
        pattern_direction: PatternDirection::None,
        price: 0,
        quantity: None,
    });

    let num_cut_pieces = 32;

    optimizer.add_cut_piece(CutPiece {
        quantity: num_cut_pieces,
        external_id: Some(1),
        width: 12,
        length: 12,
        pattern_direction: PatternDirection::None,
        can_rotate: false,
    });

    let solution = optimizer
        .set_cut_width(0)
        .set_random_seed(1)
        .optimize_nested(|_| {})
        .unwrap();

    sanity_check_solution(&solution, num_cut_pieces);

    let stock_pieces = solution.stock_pieces;
    assert_eq!(stock_pieces.len(), 1);
    let cut_pieces = &stock_pieces[0].cut_pieces;
    assert_eq!(cut_pieces.len(), 32);
}

#[test]
fn nested_32_cut_pieces_on_2_stock_piece() {
    let mut optimizer = Optimizer::new();
    optimizer.add_stock_piece(StockPiece {
        width: 48,
        length: 96,
        pattern_direction: PatternDirection::None,
        price: 0,
        quantity: None,
    });

    let num_cut_pieces = 32;

    optimizer.add_cut_piece(CutPiece {
        quantity: num_cut_pieces,
        external_id: Some(1),
        width: 12,
        length: 12,
        pattern_direction: PatternDirection::None,
        can_rotate: false,
    });

    let solution = optimizer
        .set_cut_width(1)
        .set_random_seed(1)
        .optimize_nested(|_| {})
        .unwrap();

    sanity_check_solution(&solution, num_cut_pieces);

    let stock_pieces = solution.stock_pieces;
    assert_eq!(stock_pieces.len(), 2);
}

#[test]
fn nested_64_cut_pieces_on_2_stock_pieces() {
    let mut optimizer = Optimizer::new();
    optimizer.add_stock_piece(StockPiece {
        width: 48,
        length: 96,
        pattern_direction: PatternDirection::None,
        price: 0,
        quantity: None,
    });

    let num_cut_pieces = 64;

    optimizer.add_cut_piece(CutPiece {
        quantity: num_cut_pieces,
        external_id: Some(1),
        width: 10,
        length: 10,
        pattern_direction: PatternDirection::None,
        can_rotate: false,
    });

    let solution = optimizer
        .set_cut_width(1)
        .set_random_seed(1)
        .optimize_nested(|_| {})
        .unwrap();

    sanity_check_solution(&solution, num_cut_pieces);

    let stock_pieces = solution.stock_pieces;
    assert_eq!(stock_pieces.len(), 2);
    assert_eq!(stock_pieces[0].cut_pieces.len(), 32);
    assert_eq!(stock_pieces[1].cut_pieces.len(), 32);
}

#[test]
fn nested_random_cut_pieces() {
    let mut optimizer = Optimizer::new();
    optimizer.add_stock_piece(StockPiece {
        width: 48,
        length: 96,
        pattern_direction: PatternDirection::ParallelToWidth,
        price: 0,
        quantity: None,
    });
    optimizer.add_stock_piece(StockPiece {
        width: 48,
        length: 96,
        pattern_direction: PatternDirection::ParallelToLength,
        price: 0,
        quantity: None,
    });
    optimizer.add_stock_piece(StockPiece {
        width: 48,
        length: 120,
        pattern_direction: PatternDirection::ParallelToWidth,
        price: 0,
        quantity: None,
    });
    optimizer.add_stock_piece(StockPiece {
        width: 48,
        length: 120,
        pattern_direction: PatternDirection::ParallelToLength,
        price: 0,
        quantity: None,
    });

    let mut rng: StdRng = SeedableRng::seed_from_u64(1);

    let num_cut_pieces = 30;

    optimizer.add_cut_piece(CutPiece {
        quantity: num_cut_pieces,
        external_id: Some(1),
        width: rng.gen_range(1..=48),
        length: rng.gen_range(1..=120),
        pattern_direction: if rng.gen_bool(0.5) {
            PatternDirection::ParallelToWidth
        } else {
            PatternDirection::ParallelToLength
        },
        can_rotate: true,
    });

    let solution = optimizer
        .set_cut_width(1)
        .set_random_seed(1)
        .optimize_nested(|_| {})
        .unwrap();

    sanity_check_solution(&solution, num_cut_pieces);
}

#[test]
fn add_equivalent_stock_pieces_sums_quantities() {
    let mut optimizer = Optimizer::new();
    optimizer
        .add_stock_piece(StockPiece {
            width: 48,
            length: 96,
            pattern_direction: PatternDirection::None,
            price: 0,
            quantity: Some(3),
        })
        .add_stock_piece(StockPiece {
            width: 48,
            length: 96,
            pattern_direction: PatternDirection::None,
            price: 0,
            quantity: Some(6),
        });

    assert_eq!(optimizer.stock_pieces.len(), 1);
    assert_eq!(optimizer.stock_pieces[0].quantity, Some(9));
}

#[test]
fn add_equivalent_stock_pieces_with_none() {
    let mut optimizer = Optimizer::new();
    optimizer
        .add_stock_piece(StockPiece {
            width: 48,
            length: 96,
            pattern_direction: PatternDirection::None,
            price: 0,
            quantity: None,
        })
        .add_stock_piece(StockPiece {
            width: 48,
            length: 96,
            pattern_direction: PatternDirection::None,
            price: 0,
            quantity: Some(6),
        });

    assert_eq!(optimizer.stock_pieces.len(), 1);
    assert_eq!(optimizer.stock_pieces[0].quantity, None);
}

#[test]
fn stock_pieces_dec_quantity() {
    let mut stock_piece = StockPiece {
        width: 48,
        length: 96,
        pattern_direction: PatternDirection::None,
        price: 0,
        quantity: Some(10),
    };

    stock_piece.dec_quantity();

    assert_eq!(stock_piece.quantity, Some(9));

    stock_piece.quantity = None;
    stock_piece.dec_quantity();

    assert_eq!(stock_piece.quantity, None);
}

#[test]
fn guillotine_rotate_cut_pieces() {
    let mut optimizer = Optimizer::new();
    optimizer
        .add_stock_piece(StockPiece {
            width: 48,
            length: 96,
            pattern_direction: PatternDirection::None,
            price: 0,
            quantity: None,
        })
        .add_stock_piece(StockPiece {
            width: 48,
            length: 120,
            pattern_direction: PatternDirection::None,
            price: 0,
            quantity: None,
        })
        .set_cut_width(1)
        .set_random_seed(1)
        .allow_mixed_stock_sizes(false);

    optimizer.add_cut_piece(CutPiece {
        quantity: 16,
        external_id: Some(1),
        width: 18,
        length: 24,
        pattern_direction: PatternDirection::None,
        can_rotate: true,
    });

    let result = optimizer.optimize_guillotine(|_| {});

    assert!(result.is_ok());
    if let Ok(solution) = result {
        assert_eq!(solution.stock_pieces.len(), 2);
        assert_eq!(solution.stock_pieces[0].length, 96);
    }
}

#[test]
fn nested_rotate_cut_pieces() {
    let mut optimizer = Optimizer::new();
    optimizer
        .add_stock_piece(StockPiece {
            width: 48,
            length: 96,
            pattern_direction: PatternDirection::None,
            price: 0,
            quantity: None,
        })
        .add_stock_piece(StockPiece {
            width: 48,
            length: 120,
            pattern_direction: PatternDirection::None,
            price: 0,
            quantity: None,
        })
        .set_cut_width(1)
        .set_random_seed(1)
        .allow_mixed_stock_sizes(false);

    optimizer.add_cut_piece(CutPiece {
        quantity: 16,
        external_id: Some(1),
        width: 18,
        length: 24,
        pattern_direction: PatternDirection::None,
        can_rotate: true,
    });

    let result = optimizer.optimize_guillotine(|_| {});

    assert!(result.is_ok());
    if let Ok(solution) = result {
        assert_eq!(solution.stock_pieces.len(), 2);
        assert_eq!(solution.stock_pieces[0].length, 96);
    }
}

#[test]
fn pighetti_github_issue_12() {
    let mut optimizer = Optimizer::new();

    let plywood = StockPiece {
        quantity: Some(1),
        length: 2440,
        width: 1220,
        pattern_direction: PatternDirection::ParallelToLength,
        price: 130,
    };

    let cut_piece_a = CutPiece {
        quantity: 12,
        external_id: Some(1),
        length: 775,
        width: 150,
        can_rotate: false,
        pattern_direction: PatternDirection::ParallelToLength,
    };

    let cut_piece_b = CutPiece {
        quantity: 25,
        external_id: Some(1),
        length: 450,
        width: 100,
        can_rotate: false,
        pattern_direction: PatternDirection::ParallelToLength,
    };

    optimizer.add_stock_piece(plywood);
    optimizer.add_cut_piece(cut_piece_a);
    optimizer.add_cut_piece(cut_piece_b);
    optimizer.set_cut_width(2);
    optimizer.set_random_seed(1);

    let result = optimizer.optimize_guillotine(|_| {});

    assert!(result.is_ok());
    if let Ok(solution) = result {
        assert_eq!(solution.stock_pieces.len(), 1);
        sanity_check_solution(&solution, 37);
    }
}

#[test]
fn pighetti_github_issue_16() {
    let plywood = StockPiece {
        quantity: Some(2),
        length: 2440,
        width: 1220,
        pattern_direction: PatternDirection::ParallelToLength,
        price: 130,
    };

    let cut_piece_a = CutPiece {
        quantity: 6,
        external_id: Some(1),
        length: 814,
        width: 465,
        can_rotate: false,
        pattern_direction: PatternDirection::ParallelToLength,
    };

    let mut optimizer = Optimizer::new();
    optimizer.add_stock_piece(plywood);
    optimizer.add_cut_piece(cut_piece_a);
    optimizer.set_cut_width(2);
    optimizer.set_random_seed(1);

    let result = optimizer.optimize_guillotine(|_| {});

    assert!(result.is_ok());
    if let Ok(solution) = result {
        assert_eq!(solution.stock_pieces.len(), 2);
        sanity_check_solution(&solution, 6);
    }
}

#[test]
fn deterministic_solutions() {
    // Run the same optimization multiple times with the same random seed and
    // check if the solution is the same each time.
    let solutions: Vec<Solution> = (0..10)
        .map(|_| {
            let plywood = StockPiece {
                quantity: Some(2),
                length: 2440,
                width: 1220,
                pattern_direction: PatternDirection::ParallelToLength,
                price: 130,
            };

            let cut_piece_a = CutPiece {
                quantity: 6,
                external_id: Some(1),
                length: 814,
                width: 465,
                can_rotate: false,
                pattern_direction: PatternDirection::ParallelToLength,
            };

            let mut optimizer = Optimizer::new();
            optimizer.add_stock_piece(plywood);
            optimizer.add_cut_piece(cut_piece_a);
            optimizer.set_cut_width(2);
            optimizer.set_random_seed(1);

            optimizer.optimize_guillotine(|_| {}).unwrap()
        })
        .collect();

    solutions.windows(2).for_each(|window| {
        let solution1 = &window[0];
        let solution2 = &window[1];
        assert_eq!(solution1.fitness, solution2.fitness);
        assert_eq!(solution1.price, solution2.price);
        solution1
            .stock_pieces
            .iter()
            .zip(solution2.stock_pieces.iter())
            .for_each(|(stock_piece1, stock_piece2)| {
                assert_eq!(stock_piece1.width, stock_piece2.width);
                assert_eq!(stock_piece1.length, stock_piece2.length);
                assert_eq!(
                    stock_piece1.pattern_direction,
                    stock_piece2.pattern_direction
                );
                assert_eq!(stock_piece1.price, stock_piece2.price);
                stock_piece1
                    .cut_pieces
                    .iter()
                    .zip(stock_piece2.cut_pieces.iter())
                    .for_each(|(cut_piece1, cut_piece2)| {
                        assert_eq!(cut_piece1, cut_piece2);
                    });
            })
    });
}

#[test]
fn guillotine_top_k_returns_ranked_candidates() {
    let top = Optimizer::new()
        .add_stock_pieces(STOCK_PIECES.iter().cloned().collect::<Vec<_>>())
        .add_cut_pieces(CUT_PIECES.iter().cloned().collect::<Vec<_>>())
        .set_cut_width(1)
        .set_random_seed(1)
        .optimize_guillotine_top_k(3, |_| {})
        .unwrap();

    assert!(!top.solutions.is_empty());
    assert!(top.solutions.len() <= 3);
    for solution in &top.solutions {
        sanity_check_solution(solution, CUT_PIECES.len());
    }
}

#[test]
fn guillotine_top_k_first_matches_best_solution() {
    let best = Optimizer::new()
        .add_stock_pieces(STOCK_PIECES.iter().cloned().collect::<Vec<_>>())
        .add_cut_pieces(CUT_PIECES.iter().cloned().collect::<Vec<_>>())
        .set_cut_width(1)
        .set_random_seed(1)
        .optimize_guillotine(|_| {})
        .unwrap();

    let top = Optimizer::new()
        .add_stock_pieces(STOCK_PIECES.iter().cloned().collect::<Vec<_>>())
        .add_cut_pieces(CUT_PIECES.iter().cloned().collect::<Vec<_>>())
        .set_cut_width(1)
        .set_random_seed(1)
        .optimize_guillotine_top_k(5, |_| {})
        .unwrap();

    assert!(!top.solutions.is_empty());
    let first = &top.solutions[0];
    assert_eq!(best.fitness, first.fitness);
    assert_eq!(best.price, first.price);
    assert_eq!(best.stock_pieces.len(), first.stock_pieces.len());
    assert_eq!(
        best.stock_pieces
            .iter()
            .map(|s| s.cut_pieces.len())
            .sum::<usize>(),
        first
            .stock_pieces
            .iter()
            .map(|s| s.cut_pieces.len())
            .sum::<usize>()
    );
}
