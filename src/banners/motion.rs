//! How Banners come out of the Island and go back into it (ADR 0023), per output: each one grows
//! out of the Island's body into its card on the Island's own spring, so it bounces as the body
//! does, and shrinks back into it as it goes. The others slide to make room or close the gap. Time
//! is an input; this never reads a clock.

use std::time::{Duration, Instant};

use super::stack::Banner;
use crate::island::motion::{CRITICAL, Mode, Spring};

// coming out takes about as long as the body growing to a Surface, going back in a little less
const COME: Duration = Duration::from_millis(520);
const GO: Duration = Duration::from_millis(300);
const SLIDE: Duration = Duration::from_millis(380);

// how far out of the Island, 0 in it to 100 its card, past it as the spring swings
const OUT: f32 = 100.0;

#[derive(Debug, Clone)]
struct Card {
    banner: Banner,
    out: Spring<1>,

    // how far from the Island it hangs, past the cards nearer it
    slot: Spring<1>,
    leaving: bool,
}

// a Banner as it draws now
#[derive(Debug, Clone, PartialEq)]
pub struct Placed {
    pub banner: Banner,

    // 0 in the Island, 1 its card; past 1 while the spring swings
    pub out: f32,
    pub slot: f32,
    pub leaving: bool,
}

#[derive(Debug, Clone, Default)]
pub struct Motion {
    // the ones going first, under the rest, then the shown, nearest the Island first
    cards: Vec<Card>,
}

impl Motion {
    /*
     * follows the Banners `shown`, nearest the Island first, each `height` tall and `gap` apart: a
     * new one comes out, one gone goes back in, the rest slide to their places
     */
    pub fn follow(
        &mut self,
        shown: &[Banner],
        height: impl Fn(&Banner) -> f32,
        gap: f32,
        motion: Mode,
        damping: f32,
        now: Instant,
    ) {
        // a reload may have changed the motion: each taken from where it stands
        for card in &mut self.cards {
            if card.out.mode() != motion {
                card.out = Spring::new(card.out.at(now), motion);
                card.slot = Spring::new(card.slot.at(now), motion);
            }
        }

        for card in &mut self.cards {
            let stays = shown.iter().any(|banner| banner.id == card.banner.id);

            if !stays && !card.leaving {
                card.leaving = true;

                // going, it never swings back out of the Island
                card.out.damp(CRITICAL);
                card.out.to([0.0], GO, now);
            }
        }

        self.cards
            .retain(|card| !(card.leaving && card.out.settled(now)));

        // the ones going and not shown again, under the rest
        let mut cards: Vec<Card> = self
            .cards
            .iter()
            .filter(|card| card.leaving && !shown.iter().any(|banner| banner.id == card.banner.id))
            .cloned()
            .collect();
        let mut slot = 0.0;

        for banner in shown {
            let card = match self.cards.iter().find(|card| card.banner.id == banner.id) {
                Some(card) => {
                    let mut card = card.clone();

                    // shown again before it was gone, it comes back out from where it is
                    if card.leaving {
                        card.leaving = false;
                        card.out.damp(damping);
                    }

                    // and one taken from where it stood by a reload heads on out
                    if card.out.target() != [OUT] {
                        card.out.to([OUT], COME, now);
                    }

                    if card.slot.target() != [slot] {
                        card.slot.damp(damping);
                        card.slot.to([slot], SLIDE, now);
                    }

                    card.banner = banner.clone();
                    card
                }
                None => {
                    let mut out = Spring::new([0.0], motion);
                    out.damp(damping);
                    out.to([OUT], COME, now);

                    // it comes out where it goes, the rest make room
                    Card {
                        banner: banner.clone(),
                        out,
                        slot: Spring::new([slot], motion),
                        leaving: false,
                    }
                }
            };

            slot += height(banner) + gap;
            cards.push(card);
        }

        self.cards = cards;
    }

    pub fn placed(&self, now: Instant) -> Vec<Placed> {
        self.cards
            .iter()
            .map(|card| Placed {
                banner: card.banner.clone(),
                out: (card.out.at(now)[0] / OUT).max(0.0),
                slot: card.slot.at(now)[0],
                leaving: card.leaving,
            })
            .collect()
    }

    pub fn is_empty(&self) -> bool {
        self.cards.is_empty()
    }

    /*
     * whether any still moves, so the view wants frames: under reduced motion too, until one gone
     * has settled and can be let go
     */
    pub fn moving(&self, now: Instant) -> bool {
        self.cards.iter().any(|card| {
            [&card.out, &card.slot]
                .into_iter()
                .any(|spring| !spring.settled(now))
        })
    }
}
