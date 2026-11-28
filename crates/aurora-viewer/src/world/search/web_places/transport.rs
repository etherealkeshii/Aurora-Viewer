//! HTTP transport for the main grid's Places search. A client belongs to one login.

use super::{Page, Sort, parse, selector};
use crate::world::search::SearchGrid;
use reqwest::{Url, cookie::Jar, redirect::Policy};
use std::{sync::Arc, time::Duration};
use uuid::Uuid;

const ENDPOINT: &str = "https://search.secondlife.com/viewer/";
const MAX_RESPONSE: usize = 4 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Maturity {
    General,
    GeneralModerate,
    All,
    Adult,
}

impl Maturity {
    fn parameter(self) -> &'static str {
        match self {
            Self::General => "g",
            Self::GeneralModerate => "gm",
            Self::All => "gma",
            Self::Adult => "a",
        }
    }

    fn allows(self, rating: char) -> bool {
        self.parameter().contains(rating)
    }
}

pub struct Request<'a> {
    pub query: &'a str,
    pub category: &'a str,
    pub maturity: Maturity,
    pub sort: Sort,
    pub page: usize,
}

fn request_url(request: &Request<'_>, session: Option<Uuid>) -> Result<Url, &'static str> {
    if request.page == 0 {
        return Err("invalid Places page");
    }
    let mut url = Url::parse(ENDPOINT).expect("static search URL");
    url.query_pairs_mut()
        .append_pair("query_term", request.query)
        .append_pair("search_type", "standard")
        .append_pair("collection_chosen", "places")
        .append_pair("places_category", request.category)
        .append_pair("maturity", request.maturity.parameter())
        .append_pair("lang", "en")
        .append_pair("sort", request.sort.parameter())
        .append_pair("page", &request.page.to_string());
    if let Some(id) = session {
        url.query_pairs_mut().append_pair("sid", &id.to_string());
    }
    Ok(url)
}

fn trusted_host(url: &Url) -> bool {
    url.scheme() == "https"
        && url.username().is_empty()
        && url.password().is_none()
        && url.port().is_none()
        && matches!(url.host_str(), Some("search.secondlife.com" | "id.secondlife.com"))
}

fn search_host(url: &Url) -> bool {
    trusted_host(url) && url.host_str() == Some("search.secondlife.com")
}

fn redirect_allowed(url: &Url, session: Option<Uuid>) -> bool {
    if !trusted_host(url) {
        return false;
    }
    if search_host(url) {
        return true;
    }
    // The identity service needs its own cookie, not the viewer session ID.
    !session.is_some_and(|id| {
        let address = url.as_str().to_ascii_lowercase();
        address.contains(&id.to_string()) || address.contains(&id.simple().to_string())
    }) && !url.query_pairs().any(|(key, _)| key.eq_ignore_ascii_case("sid"))
}

#[derive(Clone)]
pub struct Client {
    http: reqwest::Client,
    session: Option<Uuid>,
}

impl Client {
    /// Create a fresh cookie jar for one main-grid login. Never share it with
    /// the generic HTTP client or with another login/grid.
    pub fn new(grid: SearchGrid, session: Option<Uuid>, cookie: Option<&aurora_media::Cookie>) -> Result<Self, &'static str> {
        if grid != SearchGrid::Main {
            return Err("Places web search is only available on the main grid");
        }
        let jar = Arc::new(Jar::default());
        if let Some(cookie) = cookie.filter(|_| session.is_some()) {
            let origin = Url::parse(&cookie.uri).map_err(|_| "invalid OpenID cookie origin")?;
            if !trusted_host(&origin) || origin.host_str() != Some(cookie.domain.as_str()) || !cookie.secure {
                return Err("untrusted OpenID cookie origin");
            }
            if cookie.name.is_empty() || cookie.name.contains(['=', ';', '\r', '\n']) || cookie.value.contains([';', '\r', '\n']) {
                return Err("invalid OpenID cookie");
            }
            // Host-only cookie: do not widen the domain or copy it into a
            // general-purpose Cookie header that might follow a redirect.
            jar.add_cookie_str(&format!("{}={}; Path=/; Secure; HttpOnly", cookie.name, cookie.value), &origin);
        }
        let http = aurora_proxy::http_client(|b| {
            b.cookie_provider(jar)
                .referer(false)
                .timeout(Duration::from_secs(30))
                .redirect(Policy::custom(move |attempt| {
                    if attempt.previous().len() >= 20 || !redirect_allowed(attempt.url(), session) {
                        attempt.error("Places redirect refused")
                    } else {
                        attempt.follow()
                    }
                }))
        })
        .map_err(|_| "could not initialise Places HTTP client")?;
        Ok(Self { http, session })
    }

    pub async fn fetch(&self, request: &Request<'_>) -> Result<Page, &'static str> {
        if request.maturity.allows('a') && self.session.is_none() {
            return Err("Adult Places search requires authentication");
        }
        let url = request_url(request, self.session)?;
        let mut response = self.http.get(url).send().await.map_err(|_| "Places HTTP request failed")?;
        if !response.status().is_success() || !search_host(response.url()) {
            return Err("Places HTTP response refused");
        }
        let mut body = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(|_| "Places response interrupted")? {
            if chunk.len() > MAX_RESPONSE - body.len() {
                return Err("Places response too large");
            }
            body.extend_from_slice(&chunk);
        }
        let html = std::str::from_utf8(&body).map_err(|_| "Places response is not UTF-8")?;
        parse_checked(html, request.maturity)
    }
}

fn parse_checked(html: &str, maturity: Maturity) -> Result<Page, &'static str> {
    let document = scraper::Html::parse_document(html);
    let selected = document.select(&selector("select[name='maturity'] option[selected]")).next();
    if !selected
        .is_some_and(|option| option.value().attr("value") == Some(maturity.parameter()) && option.value().attr("disabled").is_none())
    {
        return Err("Places server did not grant requested maturity");
    }
    let page = parse(html)?;
    if page.places.iter().any(|place| !maturity.allows(place.maturity)) {
        return Err("Places response contains an unexpected maturity");
    }
    Ok(page)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_encodes_keywords_and_preserves_sort_and_page() {
        let request = Request {
            query: "日本語 & x",
            category: "shopping",
            maturity: Maturity::GeneralModerate,
            sort: Sort::NameDescending,
            page: 2,
        };
        let url = request_url(&request, None).unwrap();
        let pairs: std::collections::HashMap<_, _> = url.query_pairs().collect();
        assert_eq!(pairs["query_term"], request.query);
        assert_eq!(pairs["places_category"], "shopping");
        assert_eq!(pairs["maturity"], "gm");
        assert_eq!(pairs["sort"], "parcel_name_desc");
        assert_eq!(pairs["page"], "2");
        assert!(!pairs.contains_key("sid"));
    }

    #[test]
    fn refuses_other_grids_and_untrusted_redirects() {
        assert!(Client::new(SearchGrid::Beta, None, None).is_err());
        assert!(Client::new(SearchGrid::Other, None, None).is_err());
        let id = Uuid::from_u128(1);
        for url in [
            "http://search.secondlife.com/",
            "https://search.secondlife.com.example.org/",
            "https://other.example.org/",
            "https://user@id.secondlife.com/",
            "https://id.secondlife.com/?sid=secret",
        ] {
            assert!(!redirect_allowed(&Url::parse(url).unwrap(), Some(id)));
        }
        assert!(!redirect_allowed(
            &Url::parse(&format!("https://id.secondlife.com/?return_to={id}")).unwrap(),
            Some(id)
        ));
        assert!(redirect_allowed(
            &Url::parse("https://id.secondlife.com/openid/checklogin").unwrap(),
            Some(id)
        ));
        assert!(redirect_allowed(&Url::parse(ENDPOINT).unwrap(), Some(id)));
    }

    #[test]
    fn does_not_accept_a_downgraded_or_disabled_maturity() {
        let page = |option: &str| format!("<select name='maturity'>{option}</select><h2 class='role_results_counter'>0 Results</h2>");
        assert!(parse_checked(&page("<option selected value='gm'>GM</option>"), Maturity::All).is_err());
        assert!(parse_checked(&page("<option selected disabled value='gma'>GMA</option>"), Maturity::All).is_err());
        assert_eq!(
            parse_checked(&page("<option selected value='gm'>GM</option>"), Maturity::GeneralModerate)
                .unwrap()
                .total,
            0
        );
    }
}
