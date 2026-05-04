//! End-to-end benchmarks for the lotus-core engine.
//!
//! Run with: `cargo bench -p lotus-core`
//! Filter:   `cargo bench -p lotus-core -- sheet_load`
//!
//! Groups:
//!   lexer              — tokenize-only micro-benchmarks
//!   parser             — lex + parse into AST
//!   evaluator          — one-shot Evaluator::evaluate() with a prebuilt resolver
//!   extract_refs       — formula reference extraction (used by editor highlighting)
//!   sheet_load_literal — bulk-insert N literal cells (no formulas)
//!   sheet_load_formula — bulk-insert N formulas (each references prior cell)
//!   sheet_range_col    — SUM(A:A) scaling in column length
//!   sheet_wide_formula — one huge formula referencing N cells
//!   sheet_chain        — deep dependency chain (A1 → A2 → ... → AN)
//!   sheet_grid         — NxN grid where each cell = left_neighbor + 1
//!   sheet_update       — incremental single-cell update on a loaded sheet
//!   sheet_circular     — circular-dependency detection overhead
//!   sheet_spill        — A1 spills NxN, with K downstream consumers of A1#

use std::collections::HashMap;
use std::time::Duration;

use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use lotus_core::lexer::Lexer;
use lotus_core::parser::Parser;
use lotus_core::types::{cell_id, index_to_col, CellId, CellValue};
use lotus_core::{extract_refs, Evaluator, Sheet};

// -----------------------------------------------------------------------------
// Helpers
// -----------------------------------------------------------------------------

/// Build a literal-cell load: A1..A{n} = 1..n.
fn literal_load(n: u32) -> Vec<(CellId, String)> {
    (1..=n)
        .map(|i| (cell_id("A", i), i.to_string()))
        .collect()
}

/// Build a formula chain: A1=1, A2=A1+1, A3=A2+1, ...
fn chain_load(n: u32) -> Vec<(CellId, String)> {
    let mut out = Vec::with_capacity(n as usize);
    out.push((cell_id("A", 1), "1".to_string()));
    for i in 2..=n {
        out.push((cell_id("A", i), format!("=A{}+1", i - 1)));
    }
    out
}

/// One wide formula referencing `=A1+A2+...+An` (plus loads the literals).
fn wide_formula_load(n: u32) -> Vec<(CellId, String)> {
    let mut out: Vec<(CellId, String)> = (1..=n)
        .map(|i| (cell_id("A", i), i.to_string()))
        .collect();
    let sum_expr = (1..=n)
        .map(|i| format!("A{i}"))
        .collect::<Vec<_>>()
        .join("+");
    out.push((cell_id("B", 1), format!("={sum_expr}")));
    out
}

/// NxN grid: row 1 = literals, every other cell = cell-to-left + 1.
fn grid_load(n: u32) -> Vec<(CellId, String)> {
    let mut out = Vec::with_capacity((n * n) as usize);
    for col_idx in 1..=n {
        let col = index_to_col(col_idx);
        for row in 1..=n {
            if col_idx == 1 {
                out.push((cell_id(&col, row), row.to_string()));
            } else {
                let left = index_to_col(col_idx - 1);
                out.push((cell_id(&col, row), format!("={left}{row}+1")));
            }
        }
    }
    out
}

// -----------------------------------------------------------------------------
// Lexer / parser micro-benchmarks
// -----------------------------------------------------------------------------

fn lexer_cases() -> Vec<(&'static str, String)> {
    let long_add = format!(
        "={}",
        (1..=50).map(|i| format!("A{i}")).collect::<Vec<_>>().join("+")
    );
    let many_args = format!(
        "=SUM({})",
        (1..=100).map(|i| format!("A{i}")).collect::<Vec<_>>().join(",")
    );
    let nested_fns = (0..20).fold("A1".to_string(), |acc, _| format!("SUM({acc},1)"));
    let nested = format!("={nested_fns}");
    vec![
        ("simple", "=A1+B1".into()),
        ("range_sum", "=SUM(A1:Z1000)".into()),
        ("long_add_50", long_add),
        ("many_args_100", many_args),
        ("nested_fn_20", nested),
    ]
}

