#![deny(missing_docs)]
#![feature(external_doc)]
#![doc(include = "../README.md")]
#![cfg_attr(test, deny(warnings))]

extern crate flat_tree as flat;
extern crate sparse_bitfield as bitfield;

mod proof;
mod verification;

pub use self::bitfield::{Bitfield, Change};
pub use self::proof::Proof;
pub use self::verification::Verification;

use std::{cmp, convert};

/// Index a tree structure or something.
#[derive(Debug)]
pub struct TreeIndex {
  bitfield: Bitfield,
}

impl TreeIndex {
  /// Create a new TreeIndex by passing it a sparse_bitfield instance.
  #[inline]
  pub fn new(bitfield: Bitfield) -> Self {
    TreeIndex { bitfield }
  }

  /// Get a bit from the bitfield.
  #[inline]
  pub fn get(&mut self, index: usize) -> bool {
    self.bitfield.get(index)
  }

  /// Set an index on the tree to `true`, and also all of the parents to the
  /// index. Walks the flat-tree upward, until it finds the upper most node.
  #[inline]
  pub fn set(&mut self, mut index: usize) -> Change {
    if self.bitfield.set(index, true).is_unchanged() {
      return Change::Unchanged;
    }

    while self.bitfield.get(flat::sibling(index)) {
      index = flat::parent(index);
      if self.bitfield.set(index, true).is_unchanged() {
        break;
      }
    }
    Change::Changed
  }

