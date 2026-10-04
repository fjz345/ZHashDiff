//! Per-stage timings of the diff pipeline (lex, line, token, ir, rows) on large fixtures.
//! Run with `cargo bench -p zdiff --bench pipeline`. See the zdiff README.

use std::{
    hint::black_box,
    path::Path,
    sync::{Arc, atomic::AtomicBool},
    time::{Duration, Instant},
};

use zdiff::{
    cached_file::FileMetadata,
    diff_builder::{DiffBuilderOptions, build_diff_rows},
    diff_ir::DiffIR,
    ignore::IgnoreMask,
    lexer::{LexerGreedy, RawToken},
    myers::{MyersDiffAlgorithm, line_diff, myers_count_add_deletes, token_diff},
    read_file_contents,
};

const WARMUP: usize = 1;
const ITERATIONS: usize = 7;
const STAGES: [&str; 5] = ["lex", "line", "token", "ir", "rows"];

const LARGE_LINES: usize = 20_000;
const DIFFERENT_LINES: usize = 2_000;

/// Pairs under the gitignored `test/` dir; a pair is skipped when either file is missing.
const LOCAL_PAIRS: &[(&str, &str, &str)] = &[
    (
        "local/imgui_1.91.1_vs_imgui",
        "rust_files_diff_1/imgui.1.91.1.h",
        "rust_files_diff_1/imgui.h",
    ),
    (
        "local/imgui_vs_imgui2",
        "rust_files_diff_1/imgui.h",
        "rust_files_diff_1/imgui2.h",
    ),
    (
        "local/extreme_size_log",
        "test_log/extreme_size_1.txt",
        "test_log/extreme_size_2.txt",
    ),
];

struct Fixture {
    name: String,
    source: String,
    target: String,
}

struct Run {
    stages: [Duration; STAGES.len()],
    tokens: (usize, usize),
    adds_deletes: (u32, u32),
    num_rows: usize,
}

fn main() {
    // cargo bench passes --bench; cargo test --benches/--all-targets doesn't, so skip there.
    if !std::env::args().any(|arg| arg == "--bench") {
        println!("pipeline benchmark skipped: run `cargo bench -p zdiff --bench pipeline`");
        return;
    }

    println!(
        "zdiff pipeline: median of {ITERATIONS} iterations after {WARMUP} warm-up, times in ms"
    );
    print!(
        "{:<28} {:>13} {:>15} {:>13} {:>8}",
        "fixture", "lines L/R", "tokens L/R", "-/+ tokens", "n rows"
    );
    for stage in STAGES.iter().chain(&["total"]) {
        print!(" {stage:>9}");
    }
    println!();

    for fixture in generated_fixtures().into_iter().chain(local_fixtures()) {
        let runs: Vec<Run> = (0..WARMUP + ITERATIONS)
            .map(|_| run_once(&fixture.source, &fixture.target))
            .skip(WARMUP)
            .collect();
        let stages: [Duration; STAGES.len()] =
            std::array::from_fn(|i| median(runs.iter().map(|run| run.stages[i])));
        // Median of per-iteration totals, not the sum of stage medians.
        let total = median(runs.iter().map(|run| run.stages.iter().sum()));

        let last = runs.last().expect("ITERATIONS > 0");
        let lines = (
            FileMetadata::new(&fixture.source).num_lines(),
            FileMetadata::new(&fixture.target).num_lines(),
        );
        let (adds, deletes) = last.adds_deletes;
        print!(
            "{:<28} {:>13} {:>15} {:>13} {:>8}",
            fixture.name,
            format!("{}/{}", lines.0, lines.1),
            format!("{}/{}", last.tokens.0, last.tokens.1),
            format!("{deletes}/{adds}"),
            last.num_rows,
        );
        for stage in stages.iter().chain([&total]) {
            print!(" {:>9.2}", stage.as_secs_f64() * 1000.0);
        }
        println!();
    }
}

/// Same calls and settings as the zdiff-gui diff context: greedy lexer (the default mode),
/// Linear Myers (the default algorithm), default row options. Rows is `build_diff_rows` only;
/// the GUI's IR clone and row finalization are not part of it.
fn run_once(source: &str, target: &str) -> Run {
    let cancel = Arc::new(AtomicBool::new(false));
    let num_lines = FileMetadata::new(source)
        .num_lines()
        .max(FileMetadata::new(target).num_lines());

    let start = Instant::now();
    let tokens_source: Vec<RawToken> = LexerGreedy::new(black_box(source)).parse();
    let tokens_target: Vec<RawToken> = LexerGreedy::new(black_box(target)).parse();
    let lex = start.elapsed();

    // Same comparison as zdiff-gui's compare_tokens.
    let cmp = |a: &RawToken, b: &RawToken| {
        a.kind == b.kind && source.as_bytes()[a.span.clone()] == target.as_bytes()[b.span.clone()]
    };
    // The two phases of myers_diff_path, timed separately.
    let start = Instant::now();
    let hunks = line_diff(
        MyersDiffAlgorithm::Linear,
        &tokens_source,
        &tokens_target,
        &cmp,
        &IgnoreMask::default(),
        cancel.clone(),
    )
    .expect("never cancelled");
    let line = start.elapsed();

    let start = Instant::now();
    let path = token_diff(
        MyersDiffAlgorithm::Linear,
        &tokens_source,
        &tokens_target,
        &hunks,
        &cmp,
        &IgnoreMask::default(),
        cancel.clone(),
    )
    .expect("never cancelled");
    let token = start.elapsed();

    let start = Instant::now();
    let diff_ir = DiffIR::new(black_box(&path), true, cancel).expect("never cancelled");
    let ir = start.elapsed();

    let start = Instant::now();
    let diff_rows = build_diff_rows(
        diff_ir,
        Some(&tokens_source),
        Some(&tokens_target),
        source,
        target,
        &DiffBuilderOptions::default(),
        num_lines,
    );
    let rows = start.elapsed();
    let num_rows = black_box(&diff_rows).len();

    Run {
        stages: [lex, line, token, ir, rows],
        tokens: (tokens_source.len(), tokens_target.len()),
        adds_deletes: myers_count_add_deletes(&path),
        num_rows,
    }
}

