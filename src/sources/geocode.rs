//! Finding the Weather location without coordinates (ADR 0136): a place name through Open-Meteo's
//! geocoding, or the approximate place of the machine's IP address through ipwho.is. Both are
//! blocking one-shot requests that Settings runs on a worker when the user presses Find or Use my
//! location, never on a timer and never at start: the IP address leaves the machine only for the
//! second, and only because the user asked. What was typed and what came back are never logged.

use std::time::Duration;

use serde_json::Value;
use ureq::Agent;

use super::body;
use super::weather::Offline;
use crate::config::Location;

const NAMES: &str = "https://geocoding-api.open-meteo.com/v1/search";
const ADDRESS: &str = "https://ipwho.is/";

// the places asked for, which Settings lists to pick from
const COUNT: usize = 8;

// the longest text sent; a place name is far shorter
const LONGEST: usize = 100;

const PATIENCE: Duration = Duration::from_secs(15);
const BODY: u64 = 256 * 1024;

// a place, as the user reads it
#[derive(Debug, Clone, PartialEq)]
pub struct Place {
    pub name: String,

    // the first administrative level: a state, a province
    pub region: Option<String>,
    pub country: Option<String>,
    pub location: Location,
}

impl Place {
    // "Name, Region, Country", leaving out a region that only repeats the name
    pub fn label(&self) -> String {
        let mut parts = vec![self.name.as_str()];

        if let Some(region) = self.region.as_deref().filter(|&region| region != self.name) {
            parts.push(region);
        }
        if let Some(country) = self.country.as_deref() {
            parts.push(country);
        }

        parts.join(", ")
    }
}

// which of the two services was asked, for the words of an error
#[derive(Clone, Copy)]
enum Service {
    Names,
    Address,
}

impl Service {
    fn host(self) -> &'static str {
        match self {
            Service::Names => "Open-Meteo's place search",
            Service::Address => "the location service (ipwho.is)",
        }
    }

    // what the user can still do
    fn instead(self) -> &'static str {
        match self {
            Service::Names => "or enter latitude and longitude below",
            Service::Address => "or search for a place by name",
        }
    }
}

/*
 * the places `text` names, "City" or "City, Country" (or region), best match first. The error is
 * a sentence that names the cause and what to do next
 */
pub fn search(text: &str) -> Result<Vec<Place>, String> {
    let text = text.trim();
    if text.chars().count() < 2 {
        return Err(String::from(
            "Type at least 2 letters: a city, or \u{201c}City, Country\u{201d}",
        ));
    }
    let text: String = text.chars().take(LONGEST).collect();

    let ask = |name: &str| {
        get(
            Service::Names,
            NAMES,
            &[
                ("name", name),
                ("count", &COUNT.to_string()),
                ("language", "en"),
                ("format", "json"),
            ],
        )
        .map(|body| places(&body))
    };

    let mut found = ask(&text)?;

    // "City, Typo" finds nothing as a whole: the city alone is still worth showing
    if found.is_empty()
        && let Some((city, _)) = text.split_once(',')
        && city.trim().chars().count() >= 2
    {
        found = ask(city.trim())?;
    }

    if found.is_empty() {
        return Err(format!(
            "No place matches \u{201c}{text}\u{201d}. Check the spelling, try the city alone, {}",
            Service::Names.instead()
        ));
    }

    Ok(found)
}

// where the machine's IP address is, to the city
pub fn locate() -> Result<Place, String> {
    let body = get(Service::Address, ADDRESS, &[])?;

    let unlocated = || {
        format!(
            "{} could not place this address, as a VPN or a private network can make it: {}",
            Service::Address.host(),
            Service::Address.instead()
        )
    };

    if body.get("success").and_then(Value::as_bool) == Some(false) {
        return Err(unlocated());
    }

    here(&body).ok_or_else(unlocated)
}

// the JSON answer of `url`, or why there is none
fn get(service: Service, url: &str, query: &[(&str, &str)]) -> Result<Value, String> {
    let agent: Agent = Agent::config_builder()
        .timeout_global(Some(PATIENCE))
        .http_status_as_error(false)
        .build()
        .into();

    let mut request = agent.get(url);
    for &(key, value) in query {
        request = request.query(key, value);
    }

    let offline = |error: ureq::Error| match error {
        ureq::Error::BodyExceedsLimit(_) => format!(
            "{} sent an answer that is too large. Try again later, {}",
            service.host(),
            service.instead()
        ),
        error => format!(
            "Could not reach {} ({}). Check the network connection and try again, {}",
            service.host(),
            Offline::of(&error).why(),
            service.instead()
        ),
    };

    let mut answer = request.call().map_err(offline)?;
    let status = answer.status().as_u16();
    let body: Value = body::text(answer.body_mut(), BODY)
        .map_err(offline)
        .map(|text| serde_json::from_str(&text).unwrap_or(Value::Null))?;

    if status != 200 {
        let reason = body
            .get("reason")
            .or_else(|| body.get("message"))
            .and_then(Value::as_str)
            .map_or_else(|| format!("status {status}"), str::to_owned);

        return Err(format!(
            "{} refused the request ({reason}). Try again later, {}",
            service.host(),
            service.instead()
        ));
    }

    Ok(body)
}

