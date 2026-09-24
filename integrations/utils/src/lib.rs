#![allow(clippy::type_complexity)]

use futures::{stream::once, FutureExt, Stream, StreamExt};
use hydration_context::{SharedContext, SsrSharedContext};
use leptos::{
    context::provide_context,
    nonce::use_nonce,
    prelude::ReadValue,
    reactive::owner::{Owner, Sandboxed},
    IntoView, PrefetchLazyFn, WasmSplitManifest,
};
use leptos_config::LeptosOptions;
use leptos_meta::{Link, ServerMetaContextOutput};
use std::{future::Future, pin::Pin, sync::Arc};

pub type PinnedStream<T> = Pin<Box<dyn Stream<Item = T> + Send>>;
pub type PinnedFuture<T> = Pin<Box<dyn Future<Output = T> + Send>>;
pub type BoxedFnOnce<T> = Box<dyn FnOnce() -> T + Send>;

/// An HTML stream together with the renderer's completion state.
pub struct Rendered {
    body: RenderBody,
}

enum RenderBody {
    Streaming(PinnedStream<String>),
    Complete {
        html: String,
        tail: PinnedStream<String>,
    },
}

impl Rendered {
    /// Returns the HTML and serialization stream without completion metadata.
    pub fn into_stream(self) -> PinnedStream<String> {
        match self.body {
            RenderBody::Streaming(stream) => stream,
            RenderBody::Complete { html, tail } => {
                Box::pin(once(async move { html }).chain(tail))
            }
        }
    }

    /// A stream whose components may still render metadata in later polls.
    pub fn streaming(stream: PinnedStream<String>) -> Self {
        Self {
            body: RenderBody::Streaming(stream),
        }
    }

    /// A fully rendered HTML document, followed by resource serialization.
    /// Only call after the entire HTML-rendering stream has completed. This
    /// proves metadata registration is complete without a scheduler yield.
    pub fn complete(html: String, tail: PinnedStream<String>) -> Self {
        Self {
            body: RenderBody::Complete { html, tail },
        }
    }
}

/// Collects rendered chunks, reusing the first nonempty chunk's allocation.
pub async fn collect_html(stream: impl Stream<Item = String>) -> String {
    futures::pin_mut!(stream);
    let mut output = String::new();
    while let Some(chunk) = stream.next().await {
        if output.is_empty() {
            output = chunk;
        } else {
            output.push_str(&chunk);
        }
    }
    output
}

fn join_html_chunks(chunks: Vec<String>) -> String {
    let mut chunks = chunks.into_iter();
    let mut output = chunks.next().unwrap_or_default();
    output.reserve(chunks.as_slice().iter().map(String::len).sum());
    for chunk in chunks {
        output.push_str(&chunk);
    }
    output
}

pub trait ExtendResponse: Sized {
    type ResponseOptions: Send;

