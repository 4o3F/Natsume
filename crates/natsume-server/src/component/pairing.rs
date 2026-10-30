//! Shared pairing-code syntax and bounded per-domain admission mechanics.
use std::{
    collections::{HashMap, VecDeque},
    net::IpAddr,
    time::{Duration, Instant},
};
const ALPHABET: &[u8; 32] = b"23456789ABCDEFGHJKLMNPQRSTUVWXYZ";
const WINDOW: Duration = Duration::from_mins(1);

pub(super) fn code(random: [u8; 8]) -> String {
    random
        .iter()
        .map(|byte| char::from(ALPHABET[usize::from(byte & 31)]))
        .collect()
}
pub(super) fn normalize(value: &str) -> Option<String> {
    let value = value.trim();
    // Inspect bytes before slicing, including malicious non-ASCII input.
    let raw: Vec<u8> = if value.len() == 9 && value.as_bytes()[4] == b'-' {
        value
            .bytes()
            .enumerate()
            .filter_map(|(i, b)| (i != 4).then_some(b))
            .collect()
    } else {
        value.bytes().collect()
    };
    let raw: Vec<u8> = raw.into_iter().map(|b| b.to_ascii_uppercase()).collect();
    if raw.len() != 8 || !raw.iter().all(|b| ALPHABET.contains(b)) {
        return None;
    }
    String::from_utf8(raw).ok()
}
#[derive(Default)]
pub(super) struct PairingBudget {
    attempts: VecDeque<(Instant, IpAddr)>,
    source_blocks: HashMap<IpAddr, Instant>,
    global_block: Option<Instant>,
}
impl PairingBudget {
    pub(super) fn admit(&mut self, source: IpAddr, now: Instant) -> bool {
        let source = source.to_canonical();
        self.attempts
            .retain(|(at, _)| now.duration_since(*at) < WINDOW);
        self.source_blocks.retain(|_, until| *until > now);
        if self.global_block.is_some_and(|until| until > now)
            || self.source_blocks.contains_key(&source)
        {
            return false;
        }
        if self.attempts.len() >= 60 {
            self.global_block = Some(now + WINDOW);
            return false;
        }
        if self.attempts.iter().filter(|(_, ip)| *ip == source).count() >= 5 {
            self.source_blocks.insert(source, now + WINDOW);
            return false;
        }
        self.attempts.push_back((now, source));
        true
    }
}