fn bench_lexer(c: &mut Criterion) {
    let mut g = c.benchmark_group("lexer");
    for (name, input) in lexer_cases() {
        g.throughput(Throughput::Bytes(input.len() as u64));
        g.bench_with_input(BenchmarkId::from_parameter(name), &input, |b, input| {
            b.iter(|| {
                let mut lex = Lexer::new(input);
                black_box(lex.tokenize().unwrap());
            });
        });
    }
    g.finish();
}

fn bench_parser(c: &mut Criterion) {
    let mut g = c.benchmark_group("parser");
    for (name, input) in lexer_cases() {
        g.throughput(Throughput::Bytes(input.len() as u64));
        g.bench_with_input(BenchmarkId::from_parameter(name), &input, |b, input| {
            b.iter(|| {
                black_box(Parser::parse(input).unwrap());
            });
        });
    }
    g.finish();
}

// -----------------------------------------------------------------------------
// Evaluator (standalone, no Sheet)
// -----------------------------------------------------------------------------

fn bench_evaluator(c: &mut Criterion) {
    let mut g = c.benchmark_group("evaluator");

    // Pure arithmetic — exercises binary op folding.
    let arith: String = format!(
        "={}",
        (1..=100).map(|i| i.to_string()).collect::<Vec<_>>().join("+")
    );
    g.bench_function("arith_sum_100", |b| {
        let eval = Evaluator::new(|_| CellValue::Empty);
        b.iter(|| black_box(eval.evaluate(&arith).unwrap()));
    });

    // SUM over a populated range — exercises range expansion + resolver.
    for n in [100u32, 1_000, 10_000] {
        let cells: HashMap<String, CellValue> = (1..=n)
            .map(|i| (format!("A{i}"), CellValue::Number(i as f64)))
            .collect();
        let formula = format!("=SUM(A1:A{n})");
        g.throughput(Throughput::Elements(n as u64));
        g.bench_with_input(
            BenchmarkId::new("sum_range", n),
            &(cells, formula),
            |b, (cells, formula)| {
                let cells = cells.clone();
                let eval = Evaluator::with_bounds(
                    move |id: &str| cells.get(id).cloned().unwrap_or(CellValue::Empty),
                    n,
                    26,
                );
                b.iter(|| black_box(eval.evaluate(formula).unwrap()));
            },
        );
    }

    g.finish();
}

fn bench_extract_refs(c: &mut Criterion) {
    let mut g = c.benchmark_group("extract_refs");

    let long_add = format!(
        "={}",
        (1..=100).map(|i| format!("A{i}")).collect::<Vec<_>>().join("+")
    );
    let range_heavy = "=SUM(A1:Z100)+AVERAGE(B1:B1000)+MAX(C1:C500)".to_string();

    for (name, input) in [
        ("simple", "=A1+B1".to_string()),
        ("long_add_100", long_add),
        ("range_heavy", range_heavy),
    ] {
        g.throughput(Throughput::Bytes(input.len() as u64));
        g.bench_with_input(BenchmarkId::from_parameter(name), &input, |b, input| {
            b.iter(|| black_box(extract_refs(input)));
        });
    }

    g.finish();
}

// -----------------------------------------------------------------------------
// Sheet bulk-load benchmarks
// -----------------------------------------------------------------------------

fn bench_sheet_load_literal(c: &mut Criterion) {
    let mut g = c.benchmark_group("sheet_load_literal");
    // Keep the large case's sample count low so runs finish in reasonable time.
    g.measurement_time(Duration::from_secs(8));

    for n in [100u32, 1_000, 10_000] {
        let changes = literal_load(n);
        g.throughput(Throughput::Elements(n as u64));
        g.bench_with_input(BenchmarkId::from_parameter(n), &changes, |b, changes| {
            b.iter(|| {
                let mut s = Sheet::new();
                s.set_cells(black_box(changes)).unwrap();
                black_box(s);
            });
        });
    }
    // Really large case — fewer samples to keep run-time bounded.
    let n = 50_000u32;
    let changes = literal_load(n);
    g.throughput(Throughput::Elements(n as u64));
    g.sample_size(10);
    g.bench_with_input(BenchmarkId::from_parameter(n), &changes, |b, changes| {
        b.iter(|| {
            let mut s = Sheet::new();
            s.set_cells(black_box(changes)).unwrap();
            black_box(s);
        });
    });

    g.finish();
}