    fn from_stream(stream: impl Stream<Item = String> + Send + 'static)
        -> Self;

    /// Builds a response whose rendering and serialization are already complete.
    fn from_html(html: String) -> Self {
        Self::from_stream(once(async move { html }))
    }

    fn extend_response(&mut self, opt: &Self::ResponseOptions);

    fn set_default_content_type(&mut self, content_type: &str);

    fn from_app<IV>(
        app_fn: impl FnOnce() -> IV + Send + 'static,
        meta_context: ServerMetaContextOutput,
        additional_context: impl FnOnce() + Send + 'static,
        res_options: Self::ResponseOptions,
        stream_builder: fn(
            IV,
            BoxedFnOnce<PinnedStream<String>>,
            bool,
        ) -> PinnedFuture<Rendered>,
        supports_ooo: bool,
    ) -> impl Future<Output = Self> + Send
    where
        IV: IntoView + 'static,
    {
        async move {
            let prefetches = PrefetchLazyFn::default();

            let (owner, stream) = build_response(
                app_fn,
                additional_context,
                stream_builder,
                supports_ooo,
            );

            owner.with(|| provide_context(prefetches.clone()));

            let sc = owner.shared_context().unwrap();

            let rendered = stream.await;

            while let Some(pending) = sc.await_deferred() {
                pending.await;
            }

            if let Some(prefetches) =
                prefetches.0.try_read_value().filter(|p| !p.is_empty())
            {
                use leptos::prelude::*;

                let nonce =
                    use_nonce().map(|n| n.to_string()).unwrap_or_default();
                if let Some(manifest_read) = use_context::<WasmSplitManifest>()
                    .and_then(|m| m.0.try_read_value())
                {
                    let (pkg_path, manifest, wasm_split_file) = &*manifest_read;

                    let all_prefetches = prefetches.iter().flat_map(|key| {
                        manifest.get(*key).into_iter().flatten()
                    });

                    for module in all_prefetches {
                        // to_html() on leptos_meta components registers them with the meta context,
                        // rather than returning HTML directly
                        _ = view! {
                            <Link
                                rel="preload"
                                href=format!("{pkg_path}/{module}.wasm")
                                as_="fetch"
                                type_="application/wasm"
                                crossorigin=nonce.clone()
                            />
                        }
                        .to_html();
                    }
                    _ = view! {
                        <Link rel="modulepreload" href=format!("{pkg_path}/{wasm_split_file}") crossorigin=nonce/>
                    }
                    .to_html();
                }
            }

            // Bound the ready-tail probe so long or pending serialization
            // retains streaming behavior without losing any consumed chunks.
            let (stream, html_complete) = match rendered.body {
                RenderBody::Streaming(stream) => (stream, false),
                RenderBody::Complete { mut html, mut tail } => {
                    for _ in 0..32 {
                        match tail.next().now_or_never() {
                            Some(Some(chunk)) => html.push_str(&chunk),
                            Some(None) => {
                                let html = meta_context.inject_meta_context_into_html(html, || {
                                    join_html_chunks(sc.take_deferred_hydration_scripts(true))
                                });
                                while let Some(pending) = sc.await_deferred() {
                                    pending.await;
                                }
                                let mut res = Self::from_html(html);
                                res.extend_response(&res_options);
                                res.set_default_content_type(
                                    "text/html; charset=utf-8",
                                );
                                Sandboxed::new(async move {
                                    owner.unset_with_forced_cleanup();
                                })
                                .now_or_never()
                                .expect("synchronous owner cleanup");
                                return res;
                            }
                            None => break,
                        }
                    }
                    (Rendered::complete(html, tail).into_stream(), true)
                }
            };
            let stream = stream.ready_chunks(32).map(join_html_chunks);
            let mut stream = Box::pin(
                meta_context
                    .inject_meta_context_with_completion_and_head(
                        stream,
                        html_complete,
                        {
                            let sc = sc.clone();
                            move || {
                                join_html_chunks(
                                    sc.take_deferred_hydration_scripts(
                                        html_complete,
                                    ),
                                )
                            }
                        },
                    )
                    .await
                    .then({
                        let sc = Arc::clone(&sc);
                        move |chunk| {
                            let sc = Arc::clone(&sc);
                            async move {
                                while let Some(pending) = sc.await_deferred() {
                                    pending.await;
                                }
                                chunk
                            }
                        }
                    }),
            );

            // wait for the first chunk of the stream, then set the status and headers
            let first_chunk = stream.next().await.unwrap_or_default();

            // Do not wait for another chunk: a pending stream must keep its
            // streaming behavior. A completed stream can use a sized HTTP body
            // instead of chunked framing and an otherwise redundant body stream.
            let next = match stream.next().now_or_never() {
                Some(None) => {
                    let mut res = Self::from_html(first_chunk);
                    res.extend_response(&res_options);
                    res.set_default_content_type("text/html; charset=utf-8");
                    Sandboxed::new(async move {
                        owner.unset_with_forced_cleanup();
                    })
                    .now_or_never()
                    .expect("synchronous owner cleanup");
                    return res;
                }
                Some(Some(chunk)) => Some(chunk),
                None => None,
            };

            let mut res = Self::from_stream(Sandboxed::new(
                once(async move { first_chunk })
                    .chain(futures::stream::iter(next))
                    .chain(stream)
                    // drop the owner, cleaning up the reactive runtime,
                    // once the stream is over
                    .chain(once(async move {
                        owner.unset_with_forced_cleanup();
                        Default::default()
                    })),
            ));

            res.extend_response(&res_options);

            // Set the Content Type headers on all responses. This makes Firefox show the page source
            // without complaining
            res.set_default_content_type("text/html; charset=utf-8");

            res
        }
    }
}

pub fn build_response<IV>(
    app_fn: impl FnOnce() -> IV + Send + 'static,
    additional_context: impl FnOnce() + Send + 'static,
    stream_builder: fn(
        IV,
        BoxedFnOnce<PinnedStream<String>>,
        // this argument indicates whether a request wants to support out-of-order streaming
        // responses
        bool,
    ) -> PinnedFuture<Rendered>,
    is_islands_router_navigation: bool,
) -> (Owner, PinnedFuture<Rendered>)
where
    IV: IntoView + 'static,
{
    let shared_context = Arc::new(SsrSharedContext::new())
        as Arc<dyn SharedContext + Send + Sync>;
    shared_context.enable_deferred_hydration_scripts();
    let owner = Owner::new_root(Some(Arc::clone(&shared_context)));
    let stream = Box::pin(Sandboxed::new({
        let owner = owner.clone();
        async move {
            let stream = owner.with(|| {
                additional_context();

                // run app
                let app = app_fn();

                let nonce = use_nonce()
                    .as_ref()
                    .map(|nonce| format!(" nonce=\"{nonce}\""))
                    .unwrap_or_default();

                let shared_context = Owner::current_shared_context().unwrap();

                let chunks = Box::new({
                    let shared_context = shared_context.clone();
                    move || {
                        Box::pin(shared_context.pending_data().unwrap().map(
                            move |chunk| {
                                format!("<script{nonce}>{chunk}</script>")
                            },
                        ))
                            as Pin<Box<dyn Stream<Item = String> + Send>>
                    }
                });

                // convert app to appropriate response type
                // and chain the app stream, followed by chunks
                // in theory, we could select here, and intersperse them
                // the problem is that during the DOM walk, that would be mean random <script> tags
                // interspersed where we expect other children
                //
                // we also don't actually start hydrating until after the whole stream is complete,
                // so it's not useful to send those scripts down earlier.
                stream_builder(app, chunks, is_islands_router_navigation)
            });

            stream.await
        }
    }));
    (owner, stream)
}

pub fn static_file_path(options: &LeptosOptions, path: &str) -> String {
    let trimmed_path = path.trim_start_matches('/');
    let path = if trimmed_path.is_empty() {
        "index"
    } else {
        trimmed_path
    };
    format!("{}/{}.html", options.site_root, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    enum TestResponse {
        Html(String),
        Stream(PinnedStream<String>),
    }
    impl ExtendResponse for TestResponse {
        type ResponseOptions = ();
        fn from_stream(
            stream: impl Stream<Item = String> + Send + 'static,
        ) -> Self {
            Self::Stream(Box::pin(stream))
        }
        fn from_html(html: String) -> Self {
            Self::Html(html)
        }
        fn extend_response(&mut self, _: &()) {}
        fn set_default_content_type(&mut self, _: &str) {}
    }

    fn test_builder(
        _: (),
        _: BoxedFnOnce<PinnedStream<String>>,
        complete: bool,
    ) -> PinnedFuture<Rendered> {
        use leptos::prelude::*;
        let tail = use_context::<
            Arc<
                std::sync::Mutex<
                    Option<futures::channel::oneshot::Receiver<()>>,
                >,
            >,
        >()
        .and_then(|rx| rx.lock().unwrap().take());
        Box::pin(async move {
            let html =
                "<html><head></head><body>fresh</body></html>".to_owned();
            let tail: PinnedStream<String> = match tail {
                Some(tail) => Box::pin(once(async move {
                    tail.await.unwrap();
                    "tail".to_owned()
                })),
                None => Box::pin(futures::stream::empty()),
            };
            let rendered = Rendered::complete(html, tail);
            if complete {
                rendered
            } else {
                Rendered::streaming(rendered.into_stream())
            }
        })
    }

    #[tokio::test]
    async fn ready_bodies_finish_cleanup_and_pending_bodies_still_stream() {
        use leptos::prelude::*;
        use std::sync::{
            atomic::{AtomicUsize, Ordering},
            Mutex,
        };
        _ = any_spawner::Executor::init_tokio();
        for (pending, complete) in
            [(false, false), (true, false), (false, true), (true, true)]
        {
            let cleaned = Arc::new(AtomicUsize::new(0));
            let (tx, rx) = futures::channel::oneshot::channel();
            let tail = Arc::new(Mutex::new(pending.then_some(rx)));
            let (context, output) = leptos_meta::ServerMetaContext::new();
            let count = cleaned.clone();
            let response = TestResponse::from_app(
                move || {
                    on_cleanup(move || {
                        count.fetch_add(1, Ordering::SeqCst);
                    });
                },
                output,
                move || {
                    provide_context(context);
                    provide_context(tail);
                },
                (),
                test_builder,
                complete,
            )
            .await;
            if pending {
                assert_eq!(cleaned.load(Ordering::SeqCst), 0);
                let TestResponse::Stream(mut stream) = response else {
                    panic!("pending tail was buffered");
                };
                assert!(stream.next().await.unwrap().contains("fresh"));
                tx.send(()).unwrap();
                assert_eq!(stream.collect::<String>().await, "tail");
                assert_eq!(cleaned.load(Ordering::SeqCst), 1);
            } else {
                let TestResponse::Html(html) = response else {
                    panic!("completed stream was not collapsed");
                };
                assert!(html.contains("fresh"));
                assert_eq!(cleaned.load(Ordering::SeqCst), 1);
            }
        }
    }

    #[tokio::test]
    async fn completed_documents_preserve_long_serialization_tails() {
        fn builder(
            _: (),
            _: BoxedFnOnce<PinnedStream<String>>,
            _: bool,
        ) -> PinnedFuture<Rendered> {
            Box::pin(async {
                Rendered::complete(
                    "<html><head></head><body>fresh</body></html>".to_owned(),
                    Box::pin(futures::stream::iter(
                        (0..200).map(|i| format!("[{i}]")),
                    )),
                )
            })
        }
        _ = any_spawner::Executor::init_tokio();
        let (_, output) = leptos_meta::ServerMetaContext::new();
        let response =
            TestResponse::from_app(|| (), output, || (), (), builder, false)
                .await;
        let TestResponse::Stream(stream) = response else {
            panic!("long serialization tail should retain streaming");
        };
        let actual = stream.collect::<String>().await;
        let expected = format!(
            "<html><head></head><body>fresh</body></html>{}",
            (0..200).map(|i| format!("[{i}]")).collect::<String>()
        );
        assert_eq!(actual, expected);
    }

    #[tokio::test]
    async fn completed_html_includes_metadata_from_delayed_suspense() {
        use leptos::prelude::*;
        use leptos_meta::{Meta, MetaTags, ServerMetaContext, Title};
        fn completed<IV: IntoView + 'static>(
            app: IV,
            chunks: BoxedFnOnce<PinnedStream<String>>,
            _: bool,
        ) -> PinnedFuture<Rendered> {
            let stream = app.into_view().to_html_stream_in_order();
            Box::pin(async move {
                Rendered::complete(collect_html(stream).await, chunks())
            })
        }
        _ = any_spawner::Executor::init_tokio();
        for delay in [0, 10] {
            let (context, output) = ServerMetaContext::new();
            let response = TestResponse::from_app(
                move || {
                    leptos_meta::provide_meta_context();
                    let resource = Resource::new(|| (), move |_| async move {
                        for _ in 0..delay { tokio::task::yield_now().await; }
                        "fresh < &".to_owned()
                    });
                    view! {
                        <html><head><MetaTags/></head><body>
                            <Transition fallback=|| "loading">
                                {Suspend::from_fn(move || async move {
                                    let value = resource.await;
                                    view! { <Title text="Delayed title"/>
                                        <Meta name="description" content=value.clone()/>
                                        <p>{value}</p> }
                                })}
                            </Transition>
                        </body></html>
                    }
                },
                output,
                move || provide_context(context),
                (),
                completed,
                false,
            ).await;
            let html = match response {
                TestResponse::Html(html) => html,
                TestResponse::Stream(stream) => {
                    stream.collect::<String>().await
                }
            };
            let head = html.split("</head>").next().unwrap();
            assert!(head.contains("<title>Delayed title</title>"), "{html}");
            assert!(head.contains("content=\"fresh &lt; &amp;\""), "{html}");
            assert!(html.contains("<p>fresh &lt; &amp;</p>"), "{html}");
        }
    }

    #[tokio::test]
    async fn optional_bootstrap_preserves_islands_streaming_and_regular_hydration(
    ) {
        use leptos::prelude::*;
        use leptos_meta::{MetaTags, ServerMetaContext};

        #[leptos::island]
        fn BootstrapTestIsland() -> impl IntoView {
            view! { <button>"interactive island"</button> }
        }
        fn render<IV: IntoView + 'static>(
            app: IV,
            chunks: BoxedFnOnce<PinnedStream<String>>,
            streaming: bool,
        ) -> PinnedFuture<Rendered> {
            let stream = app.into_view().to_html_stream_in_order();
            Box::pin(async move {
                if streaming {
                    Rendered::streaming(Box::pin(stream.chain(chunks())))
                } else {
                    Rendered::complete(collect_html(stream).await, chunks())
                }
            })
        }
        _ = any_spawner::Executor::init_tokio();
        for (
            island,
            delay,
            streaming,
            islands_mode,
            router,
            when_needed,
            expected,
        ) in [
            (false, 0, false, true, false, true, false),
            (true, 0, false, true, false, true, true),
            (true, 10, false, true, false, true, true),
            (false, 0, true, true, false, true, true),
            (true, 10, true, true, false, true, true),
            (false, 0, false, false, false, true, true),
            (false, 0, false, true, true, true, true),
            (false, 0, false, true, false, false, true),
        ] {
            let (context, output) = ServerMetaContext::new();
            let response = TestResponse::from_app(
                move || {
                    let options = LeptosOptions::builder().output_name("test").build();
                    view! {
                        <html><head><HydrationScripts options islands=islands_mode
                            islands_router=router when_needed=when_needed/><MetaTags/></head>
                        <body><Suspense fallback=|| "loading">
                            {Suspend::from_fn(move || async move {
                                for _ in 0..delay { tokio::task::yield_now().await; }
                                island.then(|| view! { <BootstrapTestIsland/> })
                            })}
                        </Suspense></body></html>
                    }
                }, output, move || provide_context(context), (), render, streaming,
            ).await;
            let html = match response {
                TestResponse::Html(html) => html,
                TestResponse::Stream(stream) => {
                    stream.collect::<String>().await
                }
            };
            let head = html.split("</head>").next().unwrap();
            assert_eq!(
                head.contains("<script type=\"module\""),
                expected,
                "{html}"
            );
            assert_eq!(
                head.contains("rel=\"modulepreload\""),
                expected,
                "{html}"
            );
            assert_eq!(html.contains("interactive island"), island, "{html}");
        }
        // A custom integration that has not enabled the new protocol retains
        // the inline bootstrap, even when the application opts into elision.
        let sc = Arc::new(SsrSharedContext::new_islands());
        let owner = Owner::new_root(Some(sc));
        let html = owner.with(|| {
            let options = LeptosOptions::builder().output_name("test").build();
            view! { <HydrationScripts options islands=true when_needed=true/> }
                .to_html()
        });
        assert!(html.contains("<script type=\"module\""));
    }

