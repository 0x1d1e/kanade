//! A password, or what is typed of one: what a Service holds while the user types it, and what the
//! program that checks it is handed. It never prints, so no log, panic or `Debug` of a Service
//! holding one shows it, and it is zeroed the moment the last holder goes, so a password does not
//! outlive the join or the prompt that took it.

use std::fmt;

use zeroize::Zeroize;

#[derive(Clone, Default, PartialEq, Eq)]
pub struct Secret(String);

impl Secret {
    pub fn push(&mut self, letter: char) {
        self.0.push(letter);
    }

    pub fn pop(&mut self) {
        self.0.pop();
    }

    // in characters, which is what the field draws a dot for
    pub fn len(&self) -> usize {
        self.0.chars().count()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    // what a program that checks or keeps it is handed; only the call handing it over reads it
    pub(super) fn plaintext(&self) -> &str {
        &self.0
    }
}

impl Drop for Secret {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        formatter.write_str("Secret(..)")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // a password never prints: not in a Debug, not in a panic a Service holding one reports
    #[test]
    fn a_secret_prints_as_nothing() {
        let mut secret = Secret::default();

        for letter in "hunter2".chars() {
            secret.push(letter);
        }

        assert_eq!(secret.len(), 7);
        assert_eq!(secret.plaintext(), "hunter2");
        assert_eq!(format!("{secret:?}"), "Secret(..)");
        assert_eq!(format!("{secret:#?}"), "Secret(..)");
        assert!(!format!("{:?}", Some(secret.clone())).contains("hunter"));

        secret.pop();
        assert_eq!(secret.len(), 6);
        assert!(!secret.is_empty());
        assert!(Secret::default().is_empty());
    }
}
