/*
 * the calculator, as a Launcher provider: a query that is arithmetic shows its value first, and
 * pressing it copies the value. Numbers, `+ - * / ^` (`**` too, and `× ÷ −`), parentheses and
 * signs; `^` binds tightest and right first, so `-2^2` is -4 and `2^3^2` is 512
 */

use super::provider::{Action, Answer, Fit, LauncherProvider, Mark};

pub struct Calculator;

impl LauncherProvider for Calculator {
    fn find(&self, query: &str) -> Vec<Answer> {
        let Some(value) = evaluate(query) else {
            return Vec::new();
        };

        let value = shown(value);

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
    Open,
    Close,
}

/*
 * the value of `query`, none when it is not arithmetic, has no operator between two numbers (a
 * number alone is no sum to show) or has no finite value, like a division by zero
 */
pub fn evaluate(query: &str) -> Option<f64> {
    let mut parser = Parser {
        tokens: tokens(query)?,
        at: 0,
        operators: 0,
    };

    let value = parser.sum()?;

    let whole = parser.at == parser.tokens.len();

    (whole && parser.operators > 0 && value.is_finite()).then_some(value)
}

fn tokens(query: &str) -> Option<Vec<Token>> {
    let mut tokens = Vec::new();
    let mut chars = query.chars().peekable();

    while let Some(char) = chars.next() {
        let token = match char {
            ' ' | '\t' => continue,
            '+' => Token::Plus,
            '-' | '−' => Token::Minus,
            '*' if chars.next_if_eq(&'*').is_some() => Token::Power,
            '*' | '×' => Token::Times,
            '/' | '÷' => Token::Divide,
            '^' => Token::Power,
            '(' => Token::Open,
            ')' => Token::Close,
            '0'..='9' | '.' => {
                let mut number = String::from(char);

                while let Some(digit) = chars.next_if(|next| next.is_ascii_digit() || *next == '.')
                {
                    number.push(digit);
                }

                // `.` alone and `1.2.3` are no number
                Token::Number(number.parse().ok()?)
            }
            _ => return None,
        };

        tokens.push(token);
    }

    Some(tokens)
}

struct Parser {
    tokens: Vec<Token>,
    at: usize,

    // how many operators stood between two values
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

    fn sum(&mut self) -> Option<f64> {
        let mut value = self.product()?;

        while let Some(operator) = self.operator(&[Token::Plus, Token::Minus]) {
            let right = self.product()?;

            value = match operator {
                Token::Plus => value + right,
                _ => value - right,
            };
        }

        Some(value)
    }

    fn product(&mut self) -> Option<f64> {
        let mut value = self.signed()?;

        while let Some(operator) = self.operator(&[Token::Times, Token::Divide]) {
            let right = self.signed()?;

            value = match operator {
                Token::Times => value * right,
                _ => value / right,
            };
        }

        Some(value)
    }

    fn signed(&mut self) -> Option<f64> {
        if self.next_if(Token::Minus) {
            return Some(-self.signed()?);
        }

        if self.next_if(Token::Plus) {
            return self.signed();
        }

        self.power()
    }

    fn power(&mut self) -> Option<f64> {
        let base = self.atom()?;

        if self.operator(&[Token::Power]).is_some() {
            return Some(base.powf(self.signed()?));
        }

        Some(base)
    }

    fn atom(&mut self) -> Option<f64> {
        match *self.tokens.get(self.at)? {
            Token::Number(number) => {
                self.at += 1;
                Some(number)
            }
            Token::Open => {
                self.at += 1;
                let value = self.sum()?;

                self.next_if(Token::Close).then_some(value)
            }
            _ => None,
        }
    }
}

/*
 * `value` to twelve significant digits, so 0.1 + 0.2 is 0.3; very large and very small ones as
 * an exponent
 */
pub fn shown(value: f64) -> String {
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
