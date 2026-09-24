use crate::{
    html::attribute::{any_attribute::AnyAttribute, Attribute},
    hydration::Cursor,
    ssr::StreamBuilder,
    view::{
        add_attr::AddAnyAttr, iterators::OptionState, Mountable, Position,
        PositionState, Render, RenderHtml,
    },
};
use any_spawner::Executor;
use futures::{
    future::{AbortHandle, Abortable},
    select, FutureExt,
};
use or_poisoned::OrPoisoned;
use reactive_graph::{
    computed::{
        suspense::{LocalResourceNotifier, SuspenseContext},
        ScopedFuture,
    },
    graph::{
        AnySource, AnySubscriber, Observer, ReactiveNode, Source, Subscriber,
        ToAnySubscriber, WithObserver,
    },
    owner::{on_cleanup, provide_context, use_context},
};
use std::{
    cell::RefCell,
    fmt::Debug,
    future::{Future, IntoFuture},
    mem,
    pin::Pin,
    rc::Rc,
    sync::{Arc, Mutex, Weak},
};
use throw_error::ErrorHook;

/// A suspended `Future`, which can be used in the view.
pub struct Suspend<T> {
    pub(crate) subscriber: SuspendSubscriber,
    pub(crate) inner: SuspendFuture<T>,
}

pub(crate) enum SuspendFuture<T> {
    Pending(Pin<Box<dyn Future<Output = T> + Send>>),
    Ready(Option<T>),
}

// Only the boxed future is structurally pinned. Completed values are never
// exposed through Pin and may be moved out when their view is consumed.
impl<T> Unpin for SuspendFuture<T> {}

