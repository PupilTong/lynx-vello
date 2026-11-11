//! The cache tiers, all `vant`'s: an in-memory LRU with a byte budget, and
//! the HTTP freshness rules the byte tier
//! ([`vant::byte_cache::ByteCache`]) applies over the platform's
//! [`Store`](vant::Store).

pub use vant::{http, memory};
