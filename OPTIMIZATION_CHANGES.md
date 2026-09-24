# What changed in this Leptos fork

This fork reduces repeated work and temporary allocation during server rendering.
It does not reuse rendered pages or dynamic results between requests.

## The approach

Leptos still builds views, runs the reactive system, renders fresh request data,
and supports hydration and streaming. The changes avoid unnecessary copies,
allocations and setup when the renderer already knows the work is complete.

| Change | How it works | Main implementation |
|---|---|---|
| Literal markup | The view macro keeps known text/attributes in static representations; dynamic attributes, spreads and directives retain their normal paths. | `leptos_macro/src/view/mod.rs`, `tachys/src/html/attribute/precompiled.rs`, `tachys/src/view/static_str.rs` |
| Dynamic text | `format_view!` writes formatted, escaped values directly into the output buffer. String escaping avoids temporary strings. | `tachys/src/view/display.rs`, `tachys/src/view/strings.rs`, `tachys/src/html/attribute/value.rs` |
| Async preparation | `Suspend::from_fn` can retain a prepared future/view within one request. Suspense avoids creating monitoring effects when children are already ready. Pending resources keep their normal discovery and wakeup handling. | `tachys/src/reactive_graph/suspense.rs`, `leptos/src/suspense_component.rs`, `reactive_graph/src/computed/async_derived/mod.rs` |
| View ownership and dispatch | Owned views can avoid recursive ownership conversion; erased views share a per-type dispatch table. | `tachys/src/view/any_view.rs` |
| Small collections | Small reactive edge sets and optional inline view lists avoid small heap allocations, spilling when necessary. Class fragments can render directly from arrays. | `reactive_graph/src/graph/small_set.rs`, `tachys/src/view/iterators.rs`, `tachys/src/html/class.rs` |
| Streaming buffers | Tags write into the stream buffer, cleared class/style scratch capacity is reused, and chunk collection reuses existing strings. | `tachys/src/html/element/mod.rs`, `tachys/src/ssr/mod.rs`, `integrations/utils/src/lib.rs` |
| Finished HTTP responses | Fully completed HTML and serialization use a sized body. Pending or long serialization tails still stream; metadata, headers and owner cleanup are retained. | `integrations/utils/src/lib.rs`, `integrations/axum/src/lib.rs`, `integrations/actix/src/lib.rs` |
| Metadata | Head tags and document attributes use one request-local buffer instead of three channels. Title state shares an allocation while retaining its locks and reactive behavior. | `meta/src/lib.rs`, `meta/src/title.rs` |
| Optional island startup | `HydrationScripts when_needed=true` can omit unused startup scripts on completed pages without islands. It defaults off; actual islands and streaming keep startup support. | `leptos/src/hydration/mod.rs`, `hydration_context/src/ssr.rs` |
| Fixed URL configuration | The immutable fallback URL base is parsed once. Every request URL is still parsed independently. | `router/src/location/server.rs` |

## The September 24 follow-up

This smaller pass also addresses compatibility with existing applications:

1. **Restore the original integration APIs.** `ExtendResponse::from_app` and
   `build_response` again accept the upstream stream-only callback types. New
   `from_app_rendered` and `build_response_rendered` variants carry completion
   information for the built-in integrations. Legacy custom stream builders
   retain their original hydration bootstrap behavior. If you already adopted
   the earlier fork's `Rendered` callback, use the new `*_rendered` names.
2. **Share hydration storage once.** Pending resources, errors and sealed error
   boundaries share one allocation instead of three. Their locks remain separate.
   Serialization still works if its original context has been dropped.
3. **Reserve output capacity.** Hydration setup strings reserve their known fixed
   portion, and script wrappers reserve their exact required size before writing.
   Their contents and ordering are unchanged.
4. **Encode nonces on the stack.** The same 16 random bytes become the same
   22-character URL-safe encoding, without first allocating a temporary String.
   The public shared-string representation and fresh randomness remain.
5. **Avoid redundant reference counts.** Context traversal moves existing owner
   handles instead of cloning them at each step. Shared-context lookup returns
   the handle it already owns. Lookup, shadowing, updates and removal keep their
   existing behavior; no context values are cached.

## What applications need to know