fn median(durations: impl Iterator<Item = Duration>) -> Duration {
    let mut sorted: Vec<Duration> = durations.collect();
    sorted.sort();
    sorted[sorted.len() / 2]
}

fn generated_fixtures() -> Vec<Fixture> {
    let large = generate_lines(1, LARGE_LINES);
    let scattered = scattered_edits(&large, 2);
    vec![
        Fixture {
            name: "generated/scattered_20k".into(),
            source: join_lines(&large),
            target: join_lines(&scattered),
        },
        Fixture {
            name: "generated/identical_20k".into(),
            source: join_lines(&large),
            target: join_lines(&large),
        },
        Fixture {
            name: "generated/different_2k".into(),
            source: join_lines(&generate_lines(3, DIFFERENT_LINES)),
            target: join_lines(&generate_lines(4, DIFFERENT_LINES)),
        },
    ]
}

fn local_fixtures() -> Vec<Fixture> {
    let test_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../test");
    LOCAL_PAIRS
        .iter()
        .filter_map(|&(name, source, target)| {
            let (source, target) = (test_dir.join(source), test_dir.join(target));
            if !source.exists() || !target.exists() {
                return None;
            }
            let read = |path: &Path| {
                read_file_contents(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
            };
            Some(Fixture {
                name: name.into(),
                source: read(&source),
                target: read(&target),
            })
        })
        .collect()
}

/// xorshift64*: deterministic fixtures without a rand dependency. The seed must be non-zero.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }

    fn pick<'a>(&mut self, items: &[&'a str]) -> &'a str {
        items[self.below(items.len())]
    }
}

const IDENTS: &[&str] = &[
    "value", "count", "buffer", "index", "result", "node", "offset", "length", "state", "config",
    "token", "cursor",
];
const TYPES: &[&str] = &["int", "float", "u32", "size_t", "bool", "char"];
const OPS: &[&str] = &["+", "-", "*", "/", "&", "|", "^"];

/// C-like statements with numbered identifiers so most lines are distinct.
fn code_line(rng: &mut Rng) -> String {
    let indent = "    ".repeat(1 + rng.below(3));
    let a = format!("{}_{}", rng.pick(IDENTS), rng.below(1000));
    let b = format!("{}_{}", rng.pick(IDENTS), rng.below(1000));
    match rng.below(8) {
        0 => format!(
            "{indent}{} {a} = {b} {} {};",
            rng.pick(TYPES),
            rng.pick(OPS),
            rng.below(4096)
        ),
        1 => format!("{indent}if ({a} > {b}) {{ return {}; }}", rng.below(100)),
        2 => format!(
            "{indent}{a} = compute_{}({b}, \"{}\");",
            rng.pick(IDENTS),
            rng.pick(IDENTS)
        ),
        3 => format!("{indent}// Update {a} from {b} before the next pass."),
        4 => format!(
            "{indent}for (int i = 0; i < {a}; ++i) {{ {b}[i] = {}; }}",
            rng.below(256)
        ),
        5 => String::new(),
        6 => format!("{indent}{a}.{}({b}, {});", rng.pick(IDENTS), rng.below(64)),
        _ => format!("{indent}{a} {}= {b};", rng.pick(OPS)),
    }
}

fn generate_lines(seed: u64, num_lines: usize) -> Vec<String> {
    let mut rng = Rng(seed);
    (0..num_lines).map(|_| code_line(&mut rng)).collect()
}

/// About 2% each of deleted, inserted-after and modified lines, spread over the whole file.
fn scattered_edits(lines: &[String], seed: u64) -> Vec<String> {
    let mut rng = Rng(seed);
    let mut out = Vec::with_capacity(lines.len());
    for line in lines {
        match rng.below(100) {
            0 | 1 => {}
            2 | 3 => {
                out.push(line.clone());
                out.push(code_line(&mut rng));
            }
            4 | 5 => out.push(code_line(&mut rng)),
            _ => out.push(line.clone()),
        }
    }
    out
}

fn join_lines(lines: &[String]) -> String {
    let mut text = lines.join("\n");
    text.push('\n');
    text
}