impl<T> Future for SuspendFuture<T> {
    type Output = T;
    fn poll(
        self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<T> {
        match self.get_mut() {
            Self::Pending(future) => future.as_mut().poll(cx),
            Self::Ready(value) => std::task::Poll::Ready(
                value.take().expect("completed view already consumed"),
            ),
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct SuspendSubscriber {
    inner: Arc<SuspendSubscriberInner>,
}

#[derive(Debug)]
struct SuspendSubscriberInner {
    outer_subscriber: Option<AnySubscriber>,
    sources: Mutex<Vec<AnySource>>,
}

impl SuspendSubscriber {
    pub fn new() -> Self {
        let outer_subscriber = Observer::get();
        Self {
            inner: Arc::new(SuspendSubscriberInner {
                outer_subscriber,
                sources: Default::default(),
            }),
        }
    }

    /// Re-links all reactive sources from this to another subscriber.
    ///
    /// This is used to collect reactive dependencies during the rendering phase, and only later
    /// connect them to any outer effect, to prevent the completion of async resources from
    /// triggering the render effect to run a second time.
    pub fn forward(&self) {
        if let Some(to) = &self.inner.outer_subscriber {
            let sources =
                mem::take(&mut *self.inner.sources.lock().or_poisoned());
            for source in sources {
                source.add_subscriber(to.clone());
                to.add_source(source);
            }
        }
    }
}

impl ReactiveNode for SuspendSubscriberInner {
    fn mark_dirty(&self) {}

    fn mark_check(&self) {}

    fn mark_subscribers_check(&self) {}

    fn update_if_necessary(&self) -> bool {
        false
    }
}

impl Subscriber for SuspendSubscriberInner {
    fn add_source(&self, source: AnySource) {
        self.sources.lock().or_poisoned().push(source);
    }

    fn clear_sources(&self, subscriber: &AnySubscriber) {
        for source in mem::take(&mut *self.sources.lock().or_poisoned()) {
            source.remove_subscriber(subscriber);
        }
    }
}

impl ToAnySubscriber for SuspendSubscriber {
    fn to_any_subscriber(&self) -> AnySubscriber {
        AnySubscriber(
            Arc::as_ptr(&self.inner) as usize,
            Arc::downgrade(&self.inner) as Weak<dyn Subscriber + Send + Sync>,
        )
    }
}

impl<T> Suspend<T> {
    /// Creates an asynchronous reactive view from a future factory.
    ///
    /// On the server, the future is retained through resource discovery and
    /// consumed when rendering: preparing a resolved asynchronous view does not
    /// reconstruct it at every Suspense traversal. This state belongs to one
    /// view instance, never to a global or cross-request cache. Synchronous
    /// resource reads conservatively restart discovery so conditional reads
    /// still see resources that finished loading between traversals.
    ///
    /// In the browser, the factory runs inside a reactive render effect, just
    /// like `move || Suspend::new(factory())`, including subsequent updates.
    pub fn from_fn<F, Fut>(mut factory: F) -> ReactiveSuspend<T>
    where
        F: FnMut() -> Fut + Send + 'static,
        Fut: IntoFuture<Output = T>,
        Fut::IntoFuture: Send + 'static,
        T: 'static,
    {
        ReactiveSuspend {
            factory: Box::new(move || Suspend::new(factory())),
            prepared: None,
        }
    }

    /// Creates a new suspended view.
    pub fn new<Fut>(fut: Fut) -> Self
    where
        Fut: IntoFuture<Output = T>,
        Fut::IntoFuture: Send + 'static,
    {
        let subscriber = SuspendSubscriber::new();
        let any_subscriber = subscriber.to_any_subscriber();
        let inner = any_subscriber
            .with_observer(|| Box::pin(ScopedFuture::new(fut.into_future())));
        Self {
            subscriber,
            inner: SuspendFuture::Pending(inner),
        }
    }
}

/// A future factory with reactive browser updates and retained server preparation.
///
/// Construct with [`Suspend::from_fn`]. Await resources inside the future to
/// take advantage of retained preparation; synchronous resource discovery keeps
/// the existing conservative behavior.
pub struct ReactiveSuspend<T> {
    factory: Box<dyn FnMut() -> Suspend<T> + Send>,
    prepared: Option<Suspend<T>>,
}

impl<T> ReactiveSuspend<T> {
    fn take_view(&mut self) -> Suspend<T> {
        self.prepared.take().unwrap_or_else(|| (self.factory)())
    }
}

impl<T: RenderHtml + 'static> Render for ReactiveSuspend<T> {
    type State = super::RenderEffectState<SuspendState<T>>;

    fn build(self) -> Self::State {
        self.factory.build()
    }

    fn rebuild(self, state: &mut Self::State) {
        self.factory.rebuild(state);
    }
}

impl<T: RenderHtml + 'static> AddAnyAttr for ReactiveSuspend<T> {
    type Output<A: Attribute> =
        <Box<dyn FnMut() -> Suspend<T> + Send> as AddAnyAttr>::Output<A>;

    fn add_any_attr<A: Attribute>(self, attr: A) -> Self::Output<A>
    where
        Self::Output<A>: RenderHtml,
    {
        self.factory.add_any_attr(attr)
    }
}

impl<T: RenderHtml + 'static> RenderHtml for ReactiveSuspend<T> {
    type AsyncOutput = Option<T>;
    type Owned = Self;
    const MIN_LENGTH: usize = T::MIN_LENGTH;

    fn dry_resolve(&mut self) {
        use reactive_graph::computed::suspense::synchronous_read_epoch;
        let before = synchronous_read_epoch();
        self.prepared
            .get_or_insert_with(|| (self.factory)())
            .dry_resolve();
        if synchronous_read_epoch() != before {
            self.prepared = None;
        }
    }

    async fn resolve(mut self) -> Self::AsyncOutput {
        self.take_view().resolve().await
    }

    fn to_html_with_buf(
        mut self,
        buf: &mut String,
        position: &mut Position,
        escape: bool,
        mark_branches: bool,
        extra: Vec<AnyAttribute>,
    ) {
        self.take_view().to_html_with_buf(
            buf,
            position,
            escape,
            mark_branches,
            extra,
        );
    }

    fn to_html_async_with_buf<const OUT_OF_ORDER: bool>(
        mut self,
        buf: &mut StreamBuilder,
        position: &mut Position,
        escape: bool,
        mark_branches: bool,
        extra: Vec<AnyAttribute>,
    ) {
        self.take_view().to_html_async_with_buf::<OUT_OF_ORDER>(
            buf,
            position,
            escape,
            mark_branches,
            extra,
        );
    }

    fn hydrate<const FROM_SERVER: bool>(
        self,
        cursor: &Cursor,
        position: &PositionState,
    ) -> Self::State {
        self.factory.hydrate::<FROM_SERVER>(cursor, position)
    }

    fn into_owned(self) -> Self {
        self
    }
}

impl<T> Debug for Suspend<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Suspend").finish()
    }
}

/// Retained view state for [`Suspend`].
pub struct SuspendState<T>
where
    T: Render,
{
    inner: Rc<RefCell<OptionState<T>>>,
}

impl<T> Mountable for SuspendState<T>
where
    T: Render,
{
    fn unmount(&mut self) {
        self.inner.borrow_mut().unmount();
    }

    fn mount(
        &mut self,
        parent: &crate::renderer::types::Element,
        marker: Option<&crate::renderer::types::Node>,
    ) {
        self.inner.borrow_mut().mount(parent, marker);
    }

    fn insert_before_this(&self, child: &mut dyn Mountable) -> bool {
        self.inner.borrow_mut().insert_before_this(child)
    }

    fn elements(&self) -> Vec<crate::renderer::types::Element> {
        self.inner.borrow().elements()
    }
}

