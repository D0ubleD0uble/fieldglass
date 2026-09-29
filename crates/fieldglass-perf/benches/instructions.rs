//! Instruction counts per scenario, under Callgrind (the gated work tier).
//!
//!     FIELDGLASS_PERF_CORPUS=<dir> cargo bench --bench instructions -- --save-summary=json
//!
//! Needs Valgrind and a `gungraun-runner` of the same version as the `gungraun`
//! dev-dependency; `run.sh` checks both before it starts and fails when either
//! is missing, rather than letting the tier skip.
//!
//! Every scenario in the catalogue is one benchmark. Gungraun runs `prepare` as
//! setup, outside the instrumented function, and the benchmark hands the
//! prepared scenario back so its output is dropped outside it too: the count is
//! the operation's and nothing else's.

use std::hint::black_box;

use fieldglass_perf::{Corpus, Prepared, Via, catalogue};
use gungraun::Callgrind;
use gungraun::prelude::*;

/// Scenario ids in catalogue order. Gungraun names each benchmark by its
/// position, so `tools/check_perf_gate.py` maps positions back through this
/// same list, which `run.sh` writes out with `--list`.
fn scenario_ids() -> Vec<String> {
    let corpus = Corpus::from_env().expect("the corpus named by FIELDGLASS_PERF_CORPUS");
    catalogue(&corpus).into_iter().map(|s| s.id).collect()
}

fn prepare(id: String) -> Prepared {
    let corpus = Corpus::from_env().expect("the corpus named by FIELDGLASS_PERF_CORPUS");
    let scenario = catalogue(&corpus)
        .into_iter()
        .find(|s| s.id == id)
        .expect("an id from scenario_ids");
    Prepared::new(&corpus, &scenario, Via::Memory)
}

#[library_benchmark]
#[benches::scenario(iter = scenario_ids(), setup = prepare)]
fn run(mut prepared: Prepared) -> Prepared {
    black_box(prepared.execute());
    prepared
}

library_benchmark_group!(name = scenarios, benchmarks = [run]);

/// Cache geometry fixed rather than read from the host CPU. Callgrind's cache
/// simulation otherwise takes the machine's own cache sizes, so the estimated
/// cycles — which weigh misses — would differ between a laptop and a runner
/// running the same instructions. These are a common desktop shape: 32 KiB
/// 8-way L1 and 8 MiB 16-way last-level, 64-byte lines.
const CACHE: [&str; 3] = ["--I1=32768,8,64", "--D1=32768,8,64", "--LL=8388608,16,64"];

main!(
    config = LibraryBenchmarkConfig::default()
        .pass_through_envs(["FIELDGLASS_PERF_CORPUS"])
        .tool(Callgrind::with_args(CACHE));
    library_benchmark_groups = scenarios
);
