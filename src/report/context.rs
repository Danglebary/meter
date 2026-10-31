//! The context section: the main thread's calls by the context each sent, what their input
//! side cost, and the share of the main thread's cache-read cost that long-context calls
//! carry.

use super::price::cost;
use super::range::Thread;
use super::tally::{add, count_one};
use super::transcript::Transcript;

/// The edges between context bands, in tokens. A call falls in the band below the first
/// edge above its context, and in the last band where no edge is above it.
pub const CONTEXT_BAND_EDGES_TOKENS: [u64; 4] = [50_000, 100_000, 200_000, 400_000];
/// A call whose context is above this is long in the cache-read share.
pub const LONG_CONTEXT_FLOOR_TOKENS: u64 = 300_000;

/// The context section. Every amount is in nanodollars; a call the price table lacks counts
/// in its band and costs nothing there.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContextSection {
    /// Every band, the narrowest context first, whether or not a call fell in it.
    pub bands: Vec<Band>,
    /// The main thread's cache-read cost.
    pub cache_read: u128,
    /// The part of `cache_read` that calls above `LONG_CONTEXT_FLOOR_TOKENS` carry.
    pub long_context_cache_read: u128,
}

/// The main-thread calls whose context falls from `from_tokens` up to, and short of,
/// `to_tokens`, and what their input, cache reads, and cache writes cost.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Band {
    pub from_tokens: u64,
    pub to_tokens: Option<u64>,
    pub calls: u64,
    pub input_side: u128,
}

/// The context section of the main-thread calls in `transcripts`.
///
/// # Panics
///
/// Where the long-context cache reads exceed the cache reads they are part of.
#[must_use]
pub fn context_section(transcripts: &[Transcript]) -> ContextSection {
    let mut section = ContextSection {
        bands: empty_bands(),
        cache_read: 0,
        long_context_cache_read: 0,
    };
    let main = transcripts
        .iter()
        .filter(|transcript| transcript.thread == Thread::Main);
    for transcript in main {
        for call in &transcript.calls {
            let context = call.usage.context();
            let priced = cost(&call.model, &call.usage).unwrap_or_default();
            let input_side = [priced.input, priced.cache_read, priced.cache_write()]
                .into_iter()
                .fold(0_u128, |sum, part| add(sum, u128::from(part)));
            let index = band_index(context);
            let Some(band) = section.bands.get_mut(index) else {
                unreachable!("a band index names one of the bands: {index}")
            };
            *band = Band {
                calls: count_one(band.calls),
                input_side: add(band.input_side, input_side),
                ..*band
            };
            section.cache_read = add(section.cache_read, u128::from(priced.cache_read));
            if context > LONG_CONTEXT_FLOOR_TOKENS {
                section.long_context_cache_read = add(
                    section.long_context_cache_read,
                    u128::from(priced.cache_read),
                );
            }
        }
    }
    assert!(
        section.long_context_cache_read <= section.cache_read,
        "the long-context cache reads are part of the cache reads"
    );
    section
}

/// One band per gap between edges, with the band below the first edge and the band from
/// the last, each holding no calls.
fn empty_bands() -> Vec<Band> {
    let mut bands = Vec::with_capacity(CONTEXT_BAND_EDGES_TOKENS.len().saturating_add(1));
    let mut from_tokens = 0;
    for edge in CONTEXT_BAND_EDGES_TOKENS {
        assert!(edge > from_tokens, "the band edges rise: {edge}");
        bands.push(Band {
            from_tokens,
            to_tokens: Some(edge),
            calls: 0,
            input_side: 0,
        });
        from_tokens = edge;
    }
    bands.push(Band {
        from_tokens,
        to_tokens: None,
        calls: 0,
        input_side: 0,
    });
    bands
}

/// The index of the band a context falls in.
fn band_index(context: u64) -> usize {
    CONTEXT_BAND_EDGES_TOKENS
        .iter()
        .position(|&edge| context < edge)
        .unwrap_or(CONTEXT_BAND_EDGES_TOKENS.len())
}