impl<T> Render for Suspend<T>
where
    T: Render + 'static,
{
    type State = SuspendState<T>;

    fn build(self) -> Self::State {
        let Self { subscriber, inner } = self;

        // create a Future that will be aborted on on_cleanup
        // this prevents trying to access signals or other resources inside the Suspend, after the
        // await, if they have already been cleaned up
        let (abort_handle, abort_registration) = AbortHandle::new_pair();
        let mut fut = Box::pin(Abortable::new(inner, abort_registration));
        on_cleanup(move || abort_handle.abort());

        // poll the future once immediately
        // if it's already available, start in the ready state
        // otherwise, start with the fallback
        let initial = fut.as_mut().now_or_never().and_then(Result::ok);
        let initially_pending = initial.is_none();
        let inner = Rc::new(RefCell::new(initial.build()));

        // get a unique ID if there's a SuspenseContext
        let id = use_context::<SuspenseContext>().map(|sc| sc.task_id());
        let error_hook = use_context::<Arc<dyn ErrorHook>>();

        // if the initial state was pending, spawn a future to wait for it
        // spawning immediately means that our now_or_never poll result isn't lost
        // if it wasn't pending at first, we don't need to poll the Future again
        if initially_pending {
            reactive_graph::spawn_local_scoped({
                let state = Rc::clone(&inner);
                async move {
                    let _guard = error_hook.as_ref().map(|hook| {
                        throw_error::set_error_hook(Arc::clone(hook))
                    });

                    let value = fut.as_mut().await;
                    drop(id);

                    if let Ok(value) = value {
                        Some(value).rebuild(&mut *state.borrow_mut());
                    }

                    subscriber.forward();
                }
            });
        } else {
            subscriber.forward();
        }

        SuspendState { inner }
    }

    fn rebuild(self, state: &mut Self::State) {
        let Self { subscriber, inner } = self;

        // create a Future that will be aborted on on_cleanup
        // this prevents trying to access signals or other resources inside the Suspend, after the
        // await, if they have already been cleaned up
        let (abort_handle, abort_registration) = AbortHandle::new_pair();
        let fut = Abortable::new(inner, abort_registration);
        on_cleanup(move || abort_handle.abort());

        // get a unique ID if there's a SuspenseContext
        let id = use_context::<SuspenseContext>().map(|sc| sc.task_id());
        let error_hook = use_context::<Arc<dyn ErrorHook>>();

        // spawn the future, and rebuild the state when it resolves
        reactive_graph::spawn_local_scoped({
            let state = Rc::clone(&state.inner);
            async move {
                let _guard = error_hook
                    .as_ref()
                    .map(|hook| throw_error::set_error_hook(Arc::clone(hook)));

                let value = fut.await;
                drop(id);

                // waiting a tick here allows Suspense to remount if necessary, which prevents some
                // edge cases in which a rebuild can't happen while unmounted because the DOM node
                // has no parent
                Executor::tick().await;
                if let Ok(value) = value {
                    Some(value).rebuild(&mut *state.borrow_mut());
                }

                subscriber.forward();
            }
        });
    }
}

impl<T> AddAnyAttr for Suspend<T>
where
    T: Send + AddAnyAttr + 'static,
{
    type Output<SomeNewAttr: Attribute> =
        Suspend<<T as AddAnyAttr>::Output<SomeNewAttr::CloneableOwned>>;

    fn add_any_attr<NewAttr: Attribute>(
        self,
        attr: NewAttr,
    ) -> Self::Output<NewAttr>
    where
        Self::Output<NewAttr>: RenderHtml,
    {
        let attr = attr.into_cloneable_owned();
        Suspend::new(async move {
            let this = self.inner.await;
            this.add_any_attr(attr)
        })
    }
}

