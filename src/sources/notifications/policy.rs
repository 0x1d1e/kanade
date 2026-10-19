//! Whether a notification shows and sounds, once its rule has been applied.
//! A matching rule wins over app silence. Fullscreen silence waits on a quiet Banner and skips
//! its sound; it does not drop the Banner, and the sound is not played later.

use super::rules::Effects;
use crate::sources::sound::Choice;

// app silence, only when no rule matched: no Banner and no sound, history as the rule left it
pub fn quiet_app(mut effects: Effects, matched: bool, silenced: bool) -> Effects {
    if matched || !silenced {
        return effects;
    }

    effects.banner = false;
    effects.sound = Choice::Off;
    effects
}

// a quiet Banner waits while a fullscreen window covers the focused output
pub fn hold_banner(loud: bool, fullscreen_silence: bool, covered: bool) -> bool {
    !loud && fullscreen_silence && covered
}

// the sound plays now, or not at all: a skipped one is not kept to play later
pub fn play_sound(enabled: bool, dnd: bool, fullscreen_silence: bool, covered: bool) -> bool {
    enabled && !dnd && !(fullscreen_silence && covered)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sources::notifications::rules::Effects;

    fn effects() -> Effects {
        Effects {
            banner: true,
            history: true,
            sound: Choice::Sound("message-new-instant".to_owned()),
            volume: Some(40),
            bypass_dnd: true,
            timeout: None,
            urgency: None,
        }
    }

    #[test]
    fn a_matching_rule_wins_over_app_silence() {
        let rule = effects();
        assert_eq!(quiet_app(rule.clone(), true, true), rule);

        let quiet = quiet_app(rule.clone(), false, true);
        assert!(!quiet.banner);
        assert_eq!(quiet.sound, Choice::Off);
        assert!(quiet.history);
        assert!(quiet.bypass_dnd);

        assert_eq!(quiet_app(rule.clone(), false, false), rule);
    }

    #[test]
    fn fullscreen_waits_on_a_quiet_banner_and_skips_its_sound() {
        assert!(hold_banner(false, true, true));
        assert!(!hold_banner(true, true, true));
        assert!(!hold_banner(false, false, true));
        assert!(!hold_banner(false, true, false));

        assert!(!play_sound(true, false, true, true));
        assert!(play_sound(true, false, true, false));
        assert!(play_sound(true, false, false, true));
        assert!(!play_sound(true, true, false, false));
        assert!(!play_sound(false, false, false, false));
    }
}
