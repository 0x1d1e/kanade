//! The recording as the request carries it: 16 kHz, mono, signed 16-bit little-endian PCM, held in
//! memory and wrapped in a canonical 44-byte WAV header. Nothing here touches a disk.

// what `pw-record` is asked for, and what the header says
pub const RATE: u32 = 16_000;
pub const CHANNELS: u16 = 1;
pub const BITS: u16 = 16;

// a recording stops here, however long the user talks
pub const MAX_SECONDS: u32 = 120;

pub const FRAME: usize = (CHANNELS * BITS / 8) as usize;
const HEADER: usize = 44;

// how many PCM bytes a second of recording is
pub const BYTES_PER_SECOND: usize = RATE as usize * FRAME;

// the most PCM bytes a recording holds
pub const MAX_BYTES: usize = MAX_SECONDS as usize * BYTES_PER_SECOND;

/*
 * how many of `incoming` bytes to keep of a recording that holds `held`: all of them until the cap,
 * then only the whole frames that fit, then none. `full` is true once the cap is reached
 */
pub fn accept(held: usize, incoming: usize) -> (usize, bool) {
    let room = MAX_BYTES.saturating_sub(held);
    let take = incoming.min(room) / FRAME * FRAME;

    (take, held + take >= MAX_BYTES)
}

// `pcm` in a WAV file; a trailing half frame is left out
pub fn wrap(pcm: &[u8]) -> Vec<u8> {
    let pcm = &pcm[..pcm.len() / FRAME * FRAME];
    let length = pcm.len() as u32;
    let block = FRAME as u16;
    let mut wav = Vec::with_capacity(HEADER + pcm.len());

    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&(36 + length).to_le_bytes());
    wav.extend_from_slice(b"WAVEfmt ");
    wav.extend_from_slice(&16u32.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes()); // PCM
    wav.extend_from_slice(&CHANNELS.to_le_bytes());
    wav.extend_from_slice(&RATE.to_le_bytes());
    wav.extend_from_slice(&(RATE * u32::from(block)).to_le_bytes());
    wav.extend_from_slice(&block.to_le_bytes());
    wav.extend_from_slice(&BITS.to_le_bytes());
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&length.to_le_bytes());
    wav.extend_from_slice(pcm);

    wav
}

#[cfg(test)]
mod tests {
    use super::*;

    fn word(wav: &[u8], at: usize) -> u32 {
        u32::from_le_bytes(wav[at..at + 4].try_into().unwrap())
    }

    #[test]
    fn the_header_describes_the_pcm_after_it() {
        let wav = wrap(&[1, 0, 2, 0, 3, 0]);

        assert_eq!(wav.len(), HEADER + 6);
        assert_eq!(&wav[..4], b"RIFF");
        assert_eq!(word(&wav, 4), 36 + 6);
        assert_eq!(&wav[8..16], b"WAVEfmt ");
        assert_eq!(word(&wav, 16), 16);
        assert_eq!(&wav[20..24], [1, 0, 1, 0]); // PCM, one channel
        assert_eq!(word(&wav, 24), 16_000);
        assert_eq!(word(&wav, 28), 32_000);
        assert_eq!(&wav[32..36], [2, 0, 16, 0]); // block align, bits
        assert_eq!(&wav[36..40], b"data");
        assert_eq!(word(&wav, 40), 6);
        assert_eq!(&wav[44..], [1, 0, 2, 0, 3, 0]);
    }

    #[test]
    fn a_half_frame_at_the_end_is_dropped() {
        let wav = wrap(&[1, 0, 2]);

        assert_eq!(word(&wav, 40), 2);
        assert_eq!(wav.len(), HEADER + 2);
    }

    #[test]
    fn a_recording_stops_at_two_minutes() {
        assert_eq!(MAX_BYTES, 3_840_000);
        assert_eq!(accept(0, 100), (100, false));
        assert_eq!(accept(MAX_BYTES - 10, 100), (10, true));
        assert_eq!(accept(MAX_BYTES - 11, 100), (10, false));
        assert_eq!(accept(MAX_BYTES, 100), (0, true));
    }
}
