#![cfg(feature = "ssr")]
use leptos::prelude::*;

#[tokio::test]
async fn inline_lists_match_vec_before_and_after_spilling() {
    use futures::StreamExt;
    for length in [0, 1, 2, 3, 12] {
        let items = || {
            (0..length).map(|n| view! { <p title=format_view!("item {n}")>{format_view!("item {n} < &")}</p> })
        };
        let ordinary = items()
            .collect_view()
            .to_html_stream_in_order()
            .collect::<String>()
            .await;
        let inline = items()
            .collect_view_inline::<2>()
            .to_html_stream_in_order()
            .collect::<String>()
            .await;
        assert_eq!(ordinary, inline);
    }
}

#[tokio::test]
async fn deferred_formatting_escapes_text_attributes_and_empty_siblings() {
    use futures::StreamExt;
    let payload = "雪 < & > \"".to_owned();
    let text = format_view!("item {}: {}", 7, payload);
    let title = text.clone();
    let html = view! { <div title=title>{text}</div> }
        .to_html_stream_in_order()
        .collect::<String>()
        .await;
    assert_eq!(html, "<div title=\"item 7: 雪 &lt; &amp; &gt; &quot;\">item 7: 雪 &lt; &amp; &gt; \"</div>");
    assert_eq!(
        view! { <p>{format_view!("{}", "")}{format_view!("{}", "end")}</p> }
            .to_html(),
        "<p> <!>end</p>"
    );
    assert_eq!(
        view! { <script>{format_view!("{}", "a < b && c > d")}</script> }
            .to_html(),
        "<script>a < b && c > d</script>"
    );
}

#[tokio::test]
async fn literal_factories_are_zero_sized_and_keep_escaping_and_overrides() {
    use futures::StreamExt;
    use leptos::tachys::view::static_str::literal;
    assert_eq!(std::mem::size_of_val(&literal(|| "literal")), 0);
    let dynamic = "fresh < & 雪".to_owned();
    let view = view! { <div title="a & b" class="one">{dynamic}</div> };
    let html = view.to_html_stream_in_order().collect::<String>().await;
    assert_eq!(
        html,
        "<div title=\"a &amp; b\" class=\"one\">fresh &lt; &amp; 雪</div>"
    );
    let html = view! { <div class="one">"literal < &"</div> }
        .add_any_attr(leptos::tachys::html::class::class("two"))
        .to_html_stream_in_order()
        .collect::<String>()
        .await;
    assert!(html.contains("class=\"two\""), "{html}");
    assert!(html.contains("literal &lt; &amp;"), "{html}");
}
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};

#[tokio::test]
async fn preparation_retains_a_completed_future_without_caching_requests() {
    for value in ["first < &", "second", ""] {
        let owner = Owner::new();
        owner.with(|| {
            let calls = Arc::new(AtomicUsize::new(0));
            let counted = calls.clone();
            let mut view = Suspend::from_fn(move || {
                counted.fetch_add(1, Ordering::SeqCst);
                async move {
                    view! { <p>{value.to_owned()}</p> }
                }
            });
            view.dry_resolve();
            view.dry_resolve();
            assert_eq!(
                view.to_html(),
                view! { <p>{value.to_owned()}</p> }.to_html()
            );
            assert_eq!(calls.load(Ordering::SeqCst), 1);
        });
    }
}

#[tokio::test]
async fn pending_future_survives_preparation_and_resolves_once() {
    use futures::channel::oneshot;
    let owner = Owner::new();
    let (tx, rx) = oneshot::channel::<String>();
    let mut rx = Some(rx);
    let mut view = owner.with(|| {
        Suspend::from_fn(move || {
            let rx = rx.take().expect("future factory was restarted");
            async move { rx.await.unwrap() }
        })
    });
    owner.with(|| {
        view.dry_resolve();
        view.dry_resolve();
    });
    tx.send("fresh < value".into()).unwrap();
    assert_eq!(view.resolve().await.to_html(), "fresh &lt; value");
}

#[tokio::test]
async fn synchronous_resource_discovery_restarts_the_factory() {
    use reactive_graph::{
        computed::suspense::SuspenseContext, computed::ArcAsyncDerived,
        signal::ArcRwSignal,
    };
    use slotmap::{DefaultKey, SlotMap};
    _ = any_spawner::Executor::init_tokio();
    let owner = Owner::new();
    let calls = Arc::new(AtomicUsize::new(0));
    let counted = calls.clone();
    let resource = owner.with(|| {
        provide_context(SuspenseContext {
            tasks: ArcRwSignal::new(SlotMap::<DefaultKey, ()>::new()),
        });
        ArcAsyncDerived::new(|| async { "ready".to_owned() })
    });
    resource.clone().await;
    let mut view = owner.with(|| {
        Suspend::from_fn(move || {
            counted.fetch_add(1, Ordering::SeqCst);
            let resource = resource.clone();
            async move { resource.get().unwrap_or_default() }
        })
    });
    owner.with(|| {
        view.dry_resolve();
        view.dry_resolve();
        assert_eq!(view.to_html(), "ready");
    });
    assert_eq!(calls.load(Ordering::SeqCst), 3);
}