fn bench_sheet_load_formula(c: &mut Criterion) {
    let mut g = c.benchmark_group("sheet_load_formula");
    g.measurement_time(Duration::from_secs(8));

    for n in [100u32, 1_000, 10_000] {
        let changes = chain_load(n);
        g.throughput(Throughput::Elements(n as u64));
        g.bench_with_input(BenchmarkId::from_parameter(n), &changes, |b, changes| {
            b.iter(|| {
                let mut s = Sheet::new();
                s.set_cells(black_box(changes)).unwrap();
                black_box(s);
            });
        });
    }

    g.finish();
}

// -----------------------------------------------------------------------------
// SUM(A:A) — how does whole-column range scale?
// -----------------------------------------------------------------------------

fn bench_sheet_range_col(c: &mut Criterion) {
    let mut g = c.benchmark_group("sheet_range_col");
    g.measurement_time(Duration::from_secs(8));

    for n in [100u32, 1_000, 10_000] {
        let mut changes = literal_load(n);
        changes.push((cell_id("B", 1), "=SUM(A:A)".into()));
        g.throughput(Throughput::Elements(n as u64));
        g.bench_with_input(BenchmarkId::from_parameter(n), &changes, |b, changes| {
            b.iter(|| {
                let mut s = Sheet::new();
                s.set_cells(black_box(changes)).unwrap();
                black_box(s.get("B1"));
            });
        });
    }

    g.finish();
}

// -----------------------------------------------------------------------------
// Wide formula — `=A1+A2+...+AN`
// -----------------------------------------------------------------------------

fn bench_sheet_wide_formula(c: &mut Criterion) {
    let mut g = c.benchmark_group("sheet_wide_formula");
    g.measurement_time(Duration::from_secs(8));

    for n in [50u32, 200, 1_000] {
        let changes = wide_formula_load(n);
        g.throughput(Throughput::Elements(n as u64));
        g.bench_with_input(BenchmarkId::from_parameter(n), &changes, |b, changes| {
            b.iter(|| {
                let mut s = Sheet::new();
                s.set_cells(black_box(changes)).unwrap();
                black_box(s.get("B1"));
            });
        });
    }

    g.finish();
}

// -----------------------------------------------------------------------------
// Deep dependency chain
// -----------------------------------------------------------------------------

fn bench_sheet_chain(c: &mut Criterion) {
    let mut g = c.benchmark_group("sheet_chain");
    g.measurement_time(Duration::from_secs(8));

    for n in [100u32, 1_000, 5_000] {
        let changes = chain_load(n);
        g.throughput(Throughput::Elements(n as u64));
        g.bench_with_input(BenchmarkId::from_parameter(n), &changes, |b, changes| {
            b.iter(|| {
                let mut s = Sheet::new();
                s.set_cells(black_box(changes)).unwrap();
                black_box(s);
            });
        });
    }

    g.finish();
}

// -----------------------------------------------------------------------------
// NxN grid of formulas (each cell = left + 1)
// -----------------------------------------------------------------------------

fn bench_sheet_grid(c: &mut Criterion) {
    let mut g = c.benchmark_group("sheet_grid");
    g.measurement_time(Duration::from_secs(10));
    g.sample_size(10);

    for n in [20u32, 50, 100] {
        let changes = grid_load(n);
        g.throughput(Throughput::Elements((n * n) as u64));
        g.bench_with_input(
            BenchmarkId::from_parameter(format!("{n}x{n}")),
            &changes,
            |b, changes| {
                b.iter(|| {
                    let mut s = Sheet::new();
                    s.set_cells(black_box(changes)).unwrap();
                    black_box(s);
                });
            },
        );
    }

    g.finish();
}

// -----------------------------------------------------------------------------
// Incremental update on a loaded sheet
// -----------------------------------------------------------------------------

fn bench_sheet_update(c: &mut Criterion) {
    let mut g = c.benchmark_group("sheet_update");
    g.measurement_time(Duration::from_secs(8));

    for n in [100u32, 1_000, 10_000] {
        let base = chain_load(n);
        g.throughput(Throughput::Elements(1));
        g.bench_with_input(BenchmarkId::from_parameter(n), &base, |b, base| {
            b.iter_batched(
                || {
                    let mut s = Sheet::new();
                    s.set_cells(base).unwrap();
                    s
                },
                |mut s| {
                    // Modify the root — every downstream cell must recompute.
                    s.set_cells(&[(cell_id("A", 1), "42".into())]).unwrap();
                    black_box(s);
                },
                criterion::BatchSize::SmallInput,
            );
        });
    }

    g.finish();
}

