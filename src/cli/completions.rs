//! `kanade completions bash|fish|zsh`: what a shell completes after `kanade`, derived from the
//! verbs' usage lines, so a verb added to a Module completes with no second list to keep. It
//! completes the words that name a form (`a|b|c`) up to two deep, and never a `<placeholder>`.

use super::{Entry, commands};

// what follows `kanade` (an empty prefix), `kanade <command>` or `kanade <command> <word>`, each with the words it takes
type Grammar = Vec<(Vec<String>, Vec<String>)>;

// the script for `shell`, or none when it is no shell
pub fn script(shell: &str) -> Option<String> {
    let commands = commands();
    let grammar = grammar(&commands);

    match shell {
        "bash" => Some(bash(&grammar)),
        "fish" => Some(fish(&commands, &grammar)),
        "zsh" => Some(zsh(&commands, &grammar)),
        _ => None,
    }
}

// a word of a form, with its optional brackets off; none for a placeholder like `<id>`
fn literal(word: &str) -> Option<&str> {
    let word = word.trim_start_matches('[').trim_end_matches(']');

    (!word.is_empty() && !word.starts_with('<')).then_some(word)
}

/*
 * the words a form takes, as (the word before them, the word) pairs, none before for the first. A
 * form is alternatives split by `|`; where the first goes on in a literal word of its own, as in
 * `screenshot area|window|output`, the rest are that word's, else each alternative's first is a word
 */
fn words(form: &str) -> Vec<(Option<&str>, &str)> {
    let alternatives: Vec<Vec<&str>> = form
        .split('|')
        .map(|alternative| alternative.split_whitespace().collect())
        .collect();

    let first = &alternatives[0];

    if let [word, then, ..] = first[..]
        && let (Some(word), Some(then)) = (literal(word), literal(then))
    {
        let rest = alternatives[1..]
            .iter()
            .filter_map(|alternative| literal(alternative.first()?));

        return std::iter::once(then)
            .chain(rest)
            .map(|then| (Some(word), then))
            .collect();
    }

    alternatives
        .iter()
        .filter_map(|alternative| literal(alternative.first()?))
        .map(|word| (None, word))
        .collect()
}

fn grammar(commands: &[Entry]) -> Grammar {
    let names: Vec<String> = commands
        .iter()
        .map(|command| command.name.to_owned())
        .collect();
    let mut grammar: Grammar = vec![(Vec::new(), names.clone())];

    let mut add = |prefix: Vec<&str>, word: &str| {
        let prefix: Vec<String> = prefix.into_iter().map(str::to_owned).collect();

        match grammar.iter_mut().find(|(known, _)| *known == prefix) {
            Some((_, words)) if words.iter().any(|each| each == word) => {}
            Some((_, words)) => words.push(word.to_owned()),
            None => grammar.push((prefix, vec![word.to_owned()])),
        }
    };

    for command in commands {
        // each form is a line of the usage starting with the name; an indented line is an argument
        for line in command.usage.iter().flat_map(|usage| usage.lines()) {
            let Some(form) = line
                .strip_prefix(command.name)
                .filter(|form| form.starts_with(' '))
            else {
                continue;
            };

            for (before, word) in words(form) {
                match before {
                    Some(before) => {
                        add(vec![command.name], before);
                        add(vec![command.name, before], word);
                    }
                    None => add(vec![command.name], word),
                }
            }
        }
    }

    // `help` takes the name of a command
    grammar.push((vec![String::from("help")], names));

    grammar
}

// a line of shell for each place a word completes, `place` being `$position:$command:$word`
fn arms(grammar: &Grammar, tail: impl Fn(&str) -> String) -> String {
    let mut arms = String::new();

    for (prefix, words) in grammar {
        let place = match prefix[..] {
            [] => String::from("1:*"),
            [ref command] => format!("2:{command}:*"),
            [ref command, ref word] => format!("3:{command}:{word}"),
            _ => continue,
        };

        arms.push_str(&format!("    {place}) {} ;;\n", tail(&words.join(" "))));
    }

    arms
}

