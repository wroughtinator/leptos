use super::{SerializedDataId, SharedContext};
use crate::{PinnedFuture, PinnedStream};
use futures::{
    future::join_all,
    stream::{self, once},
    Stream, StreamExt,
};
use or_poisoned::OrPoisoned;
use std::{
    collections::HashSet,
    fmt::{Debug, Write},
    mem,
    pin::Pin,
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc, Mutex, RwLock,
    },
    task::{Context, Poll},
};
use throw_error::{Error, ErrorId};

// These buffers have the same lifetime and are always shared together with
// the serialization stream. Keep their independent locks in one allocation.
#[derive(Default)]
struct AsyncBuffers {
    async_buf: RwLock<Vec<(SerializedDataId, PinnedFuture<String>)>>,
    errors: RwLock<Vec<(SerializedDataId, ErrorId, Error)>>,
    sealed_error_boundaries: RwLock<HashSet<SerializedDataId>>,
}

#[derive(Default)]
/// The shared context that should be used on the server side.
pub struct SsrSharedContext {
    id: AtomicUsize,
    non_hydration_id: AtomicUsize,
    is_hydrating: AtomicBool,
    hydration_used: AtomicBool,
    deferred_hydration_enabled: AtomicBool,
    hydration_scripts: Mutex<Vec<Box<dyn FnOnce() -> String + Send>>>,
    sync_buf: RwLock<Vec<ResolvedData>>,
    data: Arc<AsyncBuffers>,
    deferred: Mutex<Vec<PinnedFuture<()>>>,
    incomplete: Arc<Mutex<Vec<SerializedDataId>>>,
}

impl SsrSharedContext {
    /// Creates a new shared context for rendering HTML on the server.
    pub fn new() -> Self {
        Self {
            is_hydrating: AtomicBool::new(true),
            non_hydration_id: AtomicUsize::new(usize::MAX),
            ..Default::default()
        }
    }

    /// Creates a new shared context for rendering HTML on the server in "islands" mode.
    ///
    /// This defaults to a mode in which the app is not hydrated, but allows you to opt into
    /// hydration for certain portions using [`SharedContext::set_is_hydrating`].
    pub fn new_islands() -> Self {
        Self {
            is_hydrating: AtomicBool::new(false),
            non_hydration_id: AtomicUsize::new(usize::MAX),
            ..Default::default()
        }
    }

    /// Consume the data buffers, awaiting all async resources,
    /// returning both sync and async buffers.
    /// Useful to implement custom hydration contexts.
    ///
    /// WARNING: this will clear the internal buffers, it should only be called once.
    /// A second call would return an empty `vec![]`.
    pub async fn consume_buffers(&self) -> Vec<(SerializedDataId, String)> {
        let sync_data = mem::take(&mut *self.sync_buf.write().or_poisoned());
        let async_data =
            mem::take(&mut *self.data.async_buf.write().or_poisoned());

        let mut all_data = Vec::new();
        for resolved in sync_data {
            all_data.push((resolved.0, resolved.1));
        }
        for (id, fut) in async_data {
            let data = fut.await;
            all_data.push((id, data));
        }
        all_data
    }
}

impl Debug for SsrSharedContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SsrSharedContext")
            .field("id", &self.id)
            .field("is_hydrating", &self.is_hydrating)
            .field("sync_buf", &self.sync_buf)
            .field("async_buf", &self.data.async_buf.read().or_poisoned().len())
            .finish()
    }
}

impl SharedContext for SsrSharedContext {
    fn enable_deferred_hydration_scripts(&self) {
        self.deferred_hydration_enabled
            .store(true, Ordering::SeqCst);
    }
    fn supports_deferred_hydration_scripts(&self) -> bool {
        self.deferred_hydration_enabled.load(Ordering::SeqCst)
    }

    fn defer_hydration_script(
        &self,
        script: Box<dyn FnOnce() -> String + Send>,
    ) {
        self.hydration_scripts.lock().or_poisoned().push(script);
    }

