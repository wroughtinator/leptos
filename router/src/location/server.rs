use super::{Url, BASE};
use crate::params::ParamsMap;
use std::sync::{Arc, OnceLock};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RequestUrl(Arc<str>);

impl RequestUrl {
    /// Creates a server-side request URL from a path.
    pub fn new(path: &str) -> Self {
        Self(path.into())
    }
}

impl AsRef<str> for RequestUrl {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl Default for RequestUrl {
    fn default() -> Self {
        Self::new("/")
    }
}

impl RequestUrl {
    pub fn parse(&self) -> Result<Url, url::ParseError> {
        // The fallback origin is immutable framework configuration. Parse it
        // once; request paths, queries, and overriding origins remain fresh.
        static DEFAULT_BASE: OnceLock<(url::Url, String)> = OnceLock::new();
        let (base, origin) = DEFAULT_BASE.get_or_init(|| {
            let base =
                url::Url::parse(BASE).expect("valid default router origin");
            let origin = base.origin().unicode_serialization();
            (base, origin)
        });
        self.parse_using(base, Some(origin))
    }

    pub fn parse_with_base(&self, base: &str) -> Result<Url, url::ParseError> {
        let base = url::Url::parse(base)?;
        self.parse_using(&base, None)
    }

    fn parse_using(
        &self,
        base: &url::Url,
        cached_origin: Option<&str>,
    ) -> Result<Url, url::ParseError> {
        let url = url::Url::options().base_url(Some(base)).parse(&self.0)?;
        let origin = cached_origin
            .filter(|_| {
                url.scheme() == base.scheme()
                    && url.host() == base.host()
                    && url.port() == base.port()
            })
            .map(str::to_owned)
            .unwrap_or_else(|| url.origin().unicode_serialization());

        let search_params = url
            .query_pairs()
            .map(|(k, v)| (k.into_owned(), v.into_owned()))
            .collect::<ParamsMap>();

        Ok(Url {
            origin,
            path: url.path().to_string(),
            search: url.query().unwrap_or_default().to_string(),
            search_params,
            hash: Default::default(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::RequestUrl;

    #[test]
    fn cached_base_preserves_url_resolution_and_query_decoding() {
        for input in [
            "/",
            "/products/42",
            "/a/../b?x=1&x=2",
            "relative/path",
            "//example.com:8080/a",
            "https://LEPTOS.dev:443/a",
            "http://leptos.dev/a",
            "https://user:pass@leptos.dev/a",
            "https://xn--bcher-kva.example/a",
            "https://[::1]:443/a",
            "/a%2Fb?name=hello+world&value=%E9%9B%AA&empty=",
            "?sort=price#section",
            "data:text/plain,hello",
            "http://[invalid",
        ] {
            let request = RequestUrl::new(input);
            assert_eq!(
                request.parse(),
                request.parse_with_base(super::BASE),
                "{input}"
            );
        }
        let request = RequestUrl::new("../next?q=one+two");
        assert_eq!(
            request
                .parse_with_base("https://example.com/base/child/")
                .unwrap()
                .path(),
            "/base/next"
        );
        assert!(request.parse_with_base("not a base URL").is_err());
    }

    #[test]
    pub fn should_parse_url_without_origin() {
        let url = RequestUrl::new("/foo/bar").parse().unwrap();
        assert_eq!(url.path(), "/foo/bar");
    }

    #[test]
    pub fn should_not_parse_url_without_slash() {
        let url = RequestUrl::new("foo/bar").parse().unwrap();
        assert_eq!(url.path(), "/foo/bar");
    }

    #[test]
    pub fn should_parse_with_base() {
        let url = RequestUrl::new("https://www.example.com/foo/bar")
            .parse()
            .unwrap();
        assert_eq!(url.origin(), "https://www.example.com");
        assert_eq!(url.path(), "/foo/bar");
    }
}
