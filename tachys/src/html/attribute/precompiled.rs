use super::{Attribute, NamedAttributeKey, NextAttribute};
use crate::{
    renderer::types::Element,
    view::{Position, ToTemplate},
};

/// A compiler-generated factory for entirely literal attributes.
///
/// The factory must have no effects and return the same literal attributes and
/// exact serialized representation on every call. Unlike `Precompiled`, this
/// representation needs no per-view storage when the factory captures nothing.
/// Dynamic attributes and directives must use the ordinary attribute path.
#[doc(hidden)]
#[derive(Clone, Copy)]
pub struct CompiledAttributes<F>(pub F);

impl<F, A> NextAttribute for CompiledAttributes<F>
where
    F: Fn() -> (A, &'static str) + Clone + Send + 'static,
    A: Attribute,
{
    type Output<N: Attribute> = A::Output<N>;
    fn add_any_attr<N: Attribute>(self, attr: N) -> Self::Output<N> {
        (self.0)().0.add_any_attr(attr)
    }
}

impl<F, A> Attribute for CompiledAttributes<F>
where
    F: Fn() -> (A, &'static str) + Clone + Send + 'static,
    A: Attribute,
{
    const MIN_LENGTH: usize = A::MIN_LENGTH;
    type State = A::State;
    type AsyncOutput = Self;
    type Cloneable = Self;
    type CloneableOwned = Self;
    fn html_len(&self) -> usize {
        (self.0)().1.len()
    }
    fn precompiled_html(&self) -> Option<&'static str> {
        Some((self.0)().1)
    }
    fn to_html(
        self,
        buf: &mut String,
        class: &mut String,
        style: &mut String,
        inner_html: &mut String,
    ) {
        (self.0)().0.to_html(buf, class, style, inner_html);
    }
    fn hydrate<const FROM_SERVER: bool>(self, el: &Element) -> Self::State {
        (self.0)().0.hydrate::<FROM_SERVER>(el)
    }
    fn build(self, el: &Element) -> Self::State {
        (self.0)().0.build(el)
    }
    fn rebuild(self, state: &mut Self::State) {
        (self.0)().0.rebuild(state);
    }
    fn into_cloneable(self) -> Self::Cloneable {
        self
    }
    fn into_cloneable_owned(self) -> Self::CloneableOwned {
        self
    }
    fn dry_resolve(&mut self) {}
    async fn resolve(self) -> Self::AsyncOutput {
        self
    }
    fn keys(&self) -> Vec<NamedAttributeKey> {
        (self.0)().0.keys()
    }
}

impl<F, A> ToTemplate for CompiledAttributes<F>
where
    F: Fn() -> (A, &'static str),
    A: ToTemplate,
{
    const TEMPLATE: &'static str = A::TEMPLATE;
    const CLASS: &'static str = A::CLASS;
    const STYLE: &'static str = A::STYLE;
    const LEN: usize = A::LEN;
    fn to_template(
        buf: &mut String,
        class: &mut String,
        style: &mut String,
        inner_html: &mut String,
        position: &mut Position,
    ) {
        A::to_template(buf, class, style, inner_html, position);
    }
    fn to_template_attribute(
        buf: &mut String,
        class: &mut String,
        style: &mut String,
        inner_html: &mut String,
        position: &mut Position,
    ) {
        A::to_template_attribute(buf, class, style, inner_html, position);
    }
}

/// Compiler-produced literal attributes, retaining their ordinary DOM behavior.
/// The serialized representation must exactly match `attributes`, including
/// escaping and class/style normalization. No dynamic values may be captured.
#[doc(hidden)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Precompiled<A> {
    pub attributes: A,
    pub html: &'static str,
}

impl<A: Attribute> NextAttribute for Precompiled<A> {
    type Output<N: Attribute> = A::Output<N>;
    fn add_any_attr<N: Attribute>(self, attr: N) -> Self::Output<N> {
        // Any later modification invalidates the compiled representation.
        self.attributes.add_any_attr(attr)
    }
}

impl<A: Attribute> Attribute for Precompiled<A> {
    const MIN_LENGTH: usize = A::MIN_LENGTH;
    type State = A::State;
    type AsyncOutput = Precompiled<A::AsyncOutput>;
    type Cloneable = Precompiled<A::Cloneable>;
    type CloneableOwned = Precompiled<A::CloneableOwned>;
    fn html_len(&self) -> usize {
        self.html.len()
    }
    fn precompiled_html(&self) -> Option<&'static str> {
        Some(self.html)
    }
    fn to_html(
        self,
        buf: &mut String,
        class: &mut String,
        style: &mut String,
        inner_html: &mut String,
    ) {
        self.attributes.to_html(buf, class, style, inner_html);
    }
    fn hydrate<const FROM_SERVER: bool>(self, el: &Element) -> Self::State {
        self.attributes.hydrate::<FROM_SERVER>(el)
    }
    fn build(self, el: &Element) -> Self::State {
        self.attributes.build(el)
    }
    fn rebuild(self, state: &mut Self::State) {
        self.attributes.rebuild(state);
    }
    fn into_cloneable(self) -> Self::Cloneable {
        Precompiled {
            attributes: self.attributes.into_cloneable(),
            html: self.html,
        }
    }
    fn into_cloneable_owned(self) -> Self::CloneableOwned {
        Precompiled {
            attributes: self.attributes.into_cloneable_owned(),
            html: self.html,
        }
    }
    fn dry_resolve(&mut self) {
        self.attributes.dry_resolve();
    }
    async fn resolve(self) -> Self::AsyncOutput {
        Precompiled {
            attributes: self.attributes.resolve().await,
            html: self.html,
        }
    }
    fn keys(&self) -> Vec<NamedAttributeKey> {
        self.attributes.keys()
    }
}

impl<A: ToTemplate> ToTemplate for Precompiled<A> {
    const TEMPLATE: &'static str = A::TEMPLATE;
    const CLASS: &'static str = A::CLASS;
    const STYLE: &'static str = A::STYLE;
    const LEN: usize = A::LEN;
    fn to_template(
        buf: &mut String,
        class: &mut String,
        style: &mut String,
        inner_html: &mut String,
        position: &mut Position,
    ) {
        A::to_template(buf, class, style, inner_html, position);
    }
    fn to_template_attribute(
        buf: &mut String,
        class: &mut String,
        style: &mut String,
        inner_html: &mut String,
        position: &mut Position,
    ) {
        A::to_template_attribute(buf, class, style, inner_html, position);
    }
}