  /// Prove a method.
  #[inline]
  pub fn proof<'a>(
    &'a mut self,
    index: usize,
    include_hash: bool,
    nodes: &'a mut impl convert::AsMut<Vec<usize>>,
    remote_tree: &mut Self,
  ) -> Option<Proof> {
    let digest = 0;
    self.proof_with_digest(index, digest, include_hash, nodes, remote_tree)
  }

  /// Determine which Nodes prove the correctness for the Node at `index`.
  ///
  /// The passed buffer is filled with nodes that are verified by the same
  /// index. This is done so allocations can happen at the top level, and a
  /// buffer (pool) can be used to prevent extra allocations.
  // - opts.hash: always push index to nodes.
  // - nodes: proven nodes.
  // - opts.digest: not sure what digest does.
  pub fn proof_with_digest<'a>(
    &'a mut self,
    index: usize,
    mut digest: usize,
    include_hash: bool,
    nodes: &'a mut impl convert::AsMut<Vec<usize>>,
    remote_tree: &mut Self,
  ) -> Option<Proof> {
    let nodes = nodes.as_mut();

    if !self.get(index) {
      return None;
    }

    // Always return hash - no matter what the digest says.
    // NOTE: this feels like a mild hack.
    if include_hash {
      nodes.push(index);
    }

    if digest == 1 {
      let verified_by = 0;
      return Some(Proof::new(index, verified_by, nodes));
    }

    let mut roots = vec![]; // TODO: reuse from a buffer pool.
    let mut next = index;
    let mut sibling;
    let has_root = digest & 1;
    digest = shift_right(digest);

    while digest > 0 {
      if digest == 1 && has_root != 0 {
        if self.get(next) {
          remote_tree.set(next);
        }

        let next_sibling = flat::sibling(next);
        if next_sibling < next {
          next = next_sibling
        }

        flat::full_roots(flat::right_span(next) + 2, &mut roots);

        for root in roots.iter() {
          if self.get(*root) {
            remote_tree.set(*root);
          }
        }
        break;
      }

      sibling = flat::sibling(next);
      if !is_even(digest) && self.get(sibling) {
        remote_tree.set(sibling);
      }

      next = flat::parent(next);
      digest = shift_right(digest);
    }

    next = index;

    while !remote_tree.get(next) {
      sibling = flat::sibling(next);

      if !self.get(sibling) {
        let verified_by = self.verified_by(next).node;
        let mut roots = vec![];
        flat::full_roots(verified_by, &mut roots);
        for root in roots {
          if root != next && !remote_tree.get(root) {
            nodes.push(root);
          }
        }
        return Some(Proof::new(index, verified_by, nodes));
      } else if !remote_tree.get(sibling) {
        nodes.push(sibling);
      }

      next = flat::parent(next);
    }

    let verified_by = 0;
    Some(Proof::new(index, verified_by, nodes))
  }

  /// Create a digest for data at index.
  #[inline]
  pub fn digest(&mut self, index: usize) -> usize {
    if self.get(index) {
      return 1;
    }

    let mut digest = 0;
    let mut next = flat::sibling(index);
    let max = cmp::max(next + 2, self.bitfield.len()); // TODO(from mafintosh/hypercore): make this less hacky

    let mut bit = 2;
    let mut depth = flat::depth(index);
    let mut parent = flat::parent_with_depth(next, depth);
    depth += 1;

    while (flat::right_span(next) < max) || flat::left_span(parent) > 0 {
      if self.get(next) {
        digest |= bit;
      }

      if self.get(parent) {
        digest |= 2 * bit + 1;
        if digest + 1 == 4 * bit {
          return 1;
        }
        return digest;
      }

      next = flat::sibling(parent);
      parent = flat::parent_with_depth(next, depth);
      depth += 1;
      bit *= 2;
    }
    digest
  }

  /// Get the position of the highest entry in the tree. Aka max.
  ///
  /// NOTE: should we rename this to `.len()` ?
  #[inline]
  pub fn blocks(&mut self) -> usize {
    let mut top = 0;
    let mut next = 0;
    let max = self.bitfield.len();

    while flat::right_span(next) < max {
      next = flat::parent(next);
      if self.get(next) {
        top = next;
      }
    }

    if self.get(top) {
      self.verified_by(top).node / 2
    } else {
      0
    }
  }

  /// Get all root nodes.
  ///
  /// TODO: don't make this allocate, but fill a vector instead.
  #[inline]
  pub fn roots(&mut self, roots: &mut Vec<usize>) {
    flat::full_roots(2 * self.blocks(), roots)
  }

  /// Find the node that verified the node that's passed.
  ///
  /// This is different from the Javascript implementation in that it doesn't
  /// push the `top` value into an array, but returns it instead through the
  /// `Verification` type.
  #[inline]
  pub fn verified_by(&mut self, index: usize) -> Verification {
    let has_index = self.get(index);
    if !has_index {
      return Verification { node: 0, top: 0 };
    }

    // Find root of current tree.
    let mut depth = flat::depth(index);
    let mut top = index;
    let mut parent = flat::parent_with_depth(top, depth);
    depth += 1;
    while self.get(parent) && self.get(flat::sibling(top)) {
      top = parent;
      parent = flat::parent_with_depth(top, depth);
      depth += 1;
    }

    // Expand right down.
    //
    // NOTE: this is probably a candidate to move to `flat-tree`.
    depth -= 1;
    while depth != 0 {
      top = flat::left_child_with_depth(
        flat::index(depth, flat::offset_with_depth(top, depth) + 1),
        depth,
      ).unwrap();
      depth -= 1;

      while !self.get(top) && depth > 0 {
        top = flat::left_child_with_depth(top, depth).unwrap();
        depth -= 1;
      }
    }

    let node = if self.get(top) { top + 2 } else { top };

    Verification { node, top }
  }
}

/// Create a TreeIndex with an empty sparse_bitfield instance with a page size
/// of `1024`.
impl Default for TreeIndex {
  #[inline]
  fn default() -> Self {
    TreeIndex {
      bitfield: Bitfield::new(1024),
    }
  }
}

/// Check if a value is even.
#[inline]
fn is_even(n: usize) -> bool {
  match n & 1 {
    0 => true,
    1 => false,
    _ => panic!(format!(
      "Bitshift failure. Received bit {}. Expected 1 or 0",
      n
    )),
  }
}

/// Bitwise shift numbers one to the right. e.g. 1001 (9) becomes 0100 (4).
#[inline]
fn shift_right(n: usize) -> usize {
  (n - (n & 1)) / 2
}

#[test]
fn shifts_bits_right() {
  assert_eq!(shift_right(0), 0);
  assert_eq!(shift_right(9), 4);
  assert_eq!(shift_right(12), 6);
  assert_eq!(shift_right(13), 6);
  assert_eq!(shift_right(14), 7);
}