// -----------------------------------------------------------------------------
// Circular-dependency detection overhead
// -----------------------------------------------------------------------------

fn bench_sheet_circular(c: &mut Criterion) {
    let mut g = c.benchmark_group("sheet_circular");

    // Cycle through N cells: A1=A2, A2=A3, ..., AN=A1.
    for n in [10u32, 100, 1_000] {
        let mut changes: Vec<(CellId, String)> = Vec::new();
        for i in 1..n {
            changes.push((cell_id("A", i), format!("=A{}", i + 1)));
        }
        changes.push((cell_id("A", n), "=A1".into()));

        g.throughput(Throughput::Elements(n as u64));
        g.bench_with_input(BenchmarkId::from_parameter(n), &changes, |b, changes| {
            b.iter(|| {
                let mut s = Sheet::new();
                let err = s.set_cells(black_box(changes)).unwrap_err();
                black_box(err);
            });
        });
    }

    g.finish();
}

// -----------------------------------------------------------------------------
// Spill-reference scaling
// -----------------------------------------------------------------------------
//
// Targets the spill_arrays.clone() inside the per-formula-cell resolver in
// dag.rs. Two axes:
//
//   spill_size   — A1 = SEQUENCE(n, n) → spills an n×n array
//   consumers    — number of downstream cells that reference A1#
//
// The resolver clones the entire spill_arrays HashMap into its closure for
// every formula cell, so cost scales with (#anchors * #consumers). The
// "many_anchors" variant stresses that product.

fn bench_sheet_spill(c: &mut Criterion) {
    let mut g = c.benchmark_group("sheet_spill");
    g.measurement_time(Duration::from_secs(8));

    // 1) One big spill, growing number of consumers.
    //    A1 = SEQUENCE(20, 20); B1..B{k} = =SUM(A1#).
    let spill_dim = 20u32;
    for k in [1u32, 10, 100, 500] {
        let mut changes: Vec<(CellId, String)> =
            vec![(cell_id("A", 1), format!("=SEQUENCE({spill_dim}, {spill_dim})"))];
        for i in 1..=k {
            changes.push((cell_id("B", i), "=SUM(A1#)".into()));
        }
        g.throughput(Throughput::Elements(k as u64));
        g.bench_with_input(
            BenchmarkId::new("one_anchor_consumers", k),
            &changes,
            |b, changes| {
                b.iter(|| {
                    let mut s = Sheet::new();
                    s.set_cells(black_box(changes)).unwrap();
                    black_box(s);
                });
            },
        );
    }

    // 2) Many anchors, each with one consumer.
    //    A{i} = SEQUENCE(5, 5) for i in 1..=k (rows 1, 7, 13, ... so spills don't collide);
    //    B{anchor_row} = SUM(A{anchor_row}#).
    //    Stresses spill_arrays growth — every consumer's resolver clones the
    //    full anchor map.
    for k in [1u32, 10, 50, 100] {
        let mut changes: Vec<(CellId, String)> = Vec::new();
        let stride = 6u32; // 5x5 spill + 1 row gap
        for i in 0..k {
            let anchor_row = 1 + i * stride;
            changes.push((cell_id("A", anchor_row), "=SEQUENCE(5, 5)".into()));
            changes.push((cell_id("B", anchor_row), format!("=SUM(A{anchor_row}#)")));
        }
        g.throughput(Throughput::Elements(k as u64));
        g.bench_with_input(
            BenchmarkId::new("many_anchors", k),
            &changes,
            |b, changes| {
                b.iter(|| {
                    let mut s = Sheet::new();
                    s.set_cells(black_box(changes)).unwrap();
                    black_box(s);
                });
            },
        );
    }

    g.finish();
}

criterion_group!(
    benches,
    bench_lexer,
    bench_parser,
    bench_evaluator,
    bench_extract_refs,
    bench_sheet_load_literal,
    bench_sheet_load_formula,
    bench_sheet_range_col,
    bench_sheet_wide_formula,
    bench_sheet_chain,
    bench_sheet_grid,
    bench_sheet_update,
    bench_sheet_circular,
    bench_sheet_spill,
);
criterion_main!(benches);
