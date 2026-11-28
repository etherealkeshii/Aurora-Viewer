//! Main-grid Places queries use the service's own sort and page order.
use super::{Key, List, Row, RowIcon, Search, Status, Tab, Value, web_places};
use aurora_net::search::{AgentAccess, PLACE_CATEGORY_ANY, Ratings};
use tokio::{runtime::Handle, sync::oneshot, task::AbortHandle};
use uuid::Uuid;
use web_places::{Client, Kind, Maturity, Page, Request, Sort};

#[derive(Clone)]
struct Query {
    text: String,
    category: &'static str,
    maturity: Maturity,
}

struct Pending {
    query: Query,
    sort: Sort,
    page: usize,
}

type Answer = Result<Page, &'static str>;

struct Flight {
    rx: oneshot::Receiver<Answer>,
    abort: AbortHandle,
}

impl Drop for Flight {
    fn drop(&mut self) {
        self.abort.abort();
    }
}

pub(super) struct State {
    session: Option<Uuid>,
    client: Option<Client>,
    has_cookie: bool,
    query: Option<Query>,
    sort: Sort,
    next: Option<usize>,
    pending: Option<Pending>,
    flight: Option<Flight>,
}

impl Default for State {
    fn default() -> Self {
        Self {
            session: None,
            client: None,
            has_cookie: false,
            query: None,
            sort: Sort::Relevance,
            next: None,
            pending: None,
            flight: None,
        }
    }
}

impl State {
    pub(super) fn enabled(&self) -> bool {
        self.session.is_some()
    }
}

pub fn category_parameter(category: i8) -> Option<&'static str> {
    Some(match category {
        PLACE_CATEGORY_ANY => "",
        1 => "linden",
        3 => "arts",
        4 => "store",
        5 => "educational",
        6 => "game",
        7 => "gather",
        8 => "newcomer",
        9 => "park",
        10 => "home",
        11 => "shopping",
        13 => "other",
        _ => return None,
    })
}

fn maturity(ratings: Ratings, access: AgentAccess) -> Option<Maturity> {
    match (
        ratings.pg,
        ratings.mature && access.can_access_mature(),
        ratings.adult && access.can_access_adult(),
    ) {
        (true, false, false) => Some(Maturity::General),
        (true, true, false) => Some(Maturity::GeneralModerate),
        (true, true, true) => Some(Maturity::All),
        (false, false, true) => Some(Maturity::Adult),
        _ => None,
    }
}

impl Search {
    pub fn web_places_enabled(&self) -> bool {
        self.web.enabled()
    }

    pub fn web_places_sort(&self) -> Sort {
        self.web.sort
    }

    pub fn set_web_places_session(&mut self, session: Option<Uuid>) {
        if self.web.session == session {
            return;
        }
        self.web = State {
            session,
            ..State::default()
        };
        self.lists[Tab::Places.list_index().unwrap()] = List::new(Tab::Places);
        self.out.retain(|command| {
            !matches!(
                command,
                aurora_net::NetCommand::DirQuery {
                    query: aurora_net::search::DirQuery::Places { .. },
                    ..
                }
            )
        });
    }

    pub fn sort_web_places(&mut self, sort: Sort) {
        if !self.web.enabled() || self.web.sort == sort {
            return;
        }
        self.web.sort = sort;
        if self.web.query.is_some() {
            self.begin_web_page(1);
        }
    }

    pub fn sort_web_column(&mut self, column: usize) {
        let sort = match (column, self.web.sort) {
            (0, Sort::NameAscending) => Sort::NameDescending,
            (0, _) => Sort::NameAscending,
            (1, _) => Sort::TrafficDescending,
            _ => return,
        };
        self.sort_web_places(sort);
    }

    pub(super) fn find_web_places(&mut self, text: &str, ratings: Ratings, category: i8, access: AgentAccess) {
        self.web.query = None;
        self.web.pending = None;
        self.web.flight = None;
        self.web.next = None;
        let list = &mut self.lists[Tab::Places.list_index().unwrap()];
        *list = List::new(Tab::Places);
        list.text = text.trim().to_owned();
        list.web_page = Some(1);
        if list.text.is_empty() {
            list.status = Status::NoQuery;
            return;
        }
        if !ratings.any() {
            list.status = Status::NoContent;
            return;
        }
        let Some(maturity) = maturity(ratings, access) else {
            list.status = Status::WebPlacesMaturity;
            return;
        };
        let Some(category) = category_parameter(category) else {
            list.status = Status::WebPlacesCategory;
            return;
        };
        self.web.query = Some(Query {
            text: list.text.clone(),
            category,
            maturity,
        });
        self.begin_web_page(1);
    }

    fn begin_web_page(&mut self, page: usize) {
        let Some(query) = self.web.query.clone() else {
            return;
        };
        self.web.flight = None;
        self.web.next = None;
        self.web.pending = Some(Pending {
            query: query.clone(),
            sort: self.web.sort,
            page,
        });
        let list = &mut self.lists[Tab::Places.list_index().unwrap()];
        *list = List::new(Tab::Places);
        list.text = query.text;
        list.web_page = Some(page);
        list.sort = match self.web.sort {
            Sort::Relevance => None,
            Sort::NameAscending => Some((0, false)),
            Sort::NameDescending => Some((0, true)),
            Sort::TrafficDescending => Some((1, true)),
        };
        list.status = Status::Searching;
    }

    pub(super) fn turn_web_page(&mut self, forward: bool) {
        let list = &self.lists[Tab::Places.list_index().unwrap()];
        if list.status == Status::Searching {
            return;
        }
        let target = if forward {
            self.web.next
        } else {
            list.web_page.filter(|p| *p > 1).map(|p| p - 1)
        };
        if let Some(page) = target {
            self.begin_web_page(page);
        }
    }

