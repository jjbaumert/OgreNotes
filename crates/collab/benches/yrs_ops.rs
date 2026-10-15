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

use criterion::{BatchSize, BenchmarkId, Criterion, black_box, criterion_group, criterion_main};

use ogrenotes_collab::document::{OgreDoc, get_content_fragment, get_or_insert_content_fragment};
use yrs::types::xml::{XmlElementPrelim, XmlFragment, XmlOut, XmlTextPrelim};
use yrs::{ReadTxn, StateVector, Transact, types::GetString};

/// Build a document with `n` paragraphs each containing `chars`
/// characters of synthetic text. Returns the doc and its snapshot
/// bytes. The bytes-size of the snapshot scales roughly linearly
/// with total text size; calibrate `n` and `chars` so the
/// `serialized.len()` lands in the target ballpark (10 KB / 1 MB).
fn build_doc(n_paragraphs: usize, chars_per_para: usize) -> (OgreDoc, Vec<u8>) {
    assert!(n_paragraphs > 0);
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
        // OgreDoc::new creates the first paragraph. Add the remaining
        // paragraphs as siblings so the CRDT topology matches the fixture.
        let XmlOut::Element(first) = fragment.get(&txn, 0).expect("root paragraph") else {
            panic!("expected root paragraph");
        };
        first.insert(&mut txn, 0, XmlTextPrelim::new(filler.as_str()));
        for index in 1..n_paragraphs {
            let para =
                fragment.insert(&mut txn, index as u32, XmlElementPrelim::empty("paragraph"));
            para.insert(&mut txn, 0, XmlTextPrelim::new(filler.as_str()));
        }
    }
    let bytes = doc.to_state_bytes();
    (doc, bytes)
}

/// Make a wire update relative to the exact baseline restored by each target.
/// Return the expected post-apply state vector for untimed verification.
fn build_update(base_bytes: &[u8], inserted_text: &str) -> (Vec<u8>, StateVector) {
    let doc = OgreDoc::from_state_bytes(base_bytes).unwrap();
    let baseline_state_vector = doc.state_vector();
    {
        let mut txn = doc.inner().transact_mut();
        let fragment = get_or_insert_content_fragment(&mut txn);
        let XmlOut::Element(para) = fragment.get(&txn, 0).expect("root paragraph") else {
            panic!("expected root paragraph");
        };
        para.insert(&mut txn, 0, XmlTextPrelim::new(inserted_text));
    }
    (
        doc.encode_diff(&baseline_state_vector).unwrap(),
        doc.inner().transact().state_vector(),
    )
}

/// Criterion drops batched inputs after timing the apply operation. Verify
/// every sample's resulting state and content at that untimed point.
struct CheckedApply<'a> {
    doc: OgreDoc,
    expected_state_vector: &'a StateVector,
    expected_text: &'a str,
}

impl Drop for CheckedApply<'_> {
    fn drop(&mut self) {
        let txn = self.doc.inner().transact();
        assert_eq!(&txn.state_vector(), self.expected_state_vector);
        let fragment = get_content_fragment(&txn).expect("content fragment");
        let Some(XmlOut::Element(para)) = fragment.get(&txn, 0) else {
            panic!("expected root paragraph");
        };
        let Some(XmlOut::Text(text)) = para.get(&txn, 0) else {
            panic!("expected inserted text");
        };
        assert_eq!(text.get_string(&txn), self.expected_text);
    }
}

fn bench_apply_small(c: &mut Criterion) {
    let (_, base_bytes) = build_doc(1, 1);
    let inserted_text = "x".repeat(10_000);
    let (update, expected_state_vector) = build_update(&base_bytes, &inserted_text);
    c.bench_with_input(
        BenchmarkId::new("yrs_apply_update", format!("{} bytes", update.len())),
        &update,
        |b, update| {
            // Measures decode + integration only. Baseline restoration and
            // post-apply verification/destruction are outside the timer.
            b.iter_batched_ref(
                || CheckedApply {
                    doc: OgreDoc::from_state_bytes(&base_bytes).unwrap(),
                    expected_state_vector: &expected_state_vector,
                    expected_text: &inserted_text,
                },
                |target| {
                    target
                        .doc
                        .apply_update(black_box(update.as_slice()))
                        .unwrap()
                },
                BatchSize::LargeInput,
            );
        },
    );
}

fn bench_apply_large_on_large(c: &mut Criterion) {
    // Build a ~1 MB doc and a ~100 KB update on top.
    let (_, base_bytes) = build_doc(40, 25_000);
    let inserted_text = "x".repeat(100_000);
    let (update, expected_state_vector) = build_update(&base_bytes, &inserted_text);
    c.bench_with_input(
        BenchmarkId::new("yrs_apply_update_large", format!("{} bytes", update.len())),
        &update,
        |b, update| {
            // A fresh equivalent target receives the same incremental edit
            // each time. PerIteration bounds memory for the 1 MB fixture.
            b.iter_batched_ref(
                || CheckedApply {
                    doc: OgreDoc::from_state_bytes(&base_bytes).unwrap(),
                    expected_state_vector: &expected_state_vector,
                    expected_text: &inserted_text,
                },
                |target| {
                    target
                        .doc
                        .apply_update(black_box(update.as_slice()))
                        .unwrap()
                },
                BatchSize::PerIteration,
            );
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
