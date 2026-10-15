/*
 * the calculator, as a Launcher provider: a query that is arithmetic shows its value first, and
 * pressing it copies what is shown. Numbers, `+ - * / ^` (`**` too, and `× ÷ −`), parentheses and
 * signs; `^` binds tightest and right first, so `-2^2` is -4 and `2^3^2` is 512.
 *
 * Percentages: `20% of 50` is 10, `50 + 10%` is 55 and `50 - 10%` is 45 (a percent after `+` or
 * `-` is of what stands before it), `200 * 10%` is 20. Bases: `0xff`, `0b101` and `0o17` are
 * numbers, up to 2^53 so they are read exactly, and `255 to hex` writes `0xff` (also `bin`, `oct`,
 * `dec`). Units: `10 cm to in` (`in` and `as` read as `to`), from the table in `units.rs`. No
 * currency, nothing from the network.
 */

use super::provider::{Action, Answer, Fit, LauncherProvider, Mark};
use super::units;

// the largest integer a number holds exactly, which a base reads and writes
const EXACT: u64 = 1 << 53;

pub struct Calculator;

impl LauncherProvider for Calculator {
    fn find(&self, query: &str) -> Vec<Answer> {
        let Some(value) = converted(query).or_else(|| evaluate(query).map(shown)) else {
            return Vec::new();
        };

        vec![Answer {
            fit: Fit::Answer,
            title: value.clone(),
            detail: String::from("Calculator"),
            mark: Mark::Tile(String::from("=")),
            action: Action::Copy(value),
        }]
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Token {
    Number(f64),
    Plus,
    Minus,
    Times,
    Divide,
    Power,
    Percent,
    Of,

    // written in a base (`0xff`), so it is a conversion of its own, with no operator needed
    Based(f64),
    Open,
    Close,
}

/*
 * the value of `query`, none when it is not arithmetic, has no operator between two numbers (a
 * number alone is no sum to show; one written in a base is) or has no finite value, like a
 * division by zero
 */
pub fn evaluate(query: &str) -> Option<f64> {
    let (value, operators) = parse(query)?;

    (operators > 0).then_some(value)
}

// the value of `query` and how many operators it had, none when it is no whole finite sum
fn parse(query: &str) -> Option<(f64, usize)> {
    let mut parser = Parser {
        tokens: tokens(query)?,
        at: 0,
        depth: 0,
        operators: 0,
    };

    let value = parser.sum()?;

    let whole = parser.at == parser.tokens.len();

    (whole && value.is_finite()).then_some((value, parser.operators))
}

/*
 * `query` as a conversion, written as it is shown: `<sum> to <base>` writes an integer in it,
 * `<sum> <unit> to <unit>` converts. `to` may be `in` or `as`, and the last one splits, as a unit
 * may be `in`
 */
fn converted(query: &str) -> Option<String> {
    let (from, into) = split(query)?;

    if let Some(base) = Base::named(into) {
        return base.write(parse(from)?.0);
    }

    let to = units::named(into)?;
    let (number, unit) = units::ending(from)?;
    let value = units::convert(parse(number)?.0, unit, to)?;

    value
        .is_finite()
        .then(|| format!("{} {}", shown(value), to.label()))
}

// what stands before and after the last `to`, `in` or `as` word, none with nothing on a side
fn split(query: &str) -> Option<(&str, &str)> {
    // ASCII lowercase keeps every byte where it was
    let lower = query.to_ascii_lowercase();

    let (at, word) = ["to", "in", "as"]
        .into_iter()
        .filter_map(|word| Some((lower.rfind(&format!(" {word} "))?, word)))
        .max_by_key(|(at, _)| *at)?;

    let (from, into) = (query[..at].trim(), query[at + word.len() + 2..].trim());

    (!from.is_empty() && !into.is_empty()).then_some((from, into))
}

#[derive(Debug, Clone, Copy)]
enum Base {
    Hex,
    Binary,
    Octal,
    Decimal,
}

impl Base {
    fn named(name: &str) -> Option<Base> {
        match name.trim().to_lowercase().as_str() {
            "hex" | "hexadecimal" => Some(Base::Hex),
            "bin" | "binary" => Some(Base::Binary),
            "oct" | "octal" => Some(Base::Octal),
            // `dec` up to `decimal`, spelled by prefix as the color check takes the string "dec" for a color
            name if name.len() >= 3 && "decimal".starts_with(name) => Some(Base::Decimal),
            _ => None,
        }
    }

    // `value` written in it, none unless it is a whole number from 0 to 2^53
    fn write(self, value: f64) -> Option<String> {
        let whole = value.fract() == 0.0 && (0.0..=EXACT as f64).contains(&value);
        let value = whole.then_some(value as u64)?;

        Some(match self {
            Base::Hex => format!("{value:#x}"),
            Base::Binary => format!("{value:#b}"),
            Base::Octal => format!("{value:#o}"),
            Base::Decimal => value.to_string(),
        })
    }
}

fn tokens(query: &str) -> Option<Vec<Token>> {
    let chars: Vec<char> = query.chars().collect();
    let mut tokens = Vec::new();
    let mut at = 0;

    while at < chars.len() {
        let char = chars[at];
        at += 1;

        // `0x`, `0b` or `0o` and its digits; `0x`, `0b2` and the like are no number
        if let (true, Some(radix)) = (char == '0', chars.get(at).copied().and_then(radix)) {
            let digits: String = chars[at + 1..]
                .iter()
                .take_while(|next| next.is_ascii_alphanumeric())
                .collect();

            at += 1 + digits.len();

            let number = u64::from_str_radix(&digits, radix)
                .ok()
                .filter(|number| *number <= EXACT)?;

            tokens.push(Token::Based(number as f64));
            continue;
        }

        let token = match char {
            ' ' | '\t' => continue,
            '+' => Token::Plus,
            '-' | '−' => Token::Minus,
            '*' if chars.get(at) == Some(&'*') => {
                at += 1;
                Token::Power
            }
            '*' | '×' => Token::Times,
            '/' | '÷' => Token::Divide,
            '^' => Token::Power,
            '%' => Token::Percent,
            '(' => Token::Open,
            ')' => Token::Close,
            '0'..='9' | '.' => {
                let mut number = String::from(char);

                while let Some(digit) = chars
                    .get(at)
                    .filter(|next| next.is_ascii_digit() || **next == '.')
                {
                    number.push(*digit);
                    at += 1;
                }

                // `.` alone and `1.2.3` are no number
                Token::Number(number.parse().ok()?)
            }
            // `of` is the only word a sum has; the units and bases are split off before
            'o' | 'O' if chars.get(at).is_some_and(|next| matches!(next, 'f' | 'F')) => {
                at += 1;

                // `off` and `offer` are no `of`
                if chars.get(at).is_some_and(|next| next.is_alphanumeric()) {
                    return None;
                }

                Token::Of
            }
            _ => return None,
        };

        tokens.push(token);
    }

    Some(tokens)
}

// the base a prefix letter after a `0` says
fn radix(letter: char) -> Option<u32> {
    match letter.to_ascii_lowercase() {
        'x' => Some(16),
        'b' => Some(2),
        'o' => Some(8),
        _ => None,
    }
}

const DEPTH: usize = 64;

struct Parser {
    tokens: Vec<Token>,
    at: usize,

    // how deep in parentheses and signs, so a long query cannot overflow the stack
    depth: usize,

    // how many operators stood between two values, a number in a base counting as one
    operators: usize,
}

impl Parser {
    fn next_if(&mut self, token: Token) -> bool {
        let next = self.tokens.get(self.at) == Some(&token);

        if next {
            self.at += 1;
        }

        next
    }

    // the binary operator next, if one of `operators`
    fn operator(&mut self, operators: &[Token]) -> Option<Token> {
        let token = *self.tokens.get(self.at)?;

        operators.contains(&token).then(|| {
            self.at += 1;
            self.operators += 1;
            token
        })
    }

    // `parse` one level deeper, none past `DEPTH`
    fn nested(&mut self, parse: fn(&mut Self) -> Option<f64>) -> Option<f64> {
        if self.depth == DEPTH {
            return None;
        }

        self.depth += 1;
        let value = parse(self);
        self.depth -= 1;

        value
    }

    fn sum(&mut self) -> Option<f64> {
        let mut value = self.product()?;

        while let Some(operator) = self.operator(&[Token::Plus, Token::Minus]) {
            let start = self.at;
            let right = self.product()?;

            // a percent of a number added or taken is of what stands before it: 50 + 10% is 55
            let right = match self.tokens.get(start..self.at) {
                Some([Token::Number(_), Token::Percent]) => value * right,
                _ => right,
            };

            value = match operator {
                Token::Plus => value + right,
                _ => value - right,
            };
        }

        Some(value)
    }

    fn product(&mut self) -> Option<f64> {
        let mut value = self.signed()?;

        while let Some(operator) = self.operator(&[Token::Times, Token::Divide, Token::Of]) {
            let right = self.signed()?;

            value = match operator {
                Token::Times | Token::Of => value * right,
                _ => value / right,
            };
        }

        Some(value)
    }

    fn signed(&mut self) -> Option<f64> {
        if self.next_if(Token::Minus) {
            return Some(-self.nested(Self::signed)?);
        }

        if self.next_if(Token::Plus) {
            return self.nested(Self::signed);
        }

        self.power()
    }

    fn power(&mut self) -> Option<f64> {
        let base = self.atom()?;
        let base = if self.next_if(Token::Percent) {
            base / 100.0
        } else {
            base
        };

        if self.operator(&[Token::Power]).is_some() {
            return Some(base.powf(self.nested(Self::signed)?));
        }

        Some(base)
    }

    fn atom(&mut self) -> Option<f64> {
        match *self.tokens.get(self.at)? {
            Token::Number(number) => {
                self.at += 1;
                Some(number)
            }
            Token::Based(number) => {
                self.at += 1;
                self.operators += 1;
                Some(number)
            }
            Token::Open => {
                self.at += 1;
                let value = self.nested(Self::sum)?;

                self.next_if(Token::Close).then_some(value)
            }
            _ => None,
        }
    }
}

/*
 * `value` to twelve significant digits, so 0.1 + 0.2 is 0.3, except a whole number below 1e15,
 * which is shown whole; very large and very small ones as an exponent
 */
pub fn shown(value: f64) -> String {
    if value.fract() == 0.0 && value.abs() < 1e15 {
        // no "-0"
        return format!("{}", value + 0.0);
    }

    let value: f64 = format!("{value:.11e}").parse().unwrap_or(value);

    // no "-0"
    let value = if value == 0.0 { 0.0 } else { value };

    if value != 0.0 && !(1e-6..1e15).contains(&value.abs()) {
        format!("{value:e}")
    } else {
        format!("{value}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shown_value(query: &str) -> Option<String> {
        evaluate(query).map(shown)
    }

    #[test]
    fn operators_bind_by_precedence() {
        assert_eq!(evaluate("2+2*3"), Some(8.0));
        assert_eq!(evaluate("(2+2)*3"), Some(12.0));
        assert_eq!(evaluate("10 - 4 - 3"), Some(3.0), "left first");
        assert_eq!(evaluate("12 / 3 / 2"), Some(2.0), "left first");
        assert_eq!(evaluate("2^3^2"), Some(512.0), "right first");
        assert_eq!(evaluate("2**10"), Some(1024.0));
        assert_eq!(evaluate("2 × 3 ÷ 4 − 1"), Some(0.5));
    }

    #[test]
    fn signs_bind_looser_than_a_power() {
        assert_eq!(evaluate("-2^2"), Some(-4.0));
        assert_eq!(evaluate("(-2)^2"), Some(4.0));
        assert_eq!(evaluate("2^-1"), Some(0.5));
        assert_eq!(evaluate("3 - -2"), Some(5.0));
        assert_eq!(evaluate("-(1+2)*+2"), Some(-6.0));
    }

    #[test]
    fn decimals_parse() {
        assert_eq!(evaluate("1.5*2"), Some(3.0));
        assert_eq!(evaluate(".5+.25"), Some(0.75));
        assert_eq!(evaluate("1.+1"), Some(2.0));
        assert_eq!(evaluate("1.2.3+1"), None);
        assert_eq!(evaluate(".+1"), None);
    }

    #[test]
    fn only_arithmetic_with_an_operator_has_a_value() {
        for query in [
            "", "2", "-2", "(2)", "firefox", "2+", "*2", "2+(3", "2+3)", "2(3)", "2 3", "1e3+1",
            "2+x", ":smile", "7-zip",
        ] {
            assert_eq!(evaluate(query), None, "{query:?}");
        }
    }

    #[test]
    fn no_finite_value_is_none() {
        assert_eq!(evaluate("1/0"), None);
        assert_eq!(evaluate("0/0"), None);
        assert_eq!(evaluate("(-8)^(1/3)"), None);
        assert_eq!(evaluate("10^400"), None);
    }

    #[test]
    fn a_value_shows_to_twelve_digits() {
        assert_eq!(shown_value("2+2*3").as_deref(), Some("8"));
        assert_eq!(shown_value("0.1+0.2").as_deref(), Some("0.3"));
        assert_eq!(shown_value("1/3").as_deref(), Some("0.333333333333"));
        assert_eq!(shown_value("2/-3").as_deref(), Some("-0.666666666667"));
        assert_eq!(shown_value("0*-1").as_deref(), Some("0"));
        assert_eq!(shown_value("10^14").as_deref(), Some("100000000000000"));
        assert_eq!(shown_value("10^15").as_deref(), Some("1e15"));
        assert_eq!(shown_value("2^60").as_deref(), Some("1.15292150461e18"));
        assert_eq!(shown_value("1/10^7").as_deref(), Some("1e-7"));
    }

    fn said(query: &str) -> Option<String> {
        Calculator
            .find(query)
            .into_iter()
            .next()
            .map(|answer| answer.title)
    }

    #[test]
    fn a_percent_is_of_what_it_is_added_to_or_taken_from() {
        assert_eq!(said("20% of 50").as_deref(), Some("10"));
        assert_eq!(said("50 + 10%").as_deref(), Some("55"));
        assert_eq!(said("50 - 10%").as_deref(), Some("45"));
        assert_eq!(said("100 + 10% + 10%").as_deref(), Some("121"));
        assert_eq!(said("200 * 10%").as_deref(), Some("20"));
        assert_eq!(said("(1+1)% of 100").as_deref(), Some("2"));
        assert_eq!(said("50%"), None, "a percent alone is no sum");

        for query in ["of", "of 5", "5 of", "off", "2 off 3", "firefox 10%"] {
            assert_eq!(said(query), None, "{query:?}");
        }
    }

    #[test]
    fn numbers_in_a_base_are_exact_or_nothing() {
        assert_eq!(said("0xff").as_deref(), Some("255"));
        assert_eq!(said("0XFF + 1").as_deref(), Some("256"));
        assert_eq!(said("0b101").as_deref(), Some("5"));
        assert_eq!(said("0o17 * 2").as_deref(), Some("30"));
        assert_eq!(said("0xffffffffffff").as_deref(), Some("281474976710655"));

        for query in [
            "0x",
            "0b102",
            "0xfg",
            "0x20000000000001",
            "0b",
            "0o8",
            "0xff.1",
        ] {
            assert_eq!(said(query), None, "{query:?}");
        }
    }

    #[test]
    fn a_whole_number_is_written_in_a_base() {
        assert_eq!(said("255 to hex").as_deref(), Some("0xff"));
        assert_eq!(said("255 in binary").as_deref(), Some("0b11111111"));
        assert_eq!(said("8 as oct").as_deref(), Some("0o10"));
        assert_eq!(said("0xff to dec").as_deref(), Some("255"));
        assert_eq!(said("0b101 + 0x10 to hex").as_deref(), Some("0x15"));
        assert_eq!(said("0 to hex").as_deref(), Some("0x0"));

        // not whole, negative or past what is exact
        for query in [
            "1.5 to hex",
            "-1 to hex",
            "2^60 to hex",
            "1/0 to hex",
            "to hex",
        ] {
            assert_eq!(said(query), None, "{query:?}");
        }
    }

    #[test]
    fn a_unit_converts_and_says_the_unit() {
        assert_eq!(said("10 cm to in").as_deref(), Some("3.93700787402 in"));
        assert_eq!(said("10cm in inches").as_deref(), Some("3.93700787402 in"));
        assert_eq!(said("5 in to cm").as_deref(), Some("12.7 cm"));
        assert_eq!(said("5 in in cm").as_deref(), Some("12.7 cm"));
        assert_eq!(said("2*3 km as m").as_deref(), Some("6000 m"));
        assert_eq!(said("1 mile to km").as_deref(), Some("1.609344 km"));
        assert_eq!(said("1 GiB to MiB").as_deref(), Some("1024 MiB"));
        assert_eq!(said("3 fl oz to mL").as_deref(), Some("88.7205886875 mL"));
        assert_eq!(said("100 c to f").as_deref(), Some("212 °F"));
        assert_eq!(said("32 °F to °C").as_deref(), Some("0 °C"));
        assert_eq!(said("-40 f to c").as_deref(), Some("-40 °C"));
        assert_eq!(said("60 mph to km/h").as_deref(), Some("96.56064 km/h"));
    }

    #[test]
    fn what_is_no_conversion_shows_nothing() {
        for query in [
            // units of other dimensions, or unknown ones
            "1 kg to m",
            "5 foo to m",
            "5 m to foo",
            "100 USD to EUR",
            // no number, no unit, or nothing on a side
            "cm to in",
            "10 to in",
            "to in",
            "10 cm to",
            "10 cm in",
            "firefox to in",
            // a number alone, a word in the way
            "10 xcm to in",
        ] {
            assert_eq!(said(query), None, "{query:?}");
        }
    }

    #[test]
    fn a_sum_answers_first_and_copies_its_value() {
        let answers = Calculator.find("2+2*3");

        assert_eq!(answers.len(), 1);
        assert_eq!(answers[0].fit, Fit::Answer);
        assert_eq!(answers[0].title, "8");
        assert_eq!(answers[0].action, Action::Copy(String::from("8")));
        assert!(Calculator.find("firefox").is_empty());
    }
}
