# Experimental SSR optimization fork

See [What changed](OPTIMIZATION_CHANGES.md) for the September 24 follow-up,
restored integration compatibility, and the latest measurements. The benchmark
figures below describe the earlier September 22 implementation.

This branch is based on upstream `619637063c888ca958629074359759a4487ca6a0`.
The framework implementation commit is `7a337e1d`: **42 files changed, 3,219
insertions and 367 deletions**, including regression tests. This note is a
separate documentation change. This is an experimental fork, not an
upstream-reviewed release or a demonstrated universal performance improvement.

## Usability and compatibility

The tested applications remain usable. Signals, Resources, routing, server
functions, Suspense, transitions, error boundaries, hydration and streaming
implementations remain present. No feature was intentionally removed and no
unsafe Rust was added. Focused tests do not establish that every feature,
feature combination or downstream application is unaffected.

There are real compatibility and behavior costs:

- The September 24 follow-up restores the original custom integration callback
  signatures. Completion-aware integrations can opt into `from_app_rendered`
  and `build_response_rendered`; Axum and Actix use these variants. Use
  `Rendered::streaming` unless the whole HTML stream has completed.
- The new `Suspend::from_fn` retains preparation within a request/view. Read
  its documented resource-discovery and capture semantics; it is not a
  universal replacement for every existing async expression. Existing APIs
  remain available.
- `format_view!` defers formatting. Formatting should be pure; put the view
  inside a reactive closure when it must follow changing inputs.
- `into_any_static` requires owned/static views; the existing borrowed-view
  conversion remains. Compiler-produced literals now use different concrete
  generic types, which can affect downstream code naming implementation types.
- `collect_view_inline::<N>()` trades larger inline storage for fewer small
  heap allocations and spills when needed. It is opt-in, not a universal win.
- `HydrationScripts islands=true when_needed=true` is opt-in. It can omit
  bootstrap on complete pages without islands. Leave it false if WASM startup
  has independent side effects. Ordinary hydration, streaming, actual islands
  and unsupported integrations keep their bootstrap paths.

Core changes also affect macro expansion, erased-view dispatch, reactive graph
edge storage, Suspense discovery, metadata ownership and HTTP response assembly.
These broad changes need further downstream testing before production adoption.

## What was checked

During development, 26 Leptos SSR/resource tests, 9 Tachys tests, 6 integration
tests, 2 metadata-storage tests and 25 metadata documentation tests passed.
Reactive graph suites passed with 125 tests and one upstream ignored test in
both ordinary and sandboxed modes. Axum/Actix checks and WASM builds passed.
These are recorded development checks, not a new exhaustive test run at publication.

Real-browser checks covered hydration, reactive signals and async Resources,
escaped formatted text/attributes, erased branch replacement, list growth beyond
inline capacity and shrink, async error recovery, unmount/remount, title updates,
and immediate/delayed/streamed islands. A title-only `Show` without a DOM anchor
fails to restore its child title on remount. A minimal pristine-upstream browser
control reproduces the same bug; it was not introduced or fixed by this fork.

We did **not** establish non-regression of browser interaction performance,
WASM download size, compile times, binary size, peak memory across workloads,
every feature combination, or production behavior under realistic database I/O.
It would be incorrect to claim that every other feature is equally fast or faster.

## Performance scope

The benchmark also migrated eight storefront source files: direct server-function
awaits for unshared data, borrowed immutable catalog strings with owned dynamic
values still supported, new rendering APIs, typed pagination branches, and
optional unused bootstrap omission. These gains are not all transparent speedups
for unchanged applications. No rendered output or request-dependent resource
result is cached across requests. The benchmark application migration is now included in
`performance/storefront-migration.patch`; the local benchmark harness is not included.

Both frameworks used Rust 1.98.0 MSVC, mimalloc, fat LTO, one codegen unit,
native CPU targeting and PGO trained on the same 185 URLs, on a Ryzen 9800X3D.
Topcoat was unchanged at `f26ca9fcdc120efd7f6bd40693eaaa36c00b70ba`.
Three 15-second samples per route with 32 connections gave:

| Route | Edited Leptos req/s | Topcoat req/s | Difference |
|---|---:|---:|---:|
| Home | 139,018 | 141,729 | -1.9% |
| Product grid | 104,656 | 97,592 | +7.2% |
| Product detail | 135,653 | 139,964 | -3.1% |

The arithmetic-mean metric is effectively tied (+0.011%). A separate 185-URL
mixed workload, three 20-second samples in each framework order, gave median
Leptos advantages of 2.9% and 3.2%. Samples overlap in the reversed-order run.
This is a modest local win, not overwhelming or universal superiority.

All 185 Leptos response comparisons passed after normalizing fresh CSP nonces
and the explicitly declared unused bootstrap removal; all 185 Topcoat visible-
text comparisons passed. Instrumented grid allocations fell from 5,780 to 333
per response, separately from uninstrumented HTTP timings. No runtime performance
benchmarks were rerun solely to publish this previously measured implementation.
