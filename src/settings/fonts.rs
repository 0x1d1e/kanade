use fontconfig::{self, FC_FAMILY, Fontconfig, ObjectSet, Pattern};

pub fn installed() -> Result<Vec<String>, String> {
    let fontconfig = Fontconfig::new().ok_or_else(|| String::from("Fontconfig is unavailable"))?;
    let pattern =
        Pattern::new(&fontconfig).map_err(|_| String::from("Fontconfig could not list fonts"))?;
    let mut objects = ObjectSet::new(&fontconfig)
        .map_err(|_| String::from("Fontconfig could not list font families"))?;
    objects
        .add(FC_FAMILY)
        .map_err(|_| String::from("Fontconfig could not list font families"))?;

    let listed = fontconfig::list_fonts(&pattern, Some(&objects))
        .map_err(|_| String::from("Fontconfig could not list font families"))?;
    let mut families: Vec<String> = listed
        .iter()
        .filter_map(|font| font.get_string(FC_FAMILY).ok().map(str::to_owned))
        .filter(|family| !family.trim().is_empty())
        .collect();

    families.sort_by_cached_key(|family| family.to_lowercase());
    families.dedup_by(|left, right| left.eq_ignore_ascii_case(right));
    Ok(families)
}

pub fn matching(families: &[String], query: &str, limit: usize) -> Vec<String> {
    let query = query.trim();
    if query.is_empty() || limit == 0 {
        return Vec::new();
    }

    let mut ranked: Vec<(u32, String)> = families
        .iter()
        .filter_map(|family| score(query, family).map(|score| (score, family.clone())))
        .collect();
    ranked.sort_by(|(left_score, left_name), (right_score, right_name)| {
        left_score
            .cmp(right_score)
            .then_with(|| left_name.to_lowercase().cmp(&right_name.to_lowercase()))
    });
    ranked
        .into_iter()
        .take(limit)
        .map(|(_, family)| family)
        .collect()
}

// Lower scores favor a match at a word boundary and consecutive letters.
fn score(query: &str, family: &str) -> Option<u32> {
    let query: Vec<char> = query.to_lowercase().chars().collect();
    let family: Vec<char> = family.to_lowercase().chars().collect();
    if query.is_empty() {
        return None;
    }

    let mut next = 0;
    let mut first = true;
    let mut score = 0u32;
    for wanted in query {
        let found = family[next..]
            .iter()
            .position(|candidate| *candidate == wanted)?
            + next;
        let boundary = found == 0 || !family[found - 1].is_alphanumeric();
        score = score.saturating_add((found - next) as u32 * 8);
        if first {
            score = score.saturating_add(found as u32 * 2);
            first = false;
        }
        if boundary {
            score = score.saturating_sub(3);
        }
        next = found + 1;
    }

    Some(score)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fuzzy_matches_subsequences_and_prefers_word_boundaries_and_adjacent_letters() {
        let families = vec![
            String::from("Noto Sans Display"),
            String::from("Nimbus Sans"),
            String::from("Inter"),
            String::from("Noto Sans"),
        ];

        assert_eq!(
            matching(&families, "ns", 3),
            ["Noto Sans", "Noto Sans Display", "Nimbus Sans"]
        );
        assert_eq!(matching(&families, "itr", 2), ["Inter"]);
    }

    #[test]
    fn an_empty_query_has_no_suggestions() {
        assert!(matching(&[String::from("Inter")], "  ", 8).is_empty());
    }
}
