use futures::StreamExt;
use tachys::{
    html::{
        attribute::{
            any_attribute::IntoAnyAttribute, custom::custom_attribute,
        },
        class::class,
        element::{custom, div, input, span, ElementChild, InnerHtmlAttribute},
        style::style,
    },
    ssr::StreamBuilder,
    view::{add_attr::AddAnyAttr, Position, RenderHtml},
};

#[tokio::test]
async fn streamed_dynamic_attributes_match_synchronous_rendering() {
    for (name, enabled) in
        [("first < & \"", true), ("second", false), ("", true)]
    {
        let view = div()
            .add_any_attr(custom_attribute("data-value", name))
            .add_any_attr(class("base"))
            .add_any_attr(class(("active", enabled)))
            .add_any_attr(style("color: red"))
            .child((
                span().child(name),
                input().add_any_attr(custom_attribute("disabled", enabled)),
            ));
        let expected = view.clone().to_html();
        assert_eq!(
            view.clone()
                .to_html_stream_in_order()
                .collect::<String>()
                .await,
            expected
        );
        assert_eq!(
            view.to_html_stream_out_of_order().collect::<String>().await,
            expected
        );
    }
}

#[tokio::test]
async fn streaming_keeps_order_across_pending_chunks_and_appends() {
    use futures::{channel::oneshot, FutureExt};
    use tachys::ssr::StreamChunk;
    let (tx, rx) = oneshot::channel();
    let mut stream = StreamBuilder::new(None);
    stream.push_sync("before");
    stream.push_async(async move {
        let value = rx.await.unwrap();
        [StreamChunk::Sync(value)].into()
    });
    let mut tail = StreamBuilder::new(None);
    tail.push_sync("after");
    stream.append(tail);
    let mut stream = stream.finish();
    assert_eq!(stream.next().await.as_deref(), Some("before"));
    assert!(stream.next().now_or_never().is_none());
    tx.send("dynamic".to_owned()).unwrap();
    assert_eq!(stream.collect::<String>().await, "dynamicafter");
}

#[tokio::test]
async fn out_of_order_stream_preserves_fallback_and_nonce() {
    use futures::{channel::oneshot, FutureExt};
    let (tx, rx) = oneshot::channel::<String>();
    let mut stream = StreamBuilder::new(Some(vec![1]));
    let mut position = Position::FirstChild;
    stream.push_fallback(span().child("loading"), &mut position, false, vec![]);
    stream.push_async_out_of_order_with_nonce(
        async move { Some(span().child(rx.await.unwrap())) },
        &mut position,
        false,
        Some("request-nonce".into()),
        vec![],
    );
    let mut stream = stream.finish();
    assert_eq!(
        stream.next().await.unwrap(),
        "<!--s-1-o--><span>loading</span><!--s-1-c-->"
    );
    assert!(stream.next().now_or_never().is_none());
    tx.send("new < value".to_owned()).unwrap();
    let update = stream.collect::<String>().await;
    assert!(update.starts_with(
        "<template id=\"1-f\"><span>new &lt; value</span></template>"
    ));
    assert!(update.contains("<script nonce=\"request-nonce\">"));
    assert!(update.contains("range.deleteContents()"));
}

#[test]
fn primitive_attributes_escape_and_format_like_strings() {
    use tachys::html::attribute::AttributeValue;
    fn check(value: impl AttributeValue + ToString + Copy) {
        let mut actual = String::new();
        let mut expected = String::new();
        value.to_html("data-value", &mut actual);
        AttributeValue::to_html(value.to_string(), "data-value", &mut expected);
        assert_eq!(actual, expected);
    }
    for value in ['"', '&', '<', '>', '\'', 'é', '🦀'] {
        check(value);
    }
    check(i128::MIN);
    check(u128::MAX);
    check(f64::INFINITY);
    check(f64::NAN);
    check(-0.0f64);
    check("[::1]:8080".parse::<std::net::SocketAddr>().unwrap());
}

#[tokio::test]
async fn streamed_custom_tags_and_inner_html_preserve_output() {
    let view = custom("example-card")
        .add_any_attr(class("outer"))
        .child(div().inner_html("<strong>raw &amp; html</strong>"));
    let expected = view.clone().to_html();
    assert_eq!(
        view.to_html_stream_in_order().collect::<String>().await,
        expected
    );
}

#[tokio::test]
async fn extra_attributes_keep_class_override_semantics() {
    let make_attrs = || {
        vec![
            class("override").into_any_attr(),
            class(("extra", true)).into_any_attr(),
        ]
    };
    let make_view = || {
        div()
            .add_any_attr(class("original"))
            .child("child")
            .__precompile_attributes(" class=\"original\"")
    };
    let mut expected = String::new();
    make_view().to_html_with_buf(
        &mut expected,
        &mut Position::FirstChild,
        true,
        false,
        make_attrs(),
    );
    let mut stream = StreamBuilder::new(None);
    make_view().to_html_async_with_buf::<false>(
        &mut stream,
        &mut Position::FirstChild,
        true,
        false,
        make_attrs(),
    );
    assert_eq!(stream.finish().collect::<String>().await, expected);
    assert!(expected.contains("class=\"override extra\""));
}