    #[tokio::test]
    async fn collection_preserves_pending_empty_and_unicode_chunks() {
        use std::{collections::VecDeque, task::Poll};
        let mut chunks: VecDeque<String> = ["", "first <", "", "雪 &", "last"]
            .into_iter()
            .map(str::to_owned)
            .collect();
        let mut ready = false;
        let stream = futures::stream::poll_fn(move |cx| {
            ready = !ready;
            if ready {
                cx.waker().wake_by_ref();
                Poll::Pending
            } else {
                Poll::Ready(chunks.pop_front())
            }
        });
        assert_eq!(collect_html(stream).await, "first <雪 &last");
        let mut first = String::with_capacity(128);
        first.push_str("first");
        let allocation = first.as_ptr();
        let joined = join_html_chunks(vec![first, "雪".into(), "last".into()]);
        assert_eq!(joined, "first雪last");
        assert_eq!(joined.as_ptr(), allocation);
        assert!(join_html_chunks(vec![]).is_empty());
    }

    #[tokio::test]
    async fn metadata_injection_keeps_marker_fallback_attributes_and_tail() {
        use leptos::prelude::*;
        use leptos_meta::{Body, Html, Meta, ServerMetaContext, Title};
        _ = any_spawner::Executor::init_tokio();
        for marker in ["<!--HEAD-->", ""] {
            let owner = Owner::new();
            owner.set();
            let (context, output) = ServerMetaContext::new();
            provide_context(context);
            leptos_meta::provide_meta_context();
            view! {
                <Title text="Fresh title 雪"/>
                <Meta name="description" content="fresh < &"/>
                <Html attr:lang="fr"/>
                <Body attr:class="theme"/>
            }
            .to_html();
            let body = "dynamic 雪 ".repeat(1000);
            let mut first = String::with_capacity(65536);
            first.push_str(&format!(
                "<html><head>{marker}</head><body>{body}</body></html>"
            ));
            let stream = futures::stream::iter(vec![first, "tail".into()]);
            let rendered = output
                .inject_meta_context(stream)
                .await
                .collect::<String>()
                .await;
            assert!(
                rendered.starts_with("<html lang=\"fr\"><head>"),
                "{rendered}"
            );
            assert!(rendered.contains("<title>Fresh title 雪</title></head>"));
            assert!(rendered.contains("content=\"fresh &lt; &amp;\""));
            assert!(rendered.contains(&format!(
                "<body class=\"theme\">{body}</body></html>tail"
            )));
            if !marker.is_empty() {
                assert!(rendered.contains(marker));
            }
        }
    }
}