fn bash(grammar: &Grammar) -> String {
    let arms = arms(grammar, |words| format!("words=\"{words}\""));

    format!(
        "# bash completion for kanade, from `kanade completions bash`
_kanade() {{
    local words=
    case \"$COMP_CWORD:${{COMP_WORDS[1]}}:${{COMP_WORDS[2]}}\" in
{arms}    esac
    COMPREPLY=($(compgen -W \"$words\" -- \"${{COMP_WORDS[COMP_CWORD]}}\"))
}}
complete -F _kanade kanade
"
    )
}

// `text` in single quotes, as fish reads them
fn fish_quoted(text: &str) -> String {
    format!("'{}'", text.replace('\\', "\\\\").replace('\'', "\\'"))
}

// a `_describe` line of `name` and `about`, in single quotes, with no `:` in `about` to split it
fn zsh_described(name: &str, about: &str) -> String {
    format!(
        "'{name}:{}'",
        about.replace('\'', "'\\''").replace(':', "\\:")
    )
}

fn fish(commands: &[Entry], grammar: &Grammar) -> String {
    let mut script = String::from(
        "# fish completion for kanade, from `kanade completions fish`\ncomplete -c kanade -f\n",
    );

    for command in commands {
        script.push_str(&format!(
            "complete -c kanade -n __fish_use_subcommand -a {} -d {}\n",
            command.name,
            fish_quoted(command.about)
        ));
    }

    for (prefix, words) in grammar {
        let words = words.join(" ");

        let seen = match prefix[..] {
            [] => continue,
            [ref command] => format!("__fish_seen_subcommand_from {command}"),
            [ref command, ref word] => format!(
                "__fish_seen_subcommand_from {command}; and __fish_seen_subcommand_from {word}"
            ),
            _ => continue,
        };

        script.push_str(&format!(
            "complete -c kanade -n '{seen}; and not __fish_seen_subcommand_from {words}' -a '{words}'\n"
        ));
    }

    script
}

fn zsh(commands: &[Entry], grammar: &Grammar) -> String {
    let described: String = commands
        .iter()
        .map(|command| format!("        {}\n", zsh_described(command.name, command.about)))
        .collect();

    // the commands are described, so the first place is left to `_describe`
    let rest: Grammar = grammar.iter().skip(1).cloned().collect();
    let arms = arms(&rest, |words| format!("compadd -- {words}"));

    format!(
        "#compdef kanade
# zsh completion for kanade, from `kanade completions zsh`
_kanade() {{
    local -a commands=(
{described}    )
    case \"$CURRENT:$words[2]:$words[3]\" in
    2:*) _describe -t commands command commands ;;
{arms}    esac
}}
_kanade \"$@\"
"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    // the grammar is the verb table's: every command and each form's words are there to complete
    #[test]
    fn every_verb_and_its_words_complete() {
        let commands = commands();
        let grammar = grammar(&commands);

        for command in &commands {
            assert!(
                grammar[0].1.contains(&command.name.to_owned()),
                "{}",
                command.name
            );
        }

        let completes = |prefix: &[&str], word: &str| {
            grammar
                .iter()
                .any(|(known, words)| known == prefix && words.iter().any(|each| each == word))
        };

        // alternatives, a word followed by a placeholder, and a word with words of its own
        assert!(completes(&["wifi"], "toggle"));
        assert!(completes(&["timer"], "pomodoro"));
        assert!(completes(&["google-calendar"], "sign-in"));
        assert!(completes(&["capture"], "screenshot"));
        assert!(completes(&["capture", "screenshot"], "window"));
        assert!(completes(&["capture", "record"], "pause"));
        assert!(completes(&["notifications", "dnd"], "toggle"));
        assert!(completes(&["power-profile"], "cycle"));
        assert!(completes(&["help"], "wifi"));
        assert!(completes(&["completions"], "zsh"));

        // a placeholder is not a word
        assert!(
            grammar
                .iter()
                .all(|(_, words)| words.iter().all(|word| !word.contains('<')))
        );

        for shell in ["bash", "fish", "zsh"] {
            let script = script(shell).expect("a shell");

            for command in &commands {
                assert!(script.contains(command.name), "{shell}: {}", command.name);
            }
        }

        assert!(script("tcsh").is_none());
    }
}