The benchmark storefront also changed eight source files. It uses direct server-
function awaits where data is not shared or hydrated, borrows from the existing
immutable catalog, and opts into the new rendering APIs. Fresh owned dynamic
strings remain supported. Results therefore combine framework improvements and
an application migration; they are not all automatic gains for unchanged apps.
The exact eight-file migration is [this patch](performance/storefront-migration.patch),
against `tokio-rs/topcoat` commit `f26ca9fcdc120efd7f6bd40693eaaa36c00b70ba`,
under its [included MIT license](performance/TOPCOAT-LICENSE). Apply from the
Topcoat root with `git apply --directory=benchmarks/leptos <patch-path>`.
The app must resolve Leptos crates to this fork to use the added APIs.

Existing Resources and rendering APIs remain available. `format_view!` defers
formatting, so formatting should be pure and reactive values should be used in
reactive closures. Keep `when_needed=false` if WASM startup has side effects even
on pages without islands. Inline list capacity is an optional memory tradeoff.

There is no new unsafe Rust. No feature was intentionally removed. Browser speed,
compile time, WASM size and every possible feature combination have not all been
benchmarked, so this remains an experimental fork rather than a certified release.

## Validation

The follow-up passed 27 Leptos tests, 7 integration tests, 4 hydration-context
tests, and reactive-graph suites with 126 passed and one upstream ignored test
in each of ordinary and sandboxed modes, run serially. Axum/Actix checks and the
browser-target build passed. New tests cover the legacy integration signatures,
context shadow/update/removal, nonce encoding, exact empty serialization, and
pending serialization that outlives its context.

Parallel immediate-effect tests failed in both this fork and a pristine upstream
control; serial runs passed. The earlier browser checks cover hydration, reactive
updates, async errors, list changes and immediate/delayed/streamed islands. A
known title-only remount issue was also reproduced in pristine upstream.

## Measurements

Both frameworks use Rust 1.98.0 MSVC, mimalloc, fat LTO, one codegen unit,
native CPU targeting and PGO trained on the same 185 URLs, on the same Ryzen
9800X3D. Topcoat is unchanged at `f26ca9fcdc120efd7f6bd40693eaaa36c00b70ba`.
No builds, profiling or browser tests ran alongside the HTTP measurements.

Three 15-second samples per route, 32 connections, five-second warmups,
uncompressed HTTP/1.1; the table reports medians:

| Route | Leptos req/s | Topcoat req/s | Leptos difference |
|---|---:|---:|---:|
| Home | 139,658 | 142,238 | -1.8% |
| Product grid | 105,337 | 97,704 | +7.8% |
| Product detail | 135,721 | 141,273 | -3.9% |

The original three-route arithmetic-mean metric differs by -0.13%:
**effectively tied**, not a decisive overall win. Home and detail still trail.
A separate mixed workload uses 185 URLs (100 product IDs, 21 pages with four
sort orders, and home), with three 20-second samples in each order:

| Run order | Leptos req/s | Topcoat req/s | Leptos difference |
|---|---:|---:|---:|
| Leptos first | 114,934 | 111,012 | +3.5% |
| Topcoat first | 113,493 | 111,103 | +2.2% |

This establishes modest median wins on the tested mixed workload and grid,
not superiority on every route or a production database workload. Individual
samples overlap. This follow-up does not establish a separate significant
HTTP throughput gain over the previous fork; its directly measured allocation
benefit is three fewer allocations and nine fewer reallocations per response.
Final allocation counts are 205 / 330 / 235 for home / grid / detail.

All 185 complete Leptos page comparisons passed, normalizing only fresh CSP
nonces and the already-declared removal of unused island startup scripts.
All 185 Topcoat visible-text comparisons passed too. Fresh nonces and alternating
requests were checked. A fresh live-browser check also passed hydration,
Resource updates, error recovery, list growth/shrink and anchored view remount,
with no console warnings or errors.

The measured framework implementation is commit `a5d455fe`. Detailed samples,
binary hashes, source hashes and validation results are in
[the measurement record](performance/ssr-followup.json). The preceding document
[SSR_OPTIMIZATION.md](SSR_OPTIMIZATION.md) retains the earlier results and limits.