// the places in Open-Meteo's answer; one without a usable position is left out
fn places(body: &Value) -> Vec<Place> {
    let text = |place: &Value, key: &str| {
        place
            .get(key)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|text| !text.is_empty())
            .map(str::to_owned)
    };

    body.get("results")
        .and_then(Value::as_array)
        .map_or(&[][..], Vec::as_slice)
        .iter()
        .filter_map(|place| {
            Some(Place {
                name: text(place, "name")?,
                region: text(place, "admin1"),
                country: text(place, "country"),
                location: Location::new(
                    place.get("latitude")?.as_f64()?,
                    place.get("longitude")?.as_f64()?,
                )
                .ok()?,
            })
        })
        .take(COUNT)
        .collect()
}

// the place in ipwho.is's answer
fn here(body: &Value) -> Option<Place> {
    let text = |key: &str| {
        body.get(key)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|text| !text.is_empty())
            .map(str::to_owned)
    };
    let region = text("region");

    Some(Place {
        name: text("city").or_else(|| region.clone())?,
        region,
        country: text("country"),
        location: Location::new(
            body.get("latitude")?.as_f64()?,
            body.get("longitude")?.as_f64()?,
        )
        .ok()?,
    })
}

/*
 * which of `places` the text `typed` settles by itself: the only one, or, for "Name, Qualifier",
 * the only one of that name in that country or region. None when several fit, as "Paris" alone
 * does, so the user picks and a place is never chosen silently
 */
pub fn decisive(places: &[Place], typed: &str) -> Option<usize> {
    if let [_] = places {
        return Some(0);
    }

    let (name, qualifier) = typed.split_once(',')?;
    let (name, qualifier) = (name.trim().to_lowercase(), qualifier.trim().to_lowercase());
    if qualifier.is_empty() {
        return None;
    }

    let is = |text: &Option<String>| {
        text.as_deref()
            .is_some_and(|text| text.to_lowercase() == qualifier)
    };
    let mut fits = places.iter().enumerate().filter(|(_, place)| {
        place.name.to_lowercase() == name && (is(&place.country) || is(&place.region))
    });

    match (fits.next(), fits.next()) {
        (Some((index, _)), None) => Some(index),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn answer(text: &str) -> Value {
        serde_json::from_str(text).unwrap()
    }

    #[test]
    fn open_meteo_results_become_places() {
        let found = places(&answer(
            r#"{"results":[
                {"name":"Phnom Penh","latitude":11.56245,"longitude":104.91601,
                 "country":"Cambodia","admin1":"Phnom Penh"},
                {"name":"Nowhere","latitude":95.0,"longitude":0.0},
                {"name":"Paris","latitude":33.66,"longitude":-95.55,
                 "country":"United States","admin1":"Texas"}]}"#,
        ));

        // a latitude out of range is no place
        assert_eq!(found.len(), 2);
        assert_eq!(found[0].label(), "Phnom Penh, Cambodia");
        assert_eq!(found[1].label(), "Paris, Texas, United States");

        // no match is `{}`, not an error
        assert!(places(&answer("{}")).is_empty());
    }

    #[test]
    fn a_qualified_name_settles_only_when_one_place_fits() {
        let found = places(&answer(
            r#"{"results":[
                {"name":"Phnom Penh","latitude":11.5,"longitude":104.9,"country":"Cambodia","admin1":"Phnom Penh"},
                {"name":"Phnom Penh Thmei","latitude":11.6,"longitude":104.8,"country":"Cambodia"},
                {"name":"Paris","latitude":48.8,"longitude":2.3,"country":"France","admin1":"Ile-de-France"},
                {"name":"Paris","latitude":33.6,"longitude":-95.5,"country":"United States","admin1":"Texas"},
                {"name":"Paris","latitude":36.3,"longitude":-88.3,"country":"United States","admin1":"Tennessee"}]}"#,
        ));

        assert_eq!(decisive(&found, "Phnom Penh, Cambodia"), Some(0));
        assert_eq!(decisive(&found, "phnom penh , cambodia"), Some(0));
        assert_eq!(decisive(&found, "Paris, France"), Some(2));
        assert_eq!(decisive(&found, "Paris, Texas"), Some(3));

        // two Parises in the United States, and an unqualified name: the user picks
        assert_eq!(decisive(&found, "Paris, United States"), None);
        assert_eq!(decisive(&found, "Paris"), None);
        assert_eq!(decisive(&found[..1], "Phnom"), Some(0));
    }

    #[test]
    fn the_address_answer_needs_a_city_or_region_and_a_position() {
        let place = here(&answer(
            r#"{"success":true,"ip":"203.0.113.9","city":"Phnom Penh","region":"Phnom Penh",
                "country":"Cambodia","latitude":11.56,"longitude":104.91}"#,
        ))
        .unwrap();

        assert_eq!(place.label(), "Phnom Penh, Cambodia");
        assert!(!format!("{place:?}").contains("203.0.113.9"));
        assert!(here(&answer(r#"{"latitude":1.0,"longitude":2.0}"#)).is_none());
        assert!(here(&answer(r#"{"city":"X","latitude":1.0}"#)).is_none());
    }
}
