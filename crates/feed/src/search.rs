//! Searches of the USGS FDSN event service for past events: by date range and size, or one
//! event by its USGS id. Pure: builds the query URL, no I/O.

use chrono::{Days, NaiveDate};

const FDSN: &str = "https://earthquake.usgs.gov/fdsnws/event/1/query";

/// Most results one search may ask for.
pub const MAX_RESULTS: u32 = 200;

/// A search for past events.
#[derive(Debug, Clone, PartialEq)]
pub enum Search {
    /// Events from the start of `from` to the end of `to` (UTC days, both included) of at
    /// least `min_mag`, largest first, at most `limit` of them.
    Range {
        from: NaiveDate,
        to: NaiveDate,
        min_mag: f64,
        limit: u32,
    },
    /// One event, by any of its USGS ids (e.g. "usp000hvnu").
    Id(String),
}

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum SearchError {
    #[error("the start date is after the end date")]
    DatesReversed,
    #[error("a USGS event code is letters, digits and _ only")]
    BadId,
}

impl Search {
    /// The FDSN `format=geojson` query URL.
    pub fn url(&self) -> Result<String, SearchError> {
        match self {
            Search::Range {
                from,
                to,
                min_mag,
                limit,
            } => {
                if from > to {
                    return Err(SearchError::DatesReversed);
                }
                // FDSN `endtime` is an instant; the day after `to` includes all of `to`.
                let end = to.checked_add_days(Days::new(1)).unwrap_or(*to);
                let limit = (*limit).clamp(1, MAX_RESULTS);
                Ok(format!(
                    "{FDSN}?format=geojson&starttime={from}&endtime={end}\
                     &minmagnitude={min_mag:.1}&orderby=magnitude&limit={limit}"
                ))
            }
            Search::Id(id) => {
                let id = id.trim();
                let ok = !id.is_empty()
                    && id.len() <= 64
                    && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
                if !ok {
                    return Err(SearchError::BadId);
                }
                Ok(format!("{FDSN}?format=geojson&eventid={id}"))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn day(y: i32, m: u32, d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, d).unwrap()
    }

    #[test]
    fn range_url_includes_the_last_day() {
        let s = Search::Range {
            from: day(2011, 3, 1),
            to: day(2011, 3, 31),
            min_mag: 7.0,
            limit: 50,
        };
        assert_eq!(
            s.url().unwrap(),
            "https://earthquake.usgs.gov/fdsnws/event/1/query?format=geojson\
             &starttime=2011-03-01&endtime=2011-04-01&minmagnitude=7.0&orderby=magnitude&limit=50"
        );
    }

    #[test]
    fn range_checks() {
        let s = |from, to, limit| Search::Range {
            from,
            to,
            min_mag: 7.5,
            limit,
        };
        assert_eq!(
            s(day(2011, 3, 2), day(2011, 3, 1), 10).url(),
            Err(SearchError::DatesReversed)
        );
        // One day is fine; the limit is kept within what the app allows.
        let one = s(day(2011, 3, 11), day(2011, 3, 11), 10_000).url().unwrap();
        assert!(one.contains("starttime=2011-03-11&endtime=2011-03-12"));
        assert!(one.ends_with("&minmagnitude=7.5&orderby=magnitude&limit=200"));
        assert!(
            s(day(2011, 3, 11), day(2011, 3, 11), 0)
                .url()
                .unwrap()
                .ends_with("&limit=1")
        );
    }

    #[test]
    fn id_url_rejects_anything_but_a_code() {
        assert_eq!(
            Search::Id(" usp000hvnu ".into()).url().unwrap(),
            "https://earthquake.usgs.gov/fdsnws/event/1/query?format=geojson&eventid=usp000hvnu"
        );
        assert!(
            Search::Id("official20110311054624120_30".into())
                .url()
                .is_ok()
        );
        for bad in ["", "  ", "us&limit=5", "a b", "ü1"] {
            assert_eq!(
                Search::Id(bad.into()).url(),
                Err(SearchError::BadId),
                "{bad:?}"
            );
        }
    }
}
