//! Pack identity: a binary Merkle tree over sorted claim CIDs (spec §5.4).
//!
//! A flat digest only says "something differs"; a Merkle tree lets two sides
//! compare subtree hashes and find *which* claims differ in O(log n) rounds.
//! The tree shape is a pure function of the CID set, so both sides build the
//! identical tree independently.

/// A 32-byte BLAKE3 node hash.
pub type Hash = [u8; 32];

/// Domain-separation prefixes so a leaf can never be confused with an
/// interior node (second-preimage hardening, as in RFC 6962).
const LEAF: u8 = 0x00;
const NODE: u8 = 0x01;

fn leaf(cid: &str) -> Hash {
    let mut h = blake3::Hasher::new();
    h.update(&[LEAF]);
    h.update(cid.as_bytes());
    *h.finalize().as_bytes()
}

fn node(l: &Hash, r: &Hash) -> Hash {
    let mut h = blake3::Hasher::new();
    h.update(&[NODE]);
    h.update(l);
    h.update(r);
    *h.finalize().as_bytes()
}

/// All levels of the tree, leaves first. Duplicate CIDs count once. An odd
/// node at the end of a level is promoted unchanged.
pub fn levels<S: AsRef<str>>(cids: &[S]) -> Vec<Vec<Hash>> {
    let mut sorted: Vec<&str> = cids.iter().map(AsRef::as_ref).collect();
    sorted.sort_unstable();
    sorted.dedup();
    let mut levels = vec![sorted.into_iter().map(leaf).collect::<Vec<_>>()];
    while levels.last().is_some_and(|l| l.len() > 1) {
        let prev = levels.last().map(Vec::as_slice).unwrap_or_default();
        let next = prev
            .chunks(2)
            .map(|pair| match pair {
                [l, r] => node(l, r),
                [single] => *single,
                _ => unreachable!("chunks(2) yields 1 or 2 items"),
            })
            .collect();
        levels.push(next);
    }
    levels
}

/// Root of the tree as `b3:<hex>`. The empty set has the root of zero bytes.
pub fn root<S: AsRef<str>>(cids: &[S]) -> String {
    let levels = levels(cids);
    let root = match levels.last().and_then(|l| l.first()) {
        Some(h) => *h,
        None => *blake3::hash(&[]).as_bytes(),
    };
    format!("b3:{}", hex(&root))
}

pub fn hex(h: &Hash) -> String {
    h.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn order_and_duplicates_do_not_matter() {
        assert_eq!(
            root(&["b3:bb", "b3:aa", "b3:cc"]),
            root(&["b3:cc", "b3:aa", "b3:bb", "b3:aa"])
        );
    }

    #[test]
    fn any_change_changes_root() {
        let base = root(&["b3:aa", "b3:bb", "b3:cc"]);
        assert_ne!(base, root(&["b3:aa", "b3:bb"]));
        assert_ne!(base, root(&["b3:aa", "b3:bb", "b3:cd"]));
    }

    #[test]
    fn leaf_is_not_a_node() {
        // A single leaf's root differs from its raw hash (domain separation).
        assert_ne!(
            root(&["b3:aa"]),
            format!("b3:{}", blake3::hash(b"b3:aa").to_hex())
        );
    }

    #[test]
    fn levels_shape() {
        let l = levels(&["a", "b", "c", "d", "e"]);
        assert_eq!(l.iter().map(Vec::len).collect::<Vec<_>>(), vec![5, 3, 2, 1]);
    }
}