    /// Called on the UI thread; HTTP and HTML parsing run on the network runtime.
    pub fn poll_web_places(&mut self, runtime: &Handle, cookie: Option<&aurora_media::Cookie>) {
        if let Some(pending) = self.web.pending.take() {
            if self.web.client.is_none() || (cookie.is_some() && !self.web.has_cookie) {
                match Client::new(super::SearchGrid::Main, self.web.session, cookie) {
                    Ok(client) => {
                        self.web.client = Some(client);
                        self.web.has_cookie = cookie.is_some();
                    }
                    Err(_) => {
                        self.finish_web_page(Err("client"));
                        return;
                    }
                }
            }
            let client = self.web.client.as_ref().unwrap().clone();
            let (tx, rx) = oneshot::channel();
            let task = runtime.spawn(async move {
                let answer = client
                    .fetch(&Request {
                        query: &pending.query.text,
                        category: pending.query.category,
                        maturity: pending.query.maturity,
                        sort: pending.sort,
                        page: pending.page,
                    })
                    .await;
                let _ = tx.send(answer);
            });
            self.web.flight = Some(Flight {
                rx,
                abort: task.abort_handle(),
            });
        }
        let answer = match self.web.flight.as_mut().map(|f| f.rx.try_recv()) {
            Some(Ok(answer)) => answer,
            Some(Err(oneshot::error::TryRecvError::Closed)) => Err("request cancelled"),
            _ => return,
        };
        self.web.flight = None;
        self.finish_web_page(answer);
    }

    fn finish_web_page(&mut self, answer: Answer) {
        let list = &mut self.lists[Tab::Places.list_index().unwrap()];
        let Ok(page) = answer else {
            list.status = Status::WebPlacesFailed;
            return;
        };
        let current = list.page();
        if page.next.is_some_and(|next| current.checked_add(1) != Some(next)) {
            list.status = Status::WebPlacesFailed;
            return;
        }
        self.web.next = page.next;
        list.has_next = page.next.is_some();
        list.web_total = Some(page.total);
        for place in page.places {
            let key = match place.kind {
                Kind::Parcel => Key::Id(place.id),
                Kind::Region => Key::Region(place.id),
            };
            list.rows.push(Row {
                key,
                name: place.name.clone(),
                icon: RowIcon::Place,
                values: vec![place.traffic.and_then(|n| i64::try_from(n).ok()).map_or(Value::None, Value::Int)],
            });
            list.web_results.insert(key, place);
        }
        list.status = if list.rows.is_empty() {
            Status::NotFound(list.text.clone())
        } else {
            Status::Found
        };
        if let Some(key) = list.rows.first().map(|row| row.key) {
            self.select(Tab::Places, key);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use web_places::Place;

    #[test]
    fn web_places_server_order_and_page_changes() {
        let mut search = Search::default();
        search.set_web_places_session(Some(Uuid::from_u128(1)));
        search.find_web_places("example", Ratings::default(), PLACE_CATEGORY_ANY, AgentAccess::default());
        let place = |id, name: &str, kind| Place {
            id: Uuid::from_u128(id),
            index: id as usize,
            kind,
            name: name.to_owned(),
            description: None,
            teleport: "secondlife:///app/teleport/Example/1/2/3".into(),
            traffic: Some(id as u64),
            maturity: 'g',
        };
        search.web.pending = None;
        search.finish_web_page(Ok(Page {
            total: 21,
            next: Some(2),
            places: vec![place(2, "Z", Kind::Region), place(3, "A", Kind::Parcel)],
        }));
        let list = search.list(Tab::Places).unwrap();
        assert_eq!(list.sorted()[0].name, "Z");
        assert_eq!(list.selected, Some(Key::Region(Uuid::from_u128(2))));
        assert!(search.take_commands().is_empty());
        search.turn_page(Tab::Places, true);
        assert_eq!(search.web.pending.as_ref().unwrap().page, 2);
        search.sort_web_places(Sort::NameAscending);
        let pending = search.web.pending.as_ref().unwrap();
        assert_eq!(pending.page, 1);
        assert_eq!(pending.sort, Sort::NameAscending);
        assert!(search.list(Tab::Places).unwrap().rows.is_empty());
        search.set_web_places_session(None);
        assert!(search.web.pending.is_none());
        assert!(!search.web_places_enabled());
    }

    #[test]
    fn web_places_traffic_header_keeps_descending_order() {
        let mut search = Search::default();
        search.set_web_places_session(Some(Uuid::from_u128(1)));
        search.find_web_places("example", Ratings::default(), PLACE_CATEGORY_ANY, AgentAccess::default());
        assert_eq!(search.web_places_sort(), Sort::Relevance);
        search.sort_web_column(1);
        assert_eq!(search.web.pending.as_ref().unwrap().sort, Sort::TrafficDescending);
        search.sort_web_column(1);
        assert_eq!(search.web_places_sort(), Sort::TrafficDescending);
        search.sort_web_places(Sort::Relevance);
        assert_eq!(search.web.pending.as_ref().unwrap().sort, Sort::Relevance);
    }

    #[test]
    fn web_places_never_expands_requested_maturity_or_category() {
        let access = AgentAccess { max: 42, preferred: 42 };
        assert_eq!(
            maturity(
                Ratings {
                    pg: false,
                    mature: true,
                    adult: false
                },
                access
            ),
            None
        );
        assert_eq!(
            maturity(
                Ratings {
                    pg: true,
                    mature: true,
                    adult: true
                },
                access
            ),
            Some(Maturity::All)
        );
        assert_eq!(category_parameter(4), Some("store"));
        assert_eq!(category_parameter(14), None);
    }
}
