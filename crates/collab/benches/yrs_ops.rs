// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! Phase 5 M-P9 piece D — criterion benchmarks for the hottest
//! collab-crate paths.
//!
//! Targets the four most-trafficked CRDT operations in steady-state
//! request handling: applying a small incremental update (the
//! WebSocket fast path), applying a large incremental update on top
//! of a large doc (reconnect / cold-start path), serializing a
//! mid-sized doc to snapshot bytes (S3 write path), and
//! deserializing from snapshot bytes (S3 read / room-load path).
//!
//! Baselines are recorded by the criterion harness under
//! `target/criterion/`. The CI gate `scripts/check-bench-regression.sh`
//! (M-P9 piece D follow-up) compares each bench's mean against
//! main and fails the PR if any has slowed > 20 %. See
//! `design/performance-budgets.md` → "Backend criterion benchmarks".

use criterion::{black_box, criterion_group, criterion_main, Criterion, BenchmarkId};

use ogrenotes_collab::document::{get_or_insert_content_fragment, OgreDoc};
use yrs::types::xml::{XmlFragment, XmlOut, XmlTextPrelim};
use yrs::Transact;

/// Build a document with `n` paragraphs each containing `chars`
/// characters of synthetic text. Returns the doc and its snapshot
/// bytes. The bytes-size of the snapshot scales roughly linearly
/// with total text size; calibrate `n` and `chars` so the
/// `serialized.len()` lands in the target ballpark (10 KB / 1 MB).
fn build_doc(n_paragraphs: usize, chars_per_para: usize) -> (OgreDoc, Vec<u8>) {
    let doc = OgreDoc::new();
    // Synthetic filler — repeat-pattern text so the encoder doesn't
    // hit any pathological best-case (every char identical) or
    // worst-case (high entropy) shortcut; "lorem ipsum" style ASCII
    // is representative of real document content.
    let filler: String = "lorem ipsum dolor sit amet "
        .chars()
        .cycle()
        .take(chars_per_para)
        .collect();
    {
        let mut txn = doc.inner().transact_mut();
        let fragment = get_or_insert_content_fragment(&mut txn);
        // Reuse the first paragraph created by OgreDoc::new for
        // index 0; insert (n-1) more after it.
        if let Some(XmlOut::Element(first)) = fragment.get(&txn, 0) {
            first.insert(&mut txn, 0, XmlTextPrelim::new(filler.as_str()));
        }
        for _ in 1..n_paragraphs {
            // Each new paragraph: insert an empty XmlElementPrelim
            // would require importing the type, but XmlTextPrelim
            // appended at the end with a newline-y separator suffices
            // for sizing the snapshot — the bench is about CRDT cost,
            // not document semantics.
            if let Some(XmlOut::Element(first)) = fragment.get(&txn, 0) {
                first.insert(&mut txn, 0, XmlTextPrelim::new(filler.as_str()));
            }
        }
    }
    let bytes = doc.to_state_bytes();
    (doc, bytes)
}

/// Build an incremental update of approximately `chars` characters
/// applied to a fresh doc — returns the encoded update bytes.
/// Used as the input to `apply_update` benches so we measure
/// decode + apply, not the source-doc construction.
fn build_update(chars: usize) -> Vec<u8> {
    let (doc, base_bytes) = build_doc(1, 1); // baseline of 1 char
    let _ = base_bytes;
    // Now insert `chars` characters and capture the diff.
    {
        let mut txn = doc.inner().transact_mut();
        let fragment = get_or_insert_content_fragment(&mut txn);
        if let Some(XmlOut::Element(para)) = fragment.get(&txn, 0) {
            let filler: String = "x".repeat(chars);
            para.insert(&mut txn, 0, XmlTextPrelim::new(filler.as_str()));
        }
    }
    // Encode the entire state — `apply_update` against an empty doc
    // is the canonical "incremental update arriving over the wire"
    // benchmark, even though the wire format is actually a state
    // delta in production.
    doc.to_state_bytes()
}

fn bench_apply_small(c: &mut Criterion) {
    let update = build_update(10_000);
    c.bench_with_input(
        BenchmarkId::new("yrs_apply_update", format!("{} bytes", update.len())),
        &update,
        |b, update| {
            b.iter(|| {
                let mut doc = OgreDoc::new();
                doc.apply_update(black_box(update.as_slice())).unwrap();
                black_box(doc)
            });
        },
    );
}

fn bench_apply_large_on_large(c: &mut Criterion) {
    // Build a ~1 MB doc and a ~100 KB update on top.
    let (mut base_doc, _) = build_doc(40, 25_000);
    let update = build_update(100_000);
    c.bench_with_input(
        BenchmarkId::new(
            "yrs_apply_update_large",
            format!("{} bytes", update.len()),
        ),
        &update,
        |b, update| {
            b.iter(|| {
                base_doc.apply_update(black_box(update.as_slice())).unwrap();
            });
        },
    );
}

fn bench_serialize(c: &mut Criterion) {
    let (doc, bytes) = build_doc(40, 25_000);
    c.bench_with_input(
        BenchmarkId::new("doc_serialize", format!("{} bytes", bytes.len())),
        &doc,
        |b, doc| {
            b.iter(|| {
                let out = doc.to_state_bytes();
                black_box(out)
            });
        },
    );
}

fn bench_deserialize(c: &mut Criterion) {
    let (_, bytes) = build_doc(40, 25_000);
    c.bench_with_input(
        BenchmarkId::new("doc_deserialize", format!("{} bytes", bytes.len())),
        &bytes,
        |b, bytes| {
            b.iter(|| {
                let doc = OgreDoc::from_state_bytes(black_box(bytes.as_slice())).unwrap();
                black_box(doc)
            });
        },
    );
}

criterion_group!(
    benches,
    bench_apply_small,
    bench_apply_large_on_large,
    bench_serialize,
    bench_deserialize,
);
criterion_main!(benches);