    fn take_deferred_hydration_scripts(
        &self,
        html_complete: bool,
    ) -> Vec<String> {
        let scripts =
            mem::take(&mut *self.hydration_scripts.lock().or_poisoned());
        if html_complete && !self.hydration_used.load(Ordering::SeqCst) {
            Vec::new()
        } else {
            scripts.into_iter().map(|script| script()).collect()
        }
    }

    fn is_browser(&self) -> bool {
        false
    }

    #[track_caller]
    fn next_id(&self) -> SerializedDataId {
        let id = if self.get_is_hydrating() {
            self.id.fetch_add(1, Ordering::Relaxed)
        } else {
            self.non_hydration_id.fetch_sub(1, Ordering::Relaxed)
        };
        SerializedDataId(id)
    }

    fn write_async(&self, id: SerializedDataId, fut: PinnedFuture<String>) {
        self.data.async_buf.write().or_poisoned().push((id, fut))
    }

    fn read_data(&self, _id: &SerializedDataId) -> Option<String> {
        None
    }

    fn await_data(&self, _id: &SerializedDataId) -> Option<String> {
        None
    }

    fn get_is_hydrating(&self) -> bool {
        self.is_hydrating.load(Ordering::SeqCst)
    }

    fn set_is_hydrating(&self, is_hydrating: bool) {
        if is_hydrating {
            self.hydration_used.store(true, Ordering::SeqCst);
        }
        self.is_hydrating.store(is_hydrating, Ordering::SeqCst)
    }

    fn errors(&self, boundary_id: &SerializedDataId) -> Vec<(ErrorId, Error)> {
        self.data
            .errors
            .read()
            .or_poisoned()
            .iter()
            .filter_map(|(boundary, id, error)| {
                if boundary == boundary_id {
                    Some((id.clone(), error.clone()))
                } else {
                    None
                }
            })
            .collect()
    }

    fn register_error(
        &self,
        error_boundary_id: SerializedDataId,
        error_id: ErrorId,
        error: Error,
    ) {
        self.data.errors.write().or_poisoned().push((
            error_boundary_id,
            error_id,
            error,
        ));
    }

    fn take_errors(&self) -> Vec<(SerializedDataId, ErrorId, Error)> {
        mem::take(&mut *self.data.errors.write().or_poisoned())
    }

    fn seal_errors(&self, boundary_id: &SerializedDataId) {
        self.data
            .sealed_error_boundaries
            .write()
            .or_poisoned()
            .insert(boundary_id.clone());
    }

    fn pending_data(&self) -> Option<PinnedStream<String>> {
        let sync_data = mem::take(&mut *self.sync_buf.write().or_poisoned());
        let async_data = self.data.async_buf.read().or_poisoned();

        // 1) initial, synchronous setup chunk
        let mut initial_chunk = String::with_capacity(
            "__RESOLVED_RESOURCES=[];__SERIALIZED_ERRORS=[];__PENDING_RESOURCES=[];__RESOURCE_RESOLVERS=[];".len(),
        );
        // resolved synchronous resources and errors
        initial_chunk.push_str("__RESOLVED_RESOURCES=[");
        for resolved in sync_data {
            resolved.write_to_buf(&mut initial_chunk);
            initial_chunk.push(',');
        }
        initial_chunk.push_str("];");

        initial_chunk.push_str("__SERIALIZED_ERRORS=[");
        for error in mem::take(&mut *self.data.errors.write().or_poisoned()) {
            // Debug-format first to get a valid, quoted JS string literal
            // (escaping `"`, `\`, control chars), then rewrite every remaining
            // `<` to a single-backslash `<` JS unicode escape. Escaping
            // *after* `{:?}` keeps it one backslash, so the HTML tokenizer
            // never sees `</script>` while the browser's JS string parser
            // still decodes `<` straight back to `<` for the consumer.
            let msg =
                format!("{:?}", error.2.to_string()).replace('<', "\\u003c");
            _ = write!(
                initial_chunk,
                "[{}, {}, {}],",
                error.0 .0, error.1, msg
            );
        }
        initial_chunk.push_str("];");

        // pending async resources
        initial_chunk.push_str("__PENDING_RESOURCES=[");
        for (id, _) in async_data.iter() {
            _ = write!(&mut initial_chunk, "{},", id.0);
        }
        initial_chunk.push_str("];");

        // resolvers
        initial_chunk.push_str("__RESOURCE_RESOLVERS=[];");

        let async_data = AsyncDataStream {
            data: Arc::clone(&self.data),
        };

        let incomplete = Arc::clone(&self.incomplete);

        let stream = stream::once(async move { initial_chunk })
            .chain(async_data)
            .chain(once(async move {
                let mut script =
                    String::with_capacity("__INCOMPLETE_CHUNKS=[];".len());
                script.push_str("__INCOMPLETE_CHUNKS=[");
                for chunk in mem::take(&mut *incomplete.lock().or_poisoned()) {
                    _ = write!(script, "{},", chunk.0);
                }
                script.push_str("];");
                script
            }));
        Some(Box::pin(stream))
    }