impl<T> RenderHtml for Suspend<T>
where
    T: RenderHtml + Sized + 'static,
{
    type AsyncOutput = Option<T>;
    type Owned = Self;

    const MIN_LENGTH: usize = T::MIN_LENGTH;

    fn to_html_with_buf(
        self,
        buf: &mut String,
        position: &mut Position,
        escape: bool,
        mark_branches: bool,
        extra_attrs: Vec<AnyAttribute>,
    ) {
        // TODO wrap this with a Suspense as needed
        // currently this is just used for Routes, which creates a Suspend but never actually needs
        // it (because we don't lazy-load routes on the server)
        if let Some(inner) = self.inner.now_or_never() {
            inner.to_html_with_buf(
                buf,
                position,
                escape,
                mark_branches,
                extra_attrs,
            );
        }
    }

    fn to_html_async_with_buf<const OUT_OF_ORDER: bool>(
        self,
        buf: &mut StreamBuilder,
        position: &mut Position,
        escape: bool,
        mark_branches: bool,
        extra_attrs: Vec<AnyAttribute>,
    ) where
        Self: Sized,
    {
        let mut fut = self.inner;
        match (&mut fut).now_or_never() {
            Some(inner) => inner.to_html_async_with_buf::<OUT_OF_ORDER>(
                buf,
                position,
                escape,
                mark_branches,
                extra_attrs,
            ),
            None => {
                if use_context::<SuspenseContext>().is_none() {
                    buf.next_id();
                    let (local_tx, mut local_rx) =
                        futures::channel::oneshot::channel::<()>();
                    provide_context(LocalResourceNotifier::from(local_tx));
                    let mut fut = fut.fuse();
                    let fut = async move {
                        select! {
                            _  = local_rx => None,
                            value = fut => Some(value)
                        }
                    };
                    let id = buf.clone_id();

                    // out-of-order streams immediately push fallback,
                    // wrapped by suspense markers
                    if OUT_OF_ORDER {
                        let mut fallback_position = *position;
                        buf.push_fallback::<()>(
                            (),
                            &mut fallback_position,
                            mark_branches,
                            extra_attrs.clone(),
                        );

                        // TODO in 0.8: this should include a nonce
                        // we do have access to nonces via context (because this is the `reactive_graph` module)
                        // but unfortunately the Nonce type is defined in `leptos`, not in `tachys`
                        //
                        // missing it here only affects top-level Suspend, not Suspense components
                        buf.push_async_out_of_order(
                            fut,
                            position,
                            mark_branches,
                            extra_attrs,
                        );
                    } else {
                        buf.push_async({
                            let mut position = *position;
                            async move {
                                let value = fut.await;
                                let mut builder = StreamBuilder::new(id);
                                value.to_html_async_with_buf::<OUT_OF_ORDER>(
                                    &mut builder,
                                    &mut position,
                                    escape,
                                    mark_branches,
                                    extra_attrs,
                                );
                                builder.finish().take_chunks()
                            }
                        });
                        *position = Position::NextChild;
                    }
                }
            }
        }
    }

    // TODO cancellation
    fn hydrate<const FROM_SERVER: bool>(
        self,
        cursor: &Cursor,
        position: &PositionState,
    ) -> Self::State {
        let Self { subscriber, inner } = self;

        // create a Future that will be aborted on on_cleanup
        // this prevents trying to access signals or other resources inside the Suspend, after the
        // await, if they have already been cleaned up
        let (abort_handle, abort_registration) = AbortHandle::new_pair();
        let mut fut = Box::pin(Abortable::new(inner, abort_registration));
        on_cleanup(move || abort_handle.abort());

        // poll the future once immediately
        // if it's already available, start in the ready state
        // otherwise, start with the fallback
        let initial = fut.as_mut().now_or_never().and_then(Result::ok);
        let initially_pending = initial.is_none();
        let inner = Rc::new(RefCell::new(
            initial.hydrate::<FROM_SERVER>(cursor, position),
        ));

        // get a unique ID if there's a SuspenseContext
        let id = use_context::<SuspenseContext>().map(|sc| sc.task_id());
        let error_hook = use_context::<Arc<dyn ErrorHook>>();

        // if the initial state was pending, spawn a future to wait for it
        // spawning immediately means that our now_or_never poll result isn't lost
        // if it wasn't pending at first, we don't need to poll the Future again
        if initially_pending {
            reactive_graph::spawn_local_scoped({
                let state = Rc::clone(&inner);
                async move {
                    let _guard = error_hook.as_ref().map(|hook| {
                        throw_error::set_error_hook(Arc::clone(hook))
                    });

                    let value = fut.as_mut().await;
                    drop(id);

                    if let Ok(value) = value {
                        Some(value).rebuild(&mut *state.borrow_mut());
                    }

                    subscriber.forward();
                }
            });
        } else {
            subscriber.forward();
        }

        SuspendState { inner }
    }

    async fn resolve(self) -> Self::AsyncOutput {
        Some(self.inner.await)
    }

    fn dry_resolve(&mut self) {
        // this is a little crazy, but if a Suspend is immediately available, we end up
        // with a situation where polling it multiple times (here in dry_resolve and then in
        // resolve) causes a runtime panic
        // (see https://github.com/leptos-rs/leptos/issues/3113)
        //
        // at the same time, we do need to dry_resolve Suspend so that we can register synchronous
        // resource reads that happen inside them
        // (see https://github.com/leptos-rs/leptos/issues/2917)
        //
        // fuse()-ing the Future doesn't work, because that will cause the subsequent resolve()
        // simply to be pending forever
        //
        // in this case, though, we can simply... discover that the data are already here, and then
        // stuff them back into a new Future, which can safely be polled after its completion
        if let SuspendFuture::Ready(Some(inner)) = &mut self.inner {
            inner.dry_resolve();
        } else if let Some(mut inner) = (&mut self.inner).now_or_never() {
            inner.dry_resolve();
            self.inner = SuspendFuture::Ready(Some(inner));
        }
    }

    fn into_owned(self) -> Self::Owned {
        self
    }
}
