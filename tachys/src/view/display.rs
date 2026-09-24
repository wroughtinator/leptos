//! Deferred formatting that writes escaped text directly into the HTML buffer.
use super::{
    add_attr::AddAnyAttr, Position, PositionState, Render, RenderHtml,
    ToTemplate,
};
use crate::{
    html::attribute::{
        any_attribute::AnyAttribute, escape_attr, Attribute, AttributeValue,
    },
    hydration::Cursor,
    renderer::types::Element,
};
use std::fmt::{self, Display, Write};

/// A displayable value rendered as escaped text or an escaped attribute.
///
/// Formatting happens when the view is rendered. Unlike `format!`, server
/// rendering does not allocate an intermediate string. Browser state retains
/// an ordinary string, so hydration and reactive text/attribute updates keep
/// their usual behavior. Prefer pure `Display` implementations.
#[derive(Clone, Copy, Debug)]
pub struct DisplayView<T>(pub T);

/// A captured formatting expression used by [`crate::format_view!`].
#[doc(hidden)]
#[derive(Clone, Copy)]
pub struct FormatFn<F>(pub F);
impl<F: Fn(&mut fmt::Formatter<'_>) -> fmt::Result> Display for FormatFn<F> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        (self.0)(formatter)
    }
}
impl<T: Display> Display for DisplayView<T> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

/// Captures formatting inputs by move and formats directly into the renderer.
///
/// Use inside a reactive closure to recapture changing inputs on each update.
/// Values must be owned, `Send`, and `'static`; attribute values must also be
/// cloneable. HTML text and attributes are escaped by their respective renderers.
#[macro_export]
macro_rules! format_view {
    ($($args:tt)*) => {
        $crate::view::display::DisplayView($crate::view::display::FormatFn(
            move |f: &mut ::std::fmt::Formatter<'_>| ::std::write!(f, $($args)*)
        ))
    };
}

struct Escaped<'a> {
    output: &'a mut String,
    attribute: bool,
}
impl Write for Escaped<'_> {
    fn write_str(&mut self, text: &str) -> fmt::Result {
        if self.attribute {
            escape_attr(text, self.output);
        } else if memchr::memchr3(b'&', b'<', b'>', text.as_bytes()).is_none() {
            self.output.push_str(text);
        } else {
            html_escape::encode_text_to_string(text, self.output);
        }
        Ok(())
    }
}

impl<T: Display> Render for DisplayView<T> {
    type State = <String as Render>::State;
    fn build(self) -> Self::State {
        Render::build(self.to_string())
    }
    fn rebuild(self, state: &mut Self::State) {
        Render::rebuild(self.to_string(), state);
    }
}
impl<T: Display + Send + 'static> AddAnyAttr for DisplayView<T> {
    type Output<A: Attribute> = Self;
    fn add_any_attr<A: Attribute>(self, _attr: A) -> Self::Output<A> {
        self
    }
}
impl<T: Display + Send + 'static> RenderHtml for DisplayView<T> {
    type AsyncOutput = Self;
    type Owned = Self;
    const MIN_LENGTH: usize = 0;
    fn dry_resolve(&mut self) {}
    async fn resolve(self) -> Self {
        self
    }
    fn into_owned(self) -> Self {
        self
    }
    fn to_html_with_buf(
        self,
        buf: &mut String,
        position: &mut Position,
        escape: bool,
        _mark: bool,
        _extra: Vec<AnyAttribute>,
    ) {
        if matches!(position, Position::NextChildAfterText) {
            buf.push_str("<!>");
        }
        let start = buf.len();
        if escape {
            write!(
                Escaped {
                    output: buf,
                    attribute: false
                },
                "{self}"
            )
            .unwrap();
        } else {
            write!(buf, "{self}").unwrap();
        }
        if escape && buf.len() == start {
            buf.push(' ');
        }
        *position = Position::NextChildAfterText;
    }
    fn hydrate<const FROM_SERVER: bool>(
        self,
        cursor: &Cursor,
        position: &PositionState,
    ) -> Self::State {
        RenderHtml::hydrate::<FROM_SERVER>(self.to_string(), cursor, position)
    }
}
impl<T> ToTemplate for DisplayView<T> {
    const TEMPLATE: &'static str = <String as ToTemplate>::TEMPLATE;
    fn to_template(
        buf: &mut String,
        class: &mut String,
        style: &mut String,
        inner: &mut String,
        position: &mut Position,
    ) {
        <String as ToTemplate>::to_template(buf, class, style, inner, position);
    }
}
impl<T: Display + Clone + Send + 'static> AttributeValue for DisplayView<T> {
    type State = <String as AttributeValue>::State;
    type AsyncOutput = Self;
    type Cloneable = Self;
    type CloneableOwned = Self;
    fn html_len(&self) -> usize {
        0
    }
    fn to_html(self, key: &str, buf: &mut String) {
        buf.push(' ');
        buf.push_str(key);
        buf.push_str("=\"");
        write!(
            Escaped {
                output: buf,
                attribute: true
            },
            "{self}"
        )
        .unwrap();
        buf.push('"');
    }
    fn to_template(key: &str, buf: &mut String) {
        <String as AttributeValue>::to_template(key, buf);
    }
    fn hydrate<const FROM_SERVER: bool>(
        self,
        key: &str,
        el: &Element,
    ) -> Self::State {
        AttributeValue::hydrate::<FROM_SERVER>(self.to_string(), key, el)
    }
    fn build(self, el: &Element, key: &str) -> Self::State {
        AttributeValue::build(self.to_string(), el, key)
    }
    fn rebuild(self, key: &str, state: &mut Self::State) {
        AttributeValue::rebuild(self.to_string(), key, state);
    }
    fn into_cloneable(self) -> Self {
        self
    }
    fn into_cloneable_owned(self) -> Self {
        self
    }
    fn dry_resolve(&mut self) {}
    async fn resolve(self) -> Self {
        self
    }
}
