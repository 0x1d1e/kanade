//! The video meeting an event links to, for a reminder's Join action: the first `https` link in the
//! event's URL, LOCATION or DESCRIPTION, in that order, whose host is one of `HOSTS`. The allowlist
//! is this one place; anything else in an event, a link to a phishing page or an app's own scheme
//! included, is never opened from a reminder. A link is another program's text, so it is never
//! logged.

// a host that is one of these or a subdomain of one; a meeting service Kanade opens without asking
const HOSTS: [&str; 8] = [
    "meet.google.com",
    "zoom.us",
    "zoomgov.com",
    "teams.microsoft.com",
    "teams.live.com",
    "webex.com",
    "meet.jit.si",
    "whereby.com",
];

const SCHEME: &str = "https://";

// a link is at most this long, as a browser takes it
const LONGEST: usize = 2048;

// the first allowed link of `texts`, in order, each searched from its start
pub fn link(texts: &[&str]) -> Option<String> {
    texts.iter().find_map(|text| find(text))
}

fn find(text: &str) -> Option<String> {
    let lower = text.to_ascii_lowercase();

    lower.match_indices(SCHEME).find_map(|(start, _)| {
        let rest = &text[start..];
        let end = rest
            .find(|letter: char| {
                letter.is_whitespace() || letter.is_control() || "<>\"'`\\|^{}".contains(letter)
            })
            .unwrap_or(rest.len());
        let url = rest[..end].trim_end_matches(['.', ',', ';', ':', '!', '?', ')', ']']);

        allowed(url)
    })
}

// `url` with its scheme in lower case, when it is a plain `https` link to an allowed host
fn allowed(url: &str) -> Option<String> {
    let rest = url.get(SCHEME.len()..)?;

    if url.len() > LONGEST || !url.bytes().all(|byte| byte.is_ascii_graphic()) {
        return None;
    }

    let host = &rest[..rest.find(['/', '?', '#']).unwrap_or(rest.len())];

    // no user, which would make the real host another, and no port
    if host.contains(['@', ':']) {
        return None;
    }

    let host = host.to_ascii_lowercase();
    let listed = HOSTS.iter().any(|listed| {
        host == *listed
            || host
                .strip_suffix(listed)
                .is_some_and(|before| before.ends_with('.'))
    });

    listed.then(|| format!("{SCHEME}{rest}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    // what a reminder opens comes from a calendar anyone can send an invitation to
    #[test]
    fn only_an_https_link_to_a_listed_host_is_a_meeting() {
        for text in [
            "https://meet.google.com/abc-defg-hij",
            "Join: https://us02web.zoom.us/j/123?pwd=x.",
            "<a href=\"https://teams.microsoft.com/l/meetup-join/19%3a\">Join</a>",
            "HTTPS://Meet.Google.com/abc (via Google)",
            "see (https://zoom.us/j/1), thanks",
        ] {
            assert!(link(&[text]).is_some(), "{text}");
        }

        for text in [
            "http://meet.google.com/abc",
            "https://meet.google.com.evil.example/abc",
            "https://evilzoom.us/j/1",
            "https://meet.google.com@evil.example/abc",
            "https://meet.google.com:8443/abc",
            "https://zoom.us.example/",
            "https://ex\u{e4}mple.zoom.us/j/1",
            "zoommtg://zoom.us/join?confno=1",
            "meet.google.com/abc",
            "",
        ] {
            assert_eq!(link(&[text]), None, "{text}");
        }
    }

    // a link in a later field is used only when an earlier one has none, and it is cut where it ends
    #[test]
    fn the_first_field_with_a_meeting_gives_it_whole() {
        assert_eq!(
            link(&[
                "https://example.com/a",
                "no link",
                "x https://zoom.us/j/1?pwd=A_b-c#d>"
            ]),
            Some(String::from("https://zoom.us/j/1?pwd=A_b-c#d"))
        );
        assert_eq!(
            link(&[
                "https://example.com/a https://meet.google.com/a",
                "https://zoom.us/j/2"
            ]),
            Some(String::from("https://meet.google.com/a"))
        );
    }
}