#[test]
fn both_erasure_paths_preserve_dynamic_content_and_borrowed_support() {
    let borrowed = String::from("borrowed < & \"");
    let view =
        view! { <p title=borrowed.as_str()>{borrowed.as_str()}</p> }.into_any();
    drop(borrowed);
    assert_eq!(
        view.to_html(),
        "<p title=\"borrowed &lt; &amp; &quot;\">borrowed &lt; &amp; \"</p>"
    );
    for value in ["first", "second < &"] {
        let build =
            || view! { <div class=value.to_owned()>{value.to_owned()}</div> };
        assert_eq!(
            build().into_any().to_html(),
            build().into_any_static().to_html()
        );
    }
}

#[tokio::test]
async fn compiled_literals_and_class_fragments_preserve_stream_and_override_semantics(
) {
    use futures::StreamExt;
    for value in ["first < &", "second", ""] {
        let compiled = || view! { <div class="  base & x  " title="a < & &#x22;" aria-label="test"><span>{value.to_owned()}</span></div> };
        assert_eq!(
            compiled()
                .to_html_stream_in_order()
                .collect::<String>()
                .await,
            compiled().to_html()
        );
        let empty = || view! { <p class="" title="">{value.to_owned()}</p> };
        assert_eq!(
            empty().to_html_stream_in_order().collect::<String>().await,
            empty().to_html()
        );
        let fragments =
            || view! { <p class=["base", value]>{value.to_owned()}</p> };
        let expected =
            view! { <p class=format!("base {value}")>{value.to_owned()}</p> }
                .to_html();
        assert_eq!(
            fragments()
                .to_html_stream_in_order()
                .collect::<String>()
                .await,
            expected
        );
        assert_eq!(fragments().into_any().to_html(), expected);
    }
    // Fluent additions invalidate the compiled attribute representation.
    let changed = || {
        view! { <div class="base" title="literal">"child"</div> }
            .into_inner()
            .class("replacement")
    };
    assert_eq!(
        changed()
            .to_html_stream_in_order()
            .collect::<String>()
            .await,
        changed().to_html()
    );
    assert!(changed().to_html().contains("class=\"replacement\""));
}

#[tokio::test]
async fn prepared_suspense_discovers_conditional_resources() {
    use futures::StreamExt;
    _ = any_spawner::Executor::init_tokio();
    let owner = Owner::new();
    owner.set();
    let first = Resource::new(
        || (),
        |_| async {
            tokio::task::yield_now().await;
            true
        },
    );
    let second = Resource::new(
        || (),
        |_| async {
            tokio::task::yield_now().await;
            "second < &".to_owned()
        },
    );
    let app = view! {
        <Suspense fallback=|| "waiting">
            {Suspend::from_fn(move || async move {
                if first.get().unwrap_or(false) { second.get().unwrap_or_default() }
                else { "not ready".to_owned() }
            })}
        </Suspense>
    };
    assert_eq!(
        app.to_html_stream_in_order().collect::<String>().await,
        "second &lt; &amp;"
    );
}

#[tokio::test]
async fn prepared_suspense_preserves_nested_async_boundaries() {
    use futures::StreamExt;
    _ = any_spawner::Executor::init_tokio();
    let owner = Owner::new();
    owner.set();
    let first = Resource::new(
        || (),
        |_| async {
            tokio::task::yield_now().await;
            "outer".to_owned()
        },
    );
    let second = Resource::new(
        || (),
        |_| async {
            tokio::task::yield_now().await;
            "inner".to_owned()
        },
    );
    let app = view! {
        <Suspense fallback=|| "outer fallback">
            {Suspend::from_fn(move || async move {
                let text = first.await;
                view! { <section>{text}
                    <Suspense fallback=|| "inner fallback">
                        {Suspend::from_fn(move || async move { second.await })}
                    </Suspense>
                </section> }
            })}
        </Suspense>
    };
    let html = app.to_html_stream_in_order().collect::<String>().await;
    assert!(html.contains("outer") && html.contains("inner"), "{html}");
    assert!(!html.contains("fallback"), "{html}");
}

#[tokio::test]
async fn prepared_local_resource_keeps_server_fallback() {
    use futures::StreamExt;
    _ = any_spawner::Executor::init_tokio();
    let owner = Owner::new_root(Some(Arc::new(
        hydration_context::SsrSharedContext::new(),
    )));
    owner.set();
    let local = LocalResource::new(|| async { "browser only" });
    let app = view! {
        <Suspense fallback=|| "local fallback">
            {Suspend::from_fn(move || async move { local.await })}
        </Suspense>
    };
    assert_eq!(
        app.to_html_stream_in_order().collect::<String>().await,
        "local fallback"
    );
}
