//! Instruction counts per scenario, under Callgrind (the gated work tier).
//!
//!     FIELDGLASS_PERF_CORPUS=<dir> cargo bench --bench instructions -- --save-summary=json
//!
//! Needs Valgrind and a `gungraun-runner` of the same version as the `gungraun`
//! dev-dependency; `run.sh` checks both before it starts and fails when either
//! is missing, rather than letting the tier skip.
//!
//! Every scenario in the catalog is one benchmark. Gungraun runs `prepare` as
//! setup, outside the instrumented function, and the benchmark hands the
//! prepared scenario back so its output is dropped outside it too: the count is
//! the operation's and nothing else's. `prepare` ends by settling glibc's heap
//! ([`settle_heap`]), so the allocator work preparation leaves behind is done
//! before the count starts rather than inside it.

use std::hint::black_box;

use fieldglass_perf::{Corpus, Prepared, Via, catalog};
use gungraun::Callgrind;
use gungraun::prelude::*;

/// Scenario ids in catalog order. Gungraun names each benchmark by its
/// position, so `tools/check_perf_gate.py` maps positions back through this
/// same list, which `run.sh` writes out with `--list`.
///
/// The list is leaked rather than owned. Gungraun drops what is left of it
/// after `prepare` returns and before the operation starts, and anything freed
/// there would undo [`settle_heap`]. A leaked slice frees nothing.
fn scenario_ids() -> &'static [&'static str] {
    let corpus = Corpus::from_env().expect("the corpus named by FIELDGLASS_PERF_CORPUS");
    let ids: Vec<&'static str> = catalog(&corpus)
        .into_iter()
        .map(|s| &*s.id.leak())
        .collect();
    ids.leak()
}

fn prepare(id: &&'static str) -> Prepared {
    let prepared = {
        let corpus = Corpus::from_env().expect("the corpus named by FIELDGLASS_PERF_CORPUS");
        let scenario = catalog(&corpus)
            .into_iter()
            .find(|s| s.id == *id)
            .expect("an id from scenario_ids");
        Prepared::new(&corpus, &scenario, Via::Memory)
    };
    // Last, once the corpus and the catalog are freed: nothing is freed
    // between here and the operation.
    settle_heap();
    prepared
}

/// Start the operation from the same allocator state, whatever preparation
/// allocated and freed.
///
/// glibc keeps small freed chunks in fastbins and merges them into its free
/// lists only when something forces it (`malloc_consolidate`): a request above
/// the small-bin range, or a large free. Left alone, the first such call inside
/// the operation pays to merge what preparation freed, tens of thousands of
/// instructions that follow preparation's allocations rather than the
/// operation's. On PR #1799, adding one unused field moved
/// `zarr-v3-zstd-S/open` and `netcdf-classic-S/slice` by 40 to 55 thousand;
/// on #1818, lengthening the corpus path moved `grib2-5.41-L/codec` by 157
/// thousand.
/// `malloc_trim(0)` consolidates every arena and hands free pages back to the
/// system, so the only fastbin chunks the operation meets are its own.
///
/// It does not make the heap identical. Where preparation's free chunks sit
/// still decides which requests the free lists serve and which grow the heap,
/// and the per-thread cache keeps what preparation left in it. Measured on
/// #1818, that leaves a row moving by under a quarter of its gate tolerance,
/// where the merging alone used to take more than twice it.
///
/// Two ways without `unsafe` were measured the same way and are noisier, by
/// the largest residual in estimated cycles: turning fastbins off with
/// `GLIBC_TUNABLES=glibc.malloc.mxfast=0` left 44% of tolerance, and forcing
/// the merge by allocating and freeing one 96 KiB block left 60%, against 22%
/// for the trim. So this is the one `unsafe` call, kept in this function.
fn settle_heap() {
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    {
        unsafe extern "C" {
            /// glibc's `<malloc.h>`. Returns 1 when memory went back to the
            /// system and 0 when none could; neither is an error.
            fn malloc_trim(pad: usize) -> std::ffi::c_int;
        }
        // SAFETY: `malloc_trim` takes a byte count, not a pointer, and works on
        // the allocator's own free lists under the allocator's lock. It moves
        // and frees no live allocation. glibc has no safe binding for it, and
        // the safe alternatives above measured noisier.
        // nosemgrep: rust.lang.security.unsafe-usage.unsafe-usage
        unsafe {
            malloc_trim(0);
        }
    }
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
