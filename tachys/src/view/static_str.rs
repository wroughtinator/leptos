//! String values whose storage already outlives every view.
use super::{Position, PositionState, Render, RenderHtml, ToTemplate};
use crate::{
    html::{
        attribute::{any_attribute::AnyAttribute, AttributeValue},
        class::IntoClass,
        style::IntoStyle,
    },
    hydration::Cursor,
    renderer::types::Element,
};

/// A string backed by static storage, including a literal in a `view!` macro.
///
/// Unlike a general borrowed `&str`, this value does not need to allocate when
/// a view is converted into an owned or type-erased view. Rendering, escaping,
/// hydration, and rebuilding behave just like `&str`. No rendered output is
/// cached, and different values of this type may contain different strings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StaticStr<T = &'static str>(pub T);

impl<T: StaticStringValue> From<StaticStr<T>>
    for std::borrow::Cow<'static, str>
{
    fn from(value: StaticStr<T>) -> Self {
        Self::Borrowed(value.0.get())
    }
}

/// Storage for a string whose value is stable for the lifetime of a view.
pub trait StaticStringValue: Clone + Send + 'static {
    /// Returns this view's string. Implementations must return a stable value.
    fn get(&self) -> &'static str;
}

impl StaticStringValue for &'static str {
    fn get(&self) -> &'static str {
        self
    }
}

/// Compiler-generated literal factory; capturing no values makes this zero-sized.
#[doc(hidden)]
#[derive(Clone, Copy)]
pub struct LiteralFactory<F>(pub F);

impl<F: Fn() -> &'static str + Clone + Send + 'static> StaticStringValue
    for LiteralFactory<F>
{
    fn get(&self) -> &'static str {
        (self.0)()
    }
}

/// Encodes a literal in its factory's type instead of storing a fat pointer per view.
/// The factory must be effect-free and return the same literal on every call.
#[doc(hidden)]
pub fn literal<F: Fn() -> &'static str + Clone + Send + 'static>(
    factory: F,
) -> StaticStr<LiteralFactory<F>> {
    StaticStr(LiteralFactory(factory))
}

impl<T: StaticStringValue> AsRef<str> for StaticStr<T> {
    fn as_ref(&self) -> &str {
        self.0.get()
    }
}

impl<T: StaticStringValue> super::add_attr::AddAnyAttr for StaticStr<T> {
    type Output<A: crate::html::attribute::Attribute> = Self;
    fn add_any_attr<A: crate::html::attribute::Attribute>(
        self,
        _attr: A,
    ) -> Self::Output<A> {
        self
    }
}

impl<T: StaticStringValue> Render for StaticStr<T> {
    type State = <&'static str as Render>::State;
    fn build(self) -> Self::State {
        Render::build(self.0.get())
    }
    fn rebuild(self, state: &mut Self::State) {
        Render::rebuild(self.0.get(), state);
    }
}

impl<T: StaticStringValue> RenderHtml for StaticStr<T> {
    type AsyncOutput = Self;
    type Owned = Self;
    const MIN_LENGTH: usize = 0;
    fn dry_resolve(&mut self) {}
    async fn resolve(self) -> Self {
        self
    }
    fn html_len(&self) -> usize {
        self.0.get().len()
    }
    fn to_html_with_buf(
        self,
        buf: &mut String,
        position: &mut Position,
        escape: bool,
        mark_branches: bool,
        extra: Vec<AnyAttribute>,
    ) {
        self.0.get().to_html_with_buf(
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
        RenderHtml::hydrate::<FROM_SERVER>(self.0.get(), cursor, position)
    }
    fn into_owned(self) -> Self {
        self
    }
}

impl<T: StaticStringValue> ToTemplate for StaticStr<T> {
    const TEMPLATE: &'static str = <&str as ToTemplate>::TEMPLATE;
    fn to_template(
        buf: &mut String,
        class: &mut String,
        style: &mut String,
        inner_html: &mut String,
        position: &mut Position,
    ) {
        <&str as ToTemplate>::to_template(
            buf, class, style, inner_html, position,
        );
    }
}

impl<T: StaticStringValue> AttributeValue for StaticStr<T> {
    type State = <&'static str as AttributeValue>::State;
    type AsyncOutput = Self;
    type Cloneable = Self;
    type CloneableOwned = Self;
    fn html_len(&self) -> usize {
        self.0.get().len()
    }
    fn to_html(self, key: &str, buf: &mut String) {
        AttributeValue::to_html(self.0.get(), key, buf);
    }
    fn to_template(key: &str, buf: &mut String) {
        <&str as AttributeValue>::to_template(key, buf);
    }
    fn hydrate<const FROM_SERVER: bool>(
        self,
        key: &str,
        el: &Element,
    ) -> Self::State {
        AttributeValue::hydrate::<FROM_SERVER>(self.0.get(), key, el)
    }
    fn build(self, el: &Element, key: &str) -> Self::State {
        AttributeValue::build(self.0.get(), el, key)
    }
    fn rebuild(self, key: &str, state: &mut Self::State) {
        AttributeValue::rebuild(self.0.get(), key, state);
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

impl<T: StaticStringValue> IntoClass for StaticStr<T> {
    type State = <&'static str as IntoClass>::State;
    type AsyncOutput = Self;
    type Cloneable = Self;
    type CloneableOwned = Self;
    fn html_len(&self) -> usize {
        self.0.get().len()
    }
    fn to_html(self, class: &mut String) {
        IntoClass::to_html(self.0.get(), class);
    }
    fn should_overwrite(&self) -> bool {
        true
    }
    fn hydrate<const FROM_SERVER: bool>(self, el: &Element) -> Self::State {
        IntoClass::hydrate::<FROM_SERVER>(self.0.get(), el)
    }
    fn build(self, el: &Element) -> Self::State {
        IntoClass::build(self.0.get(), el)
    }
    fn rebuild(self, state: &mut Self::State) {
        IntoClass::rebuild(self.0.get(), state);
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
    fn reset(state: &mut Self::State) {
        <&str as IntoClass>::reset(state);
    }
}

impl<T: StaticStringValue> IntoStyle for StaticStr<T> {
    type State = <&'static str as IntoStyle>::State;
    type AsyncOutput = Self;
    type Cloneable = Self;
    type CloneableOwned = Self;
    fn to_html(self, style: &mut String) {
        IntoStyle::to_html(self.0.get(), style);
    }
    fn hydrate<const FROM_SERVER: bool>(self, el: &Element) -> Self::State {
        IntoStyle::hydrate::<FROM_SERVER>(self.0.get(), el)
    }
    fn build(self, el: &Element) -> Self::State {
        IntoStyle::build(self.0.get(), el)
    }
    fn rebuild(self, state: &mut Self::State) {
        IntoStyle::rebuild(self.0.get(), state);
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
    fn reset(state: &mut Self::State) {
        <&str as IntoStyle>::reset(state);
    }
}
