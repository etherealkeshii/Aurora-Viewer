//! Native representation of the Second Life web Places search response.
//! The service determines the result order; no local ranking is applied.

use scraper::{ElementRef, Html, Selector};
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sort {
    Relevance,
    TrafficDescending,
    NameAscending,
    NameDescending,
}

impl Sort {
    pub fn parameter(self) -> &'static str {
        match self {
            Self::Relevance => "",
            Self::TrafficDescending => "popularity_desc",
            Self::NameAscending => "parcel_name_asc",
            Self::NameDescending => "parcel_name_desc",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Parcel,
    Region,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Place {
    pub id: Uuid,
    pub index: usize,
    pub kind: Kind,
    pub name: String,
    pub description: Option<String>,
    pub teleport: String,
    pub traffic: Option<u64>,
    pub maturity: char,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Page {
    pub total: usize,
    pub next: Option<usize>,
    pub places: Vec<Place>,
}

fn selector(css: &str) -> Selector {
    Selector::parse(css).expect("static Places selector")
}

fn text(element: ElementRef<'_>) -> String {
    element.text().collect::<String>().trim().to_owned()
}

fn parse_place(element: ElementRef<'_>) -> Result<Place, &'static str> {
    let attr = element.value();
    let id = attr
        .attr("data-result-id")
        .and_then(|s| Uuid::parse_str(s).ok())
        .ok_or("invalid result id")?;
    let index = attr
        .attr("data-result-index")
        .and_then(|s| s.parse::<usize>().ok())
        .filter(|i| *i > 0)
        .ok_or("invalid result index")?;
    let kind = match attr.attr("data-result-type") {
        Some("Parcel") => Kind::Parcel,
        Some("Region") => Kind::Region,
        _ => return Err("unknown Places result type"),
    };
    let name = element
        .select(&selector("h2"))
        .next()
        .map(text)
        .filter(|s| !s.is_empty())
        .ok_or("missing result name")?;
    let description = element.select(&selector(".search-result__content__description")).next().map(text);
    let teleport = element
        .select(&selector("a[href^='secondlife:///app/teleport/']"))
        .next()
        .and_then(|a| a.value().attr("href"))
        .ok_or("missing teleport link")?
        .to_owned();
    let destination = teleport
        .strip_prefix("secondlife:///app/teleport/")
        .ok_or("invalid teleport link")?;
    let mut parts = destination.split('/');
    let region = parts.next().filter(|s| !s.is_empty()).ok_or("missing teleport region")?;
    if region.contains(['?', '#'])
        || ![256.0_f32, 256.0, 4096.0].into_iter().all(|limit| {
            parts
                .next()
                .and_then(|s| s.parse::<f32>().ok())
                .is_some_and(|value| value.is_finite() && value >= 0.0 && value <= limit)
        })
        || parts.next().is_some()
        || crate::slurl::parse(&teleport).is_none()
    {
        return Err("invalid teleport location");
    }
    let traffic = element
        .select(&selector("p.inline-block"))
        .find_map(|p| {
            let label = p.select(&selector("span.font-bold")).next().map(text)?;
            if label.trim_end_matches(':').trim() != "Traffic" {
                return None;
            }
            let value = text(p);
            Some(value.split_once(':').and_then(|(_, s)| s.trim().parse::<u64>().ok()))
        })
        .flatten();
    let maturity = element
        .select(&selector("span.text-xs.rounded"))
        .find_map(|span| match text(span).as_str() {
            "G" => Some('g'),
            "M" => Some('m'),
            "A" => Some('a'),
            _ => None,
        })
        .ok_or("missing result maturity")?;
    Ok(Place {
        id,
        index,
        kind,
        name,
        description,
        teleport,
        traffic,
        maturity,
    })
}

pub fn parse(html: &str) -> Result<Page, &'static str> {
    let document = Html::parse_document(html);
    let total = document
        .select(&selector(".role_results_counter"))
        .next()
        .map(text)
        .and_then(|s| {
            let mut words = s.split_whitespace();
            let total = words.next()?.replace(',', "").parse::<usize>().ok()?;
            matches!(words.next(), Some("Result" | "Results")).then_some(total)
        })
        .ok_or("missing Places result count")?;
    let places = document
        .select(&selector(".search-result"))
        .map(parse_place)
        .collect::<Result<Vec<_>, _>>()?;
    if places.len() > total || (total > 0 && places.is_empty()) || places.windows(2).any(|pair| pair[0].index >= pair[1].index) {
        return Err("inconsistent Places results");
    }
    let next = document
        .select(&selector(".div_role_pagination a[href]"))
        .find(|a| text(*a) == "Next")
        .map(|a| {
            let url = reqwest::Url::parse(a.value().attr("href").ok_or("missing next link")?).map_err(|_| "invalid next link")?;
            url.query_pairs()
                .find(|(key, _)| key == "page")
                .and_then(|(_, value)| value.parse::<usize>().ok())
                .filter(|page| *page > 1)
                .ok_or("invalid next page")
        })
        .transpose()?;
    Ok(Page { total, next, places })
}

#[cfg(test)]
mod tests {
    use super::*;

    const RESPONSE: &str = r#"
        <h2 class="role_results_counter">21 Results</h2>
        <div class="search-result" data-result-id="00000000-0000-4000-8000-000000000001" data-result-index="1" data-result-type="Parcel">
          <h2>Example <b>Shop</b> &amp; Gallery</h2>
          <p class="search-result__content__description">日本語 &amp; text</p>
          <a href="secondlife:///app/teleport/Example%20Region/10/20/300">Teleport</a>
          <p class="inline-block"><span class="font-bold">Traffic :</span> 0</p>
          <span class="text-xs rounded">M</span>
        </div>
        <div class="search-result" data-result-id="00000000-0000-4000-8000-000000000002" data-result-index="2" data-result-type="Region">
          <h2>Example Region</h2>
          <a href="secondlife:///app/teleport/Example%20Region/10/20/30">Teleport</a>
          <span class="text-xs rounded">G</span>
        </div>
        <div class="search-result" data-result-id="00000000-0000-4000-8000-000000000003" data-result-index="3" data-result-type="Parcel">
          <h2>Another Parcel</h2>
          <a href="secondlife:///app/teleport/Example%20Region/10/20/30">Teleport</a>
          <p class="inline-block"><span class="font-bold">Traffic :</span> 9000</p>
          <span class="text-xs rounded">A</span>
        </div>
        <div class="div_role_pagination"><a>1</a><a href="https://search.secondlife.com/viewer/?page=2&amp;sort=">Next</a></div>
    "#;

    #[test]
    fn preserves_server_order_and_decodes_markup() {
        let page = parse(RESPONSE).unwrap();
        assert_eq!(page.total, 21);
        assert_eq!(page.next, Some(2));
        assert_eq!(page.places.len(), 3);
        assert_eq!(page.places[0].name, "Example Shop & Gallery");
        assert_eq!(page.places[0].description.as_deref(), Some("日本語 & text"));
        assert_eq!(page.places[0].traffic, Some(0));
        assert_eq!(page.places[0].maturity, 'm');
        assert_eq!(page.places[0].teleport, "secondlife:///app/teleport/Example%20Region/10/20/300");
        assert_eq!(page.places[1].kind, Kind::Region);
        assert_eq!(page.places[1].traffic, None);
        assert_eq!(page.places[2].traffic, Some(9000));
        assert_eq!(page.places[2].maturity, 'a');
    }

    #[test]
    fn rejects_unrecognised_or_inconsistent_responses() {
        assert!(parse("<html>Log in</html>").is_err());
        assert!(parse(&RESPONSE.replace("data-result-index=\"2\"", "data-result-index=\"1\"")).is_err());
        assert!(parse(&RESPONSE.replace("data-result-type=\"Region\"", "data-result-type=\"Unknown\"")).is_err());
        assert_eq!(parse("<h2 class='role_results_counter'>0 Results</h2>").unwrap().places.len(), 0);
    }

    #[test]
    fn sort_parameters_match_the_service() {
        assert_eq!(Sort::Relevance.parameter(), "");
        assert_eq!(Sort::TrafficDescending.parameter(), "popularity_desc");
        assert_eq!(Sort::NameAscending.parameter(), "parcel_name_asc");
        assert_eq!(Sort::NameDescending.parameter(), "parcel_name_desc");
    }
}

mod transport;
pub use transport::{Client, Maturity, Request};
