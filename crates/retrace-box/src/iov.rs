//! M53 §3c: the pure half of the iovec calls. `decode` reads a guest `struct iovec` array and `plan`
//! turns its entries into the host-contiguous spans a translated call forwards. Neither touches a
//! guest or a host: `Box_::guest_iovec` supplies the reachability question as a closure.

/// A guest `struct iovec` array (16 bytes an entry: `iov_base`, then `iov_len`, little-endian) as
/// `(base, len)` pairs. A trailing partial entry is ignored; the caller reads exactly `iovcnt * 16`.
pub fn decode(bytes: &[u8]) -> Vec<(u64, u64)> {
    bytes.chunks_exact(16).map(|e| (
        u64::from_le_bytes(e[..8].try_into().unwrap()),
        u64::from_le_bytes(e[8..].try_into().unwrap()),
    )).collect()
}

/// The `(ipa, len)` spans of every entry, in order. `span_of(base, len)` answers the spans one entry
/// covers (an entry that crosses a backing boundary comes back as two or more), or `None` if the
/// program could not reach all of it. A zero-length entry plans to nothing and its base is never
/// examined (t0 M3: natively a zero-length entry with a bad base is dropped). `Err(i)` is the first
/// entry of `entries` that `span_of` could not reach.
pub fn plan(entries: &[(u64, u64)], mut span_of: impl FnMut(u64, u64) -> Option<Vec<(u64, usize)>>)
    -> Result<Vec<(u64, usize)>, usize> {
    let mut out = Vec::new();
    for (i, &(base, len)) in entries.iter().enumerate() {
        if len == 0 { continue; }
        out.extend(span_of(base, len).ok_or(i)?);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decode_reads_base_and_len_little_endian() {
        let mut b = Vec::new();
        b.extend_from_slice(&0x1122_3344_5566_7788u64.to_le_bytes());
        b.extend_from_slice(&5u64.to_le_bytes());
        b.extend_from_slice(&0x4000u64.to_le_bytes());
        b.extend_from_slice(&u64::MAX.to_le_bytes());
        assert_eq!(decode(&b), vec![(0x1122_3344_5566_7788, 5), (0x4000, u64::MAX)]);
        assert_eq!(decode(&[]), vec![]);
    }

    #[test]
    fn plan_splits_an_entry_across_two_spans() {
        let spans = plan(&[(0x1000, 100), (0x9000, 4)], |b, l| {
            if b == 0x1000 { Some(vec![(0x1000, 60), (0x8000, (l - 60) as usize)]) } else { Some(vec![(b, l as usize)]) }
        }).unwrap();
        assert_eq!(spans, vec![(0x1000, 60), (0x8000, 40), (0x9000, 4)]);
    }

    #[test]
    fn plan_returns_the_first_unreachable_entry() {
        let r = plan(&[(0x1000, 4), (0xdead, 4), (0xbeef, 4)], |b, l| {
            if b == 0x1000 { Some(vec![(b, l as usize)]) } else { None }
        });
        assert_eq!(r, Err(1));
    }

    #[test]
    fn a_zero_length_entry_plans_to_nothing_and_is_never_examined() {
        let r = plan(&[(0xdead, 0), (0x1000, 8), (0xbeef, 0)], |b, l| {
            assert_ne!(b, 0xdead, "a zero-length entry's base is never examined");
            assert_ne!(b, 0xbeef, "a zero-length entry's base is never examined");
            Some(vec![(b, l as usize)])
        });
        assert_eq!(r, Ok(vec![(0x1000, 8)]));
        assert_eq!(plan(&[(0xdead, 0)], |_, _| None), Ok(vec![]));
    }
}
