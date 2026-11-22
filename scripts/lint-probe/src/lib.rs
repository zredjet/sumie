//! 禁じる口の代表（種類ごとに 1 つ以上）と、許す口。`sumie-tools check-lint-probe` が行ごとに照らす：
//! `// 期待: <パス> …` の行ではそのパスがすべて拾われ、`// 許す` の行では何も拾われないこと。
#![allow(clippy::all)]
#![warn(clippy::disallowed_methods, clippy::disallowed_types, clippy::disallowed_macros)]

use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, BinaryHeap, VecDeque};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

pub fn macros(x: u32) -> usize {
    assert!(x > 0); // 期待: core::assert
    assert_eq!(x, 1); // 期待: core::assert_eq
    assert_ne!(x, 2); // 期待: core::assert_ne
    debug_assert!(x > 0); // 期待: core::debug_assert
    debug_assert_eq!(x, 1); // 期待: core::debug_assert_eq
    debug_assert_ne!(x, 2); // 期待: core::debug_assert_ne
    let v = vec![1u8]; // 期待: alloc::vec
    let s = format!("{x}"); // 期待: alloc::format
    v.len() + s.len()
}

pub fn prims(s: &mut [u32], t: &str, c: char, f: f64) -> f64 {
    let _ = s.split_at(1); // 期待: slice::split_at
    let _ = s.chunks(2); // 期待: slice::chunks
    s.sort(); // 期待: slice::sort
    let _ = s.binary_search(&1); // 期待: slice::binary_search
    let _ = s.to_vec(); // 期待: slice::to_vec
    let _ = t.split_at(0); // 期待: str::split_at
    let _ = t.trim(); // 期待: str::trim
    let _ = c.is_whitespace(); // 期待: char::is_whitespace
    let _ = c.to_digit(10); // 期待: char::to_digit
    let _ = f.sin(); // 期待: f64::sin
    let _ = f.hypot(1.0); // 期待: f64::hypot
    let _ = 2u32.pow(3); // 期待: u32::pow
    let _ = (-2i32).abs(); // 期待: i32::abs
    let _ = 8u64.ilog2(); // 期待: u64::ilog2
    let _ = 7i64.div_euclid(2); // 期待: i64::div_euclid
    let a = f.max(1.0); // 期待: f64::max
    let b = f.min(1.0); // 期待: f64::min
    let c2 = f.clamp(0.0, 1.0); // 期待: f64::clamp
    let d = f.mul_add(2.0, 1.0); // 期待: f64::mul_add
    let e = f.powi(2); // 期待: f64::powi
    let g = f.to_degrees(); // 期待: f64::to_degrees
    let h = f.algebraic_add(1.0); // 期待: f64::algebraic_add
    a + b + c2 + d + e + g + h
}

pub fn structs(r: &RefCell<u8>, k: &Cell<u8>, o: Option<u8>) -> usize {
    let _ = r.borrow(); // 期待: core::cell::RefCell::borrow
    k.swap(k); // 期待: core::cell::Cell::swap
    let _ = o.unwrap(); // 期待: core::option::Option::unwrap
    let mut v: Vec<u8> = Vec::with_capacity(1); // 期待: alloc::vec::Vec::with_capacity
    v.push(1); // 期待: alloc::vec::Vec::push
    v.insert(0, 2); // 期待: alloc::vec::Vec::insert
    let mut s = String::new(); // 期待: alloc::string::String::new
    s.insert(0, 'a'); // 期待: alloc::string::String::insert
    let b = Box::new(1u8); // 期待: alloc::boxed::Box::new
    let a = Arc::new(1u8); // 期待: alloc::sync::Arc::new
    v.len() + *b as usize + *a as usize
}

pub fn traits(v: &[u32]) -> u32 {
    let s: u32 = v.iter().sum(); // 期待: core::iter::traits::iterator::Iterator::sum
    let w: Vec<u32> = v.iter().copied().collect(); // 期待: core::iter::traits::iterator::Iterator::collect
    let w2 = w.clone(); // 期待: core::clone::Clone::clone
    let st = s.to_string(); // 期待: alloc::string::ToString::to_string
    let ow = "x".to_owned(); // 期待: alloc::borrow::ToOwned::to_owned
    let mut e: Vec<u32> = Vec::new(); // 期待: alloc::vec::Vec::new
    e.extend(w2.iter()); // 期待: core::iter::traits::collect::Extend::extend
    let n = v.iter().step_by(2).count() as u32; // 期待: core::iter::traits::iterator::Iterator::step_by
    let c = 5u32.clamp(1, 3); // 期待: core::cmp::Ord::clamp
    s + n + c + st.len() as u32 + ow.len() as u32 + e.len() as u32
}

pub fn types() -> usize {
    let t = std::time::Instant::now(); // 期待: std::time::Instant::now std::time::Instant
    let _ = std::env::var("X"); // 期待: std::env::var
    let _ = std::thread::spawn(|| ()); // 期待: std::thread::spawn
    let _ = t;
    let m: BTreeMap<u8, u8> = BTreeMap::new(); // 期待: alloc::collections::BTreeMap alloc::collections::btree::map::BTreeMap::new
    let d: VecDeque<u8> = VecDeque::new(); // 期待: alloc::collections::VecDeque alloc::collections::vec_deque::VecDeque::new
    let h: BinaryHeap<u8> = BinaryHeap::new(); // 期待: alloc::collections::BinaryHeap alloc::collections::binary_heap::BinaryHeap::new
    m.len() + d.len() + h.len()
}

/// 許す口（拾われてはいけない）。
pub fn allowed(v: &[u32], f: f64, flag: &AtomicBool) -> f64 {
    let _ = flag.load(Ordering::Relaxed); // 許す
    let n = v.iter().enumerate().count() as f64; // 許す
    let _ = 2u32.checked_pow(3); // 許す
    let _ = 2u32.wrapping_add(3); // 許す
    let _ = (-2i32).unsigned_abs(); // 許す
    let _ = 2u32.max(3).min(4); // 許す
    let _ = f.total_cmp(&1.0); // 許す
    n + f.abs() + f.floor() + f.ceil() + f.sqrt() + f.round() + f.trunc() + f.copysign(1.0) // 許す
}