    fn during_hydration(&self) -> bool {
        false
    }

    fn hydration_complete(&self) {}

    fn defer_stream(&self, wait_for: PinnedFuture<()>) {
        self.deferred.lock().or_poisoned().push(wait_for);
    }

    fn await_deferred(&self) -> Option<PinnedFuture<()>> {
        let deferred = mem::take(&mut *self.deferred.lock().or_poisoned());
        if deferred.is_empty() {
            None
        } else {
            Some(Box::pin(async move {
                join_all(deferred).await;
            }))
        }
    }

    fn set_incomplete_chunk(&self, id: SerializedDataId) {
        self.incomplete.lock().or_poisoned().push(id);
    }

    fn get_incomplete_chunk(&self, id: &SerializedDataId) -> bool {
        self.incomplete
            .lock()
            .or_poisoned()
            .iter()
            .any(|entry| entry == id)
    }
}

struct AsyncDataStream {
    data: Arc<AsyncBuffers>,
}

impl Stream for AsyncDataStream {
    type Item = String;

    fn poll_next(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Self::Item>> {
        let mut resolved = String::new();
        let mut async_buf = self.data.async_buf.write().or_poisoned();
        let data = mem::take(&mut *async_buf);
        for (id, mut fut) in data {
            match fut.as_mut().poll(cx) {
                // if it's not ready, put it back into the queue
                Poll::Pending => {
                    async_buf.push((id, fut));
                }
                Poll::Ready(data) => {
                    let data = data.replace('<', "\\u003c");
                    _ = write!(
                        resolved,
                        "__RESOLVED_RESOURCES[{}] = {:?};",
                        id.0, data
                    );
                }
            }
        }
        let sealed = self.data.sealed_error_boundaries.read().or_poisoned();
        for error in mem::take(&mut *self.data.errors.write().or_poisoned()) {
            if !sealed.contains(&error.0) {
                // see the initial-chunk path: Debug-format, then single-
                // backslash-escape `<` so the JS parser decodes it back to `<`
                let msg = format!("{:?}", error.2.to_string())
                    .replace('<', "\\u003c");
                _ = write!(
                    resolved,
                    "__SERIALIZED_ERRORS.push([{}, {}, {}]);",
                    error.0 .0, error.1, msg
                );
            }
        }

        if async_buf.is_empty() && resolved.is_empty() {
            return Poll::Ready(None);
        }
        if resolved.is_empty() {
            return Poll::Pending;
        }

        Poll::Ready(Some(resolved))
    }
}

#[derive(Debug)]
struct ResolvedData(SerializedDataId, String);

impl ResolvedData {
    pub fn write_to_buf(&self, buf: &mut String) {
        let ResolvedData(id, ser) = self;
        // escapes < to prevent it being interpreted as another opening HTML tag
        let ser = ser.replace('<', "\\u003c");
        write!(buf, "{}: {:?}", id.0, ser).unwrap();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::{executor::block_on, StreamExt};
    use std::fmt;

    #[test]
    fn empty_serialization_keeps_exact_setup_and_final_chunks() {
        let ctx = SsrSharedContext::new();
        let chunks = block_on(ctx.pending_data().unwrap().collect::<Vec<_>>());
        assert_eq!(chunks, [
            "__RESOLVED_RESOURCES=[];__SERIALIZED_ERRORS=[];__PENDING_RESOURCES=[];__RESOURCE_RESOLVERS=[];",
            "__INCOMPLETE_CHUNKS=[];",
        ]);
    }

    #[test]
    fn pending_serialization_outlives_context_and_preserves_late_state() {
        use futures::{channel::oneshot, FutureExt};
        let ctx = SsrSharedContext::new();
        let (tx, rx) = oneshot::channel();
        ctx.write_async(
            SerializedDataId(7),
            Box::pin(async move { rx.await.unwrap() }),
        );
        let mut stream = ctx.pending_data().unwrap();
        let initial = block_on(stream.next()).unwrap();
        assert!(initial.contains("__PENDING_RESOURCES=[7,];"));
        ctx.register_error(
            SerializedDataId(2),
            ErrorId::from(3_usize),
            Error::from(CustomError("sealed")),
        );
        ctx.seal_errors(&SerializedDataId(2));
        ctx.set_incomplete_chunk(SerializedDataId(9));
        assert!(stream.next().now_or_never().is_none());
        drop(ctx);
        tx.send("fresh value".to_owned()).unwrap();
        let chunks = block_on(stream.collect::<Vec<_>>());
        assert_eq!(
            chunks,
            [
                "__RESOLVED_RESOURCES[7] = \"fresh value\";",
                "__INCOMPLETE_CHUNKS=[9,];",
            ]
        );
    }

    #[derive(Debug)]
    struct CustomError(&'static str);

    impl fmt::Display for CustomError {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str(self.0)
        }
    }

    impl std::error::Error for CustomError {}

    /// An error message containing `</script>` must not be able to escape
    /// the surrounding <script> tag in the streamed initial chunk.
    #[test]
    fn error_in_initial_chunk_escapes_script_close_tag() {
        let ctx = SsrSharedContext::new();
        ctx.register_error(
            SerializedDataId(0),
            ErrorId::from(0_usize),
            Error::from(CustomError(
                "boom</script><script>alert('pwned')</script><script>",
            )),
        );

        let mut stream = ctx.pending_data().expect("pending_data on ssr");
        let initial = block_on(stream.next()).expect("at least one chunk");

        assert!(
            !initial.contains("</script>"),
            "initial chunk must not contain a literal `</script>` substring, \
             got: {initial}"
        );
        assert!(
            !initial.contains('<'),
            "initial chunk must not contain a literal `<` character anywhere \
             inside the serialized errors, got: {initial}"
        );
        assert!(
            initial.contains("\\u003c") && !initial.contains("\\\\u003c"),
            "expected a single-backslash `\\u003c` escape in place of `<` (a \
             double backslash would be decoded to a literal `\\u003c` and \
             shown raw to the user), got: {initial}"
        );
    }

    /// The same escape must be applied to errors emitted later via the
    /// async stream (AsyncDataStream::poll_next).
    #[test]
    fn error_in_async_stream_escapes_script_close_tag() {
        let ctx = SsrSharedContext::new();

        // park one async resource so AsyncDataStream emits a follow-up chunk
        ctx.write_async(
            SerializedDataId(1),
            Box::pin(async { String::from("\"ok\"") }),
        );

        let mut stream = ctx.pending_data().expect("pending_data on ssr");
        // skip the initial setup chunk; we want the next one
        let _initial = block_on(stream.next()).expect("initial chunk");

        // register an error after pending_data() has been called so it is
        // serialized through the streaming path rather than the initial chunk
        ctx.register_error(
            SerializedDataId(2),
            ErrorId::from(7_usize),
            Error::from(CustomError("late</script><script>x</script>")),
        );

        let mut saw_error = false;
        while let Some(chunk) = block_on(stream.next()) {
            if chunk.contains("__SERIALIZED_ERRORS.push") {
                saw_error = true;
                assert!(
                    !chunk.contains("</script>"),
                    "streamed error chunk must not contain `</script>`: \
                     {chunk}"
                );
                assert!(
                    chunk.contains("\\u003c") && !chunk.contains("\\\\u003c"),
                    "streamed error chunk should carry a single-backslash \
                     escaped `<`: {chunk}"
                );
            }
        }
        assert!(
            saw_error,
            "expected at least one streamed __SERIALIZED_ERRORS.push chunk"
        );
    }
}
