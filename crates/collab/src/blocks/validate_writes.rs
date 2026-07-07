// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! Pre-apply validation for LiveApp-block CRDT writes.
//!
//! The paste/import path already runs `LiveAppBlock::validate_attrs`
//! before landing a node in the doc, but interactive yrs updates
//! bypass that gate — a client can craft an update that sets
//! arbitrary attrs on any LiveApp NodeType. This module lets the
//! interactive write path speculatively apply an update to a scratch
//! doc clone, walk the resulting tree, and reject the whole update
//! if any LiveApp node's attrs fail validation.
//!
//! ## Approach
//!
//! Scratch-doc-clone-then-apply, not update-payload-parsing. yrs's
//! `Update` struct is version-unstable; cloning the doc's state
//! bytes and re-applying the update gives us a deterministic tree
//! to walk.
//!
//! ## Cost
//!
//! Two walk modes; select via `WalkScope`.
//!
//! - **`Full`** — walks the entire post-apply tree via
//!   `walk_doc`. Every LiveApp node in the doc is validated on
//!   every write. O(doc size). This is the pre-Phase-3 behavior
//!   and the safe default until the canary rollout has proven
//!   `Changed` equivalent.
//! - **`Changed`** — walks only elements the transaction
//!   touched, via `walk_changed`. Uses yrs's `observe_deep` on
//!   the scratch's root fragment: every touched branch fires an
//!   event whose target is the `XmlElementRef` we then feed to
//!   the validator. O(touched-elements). Fixes gap-001 from the
//!   post-hardening audit — a pre-existing invalid attribute on
//!   card X no longer blocks a write that only touches card Y
//!   (or an unrelated paragraph).
//! - **`Canary`** — run BOTH walks and emit
//!   `liveapp.gate_walk_canary_mismatch_total{node_type,field,direction}`
//!   on disagreement. Falls back to `Full`'s answer (safe
//!   default). Rollout mode until the canary metric stays zero
//!   for a safe-observation window.
//!
//! Both modes still pay the state-bytes clone + scratch apply
//! cost — that's fundamental to speculating an apply we might
//! reject. The walk itself is where `Changed` wins.
//!
//! ## Non-goals
//!
//! - Does NOT canonicalize the write. Reject-or-accept only.
//! - Does NOT run on `room::apply_update` (compaction / snapshot
//!   restore). Only interactive `documents.rs` / `ws.rs` paths.

use ogrenotes_common::metrics::{counter, MetricKey};
use yrs::{Doc, Out, ReadTxn, Transact, WriteTxn, Update};
use yrs::types::xml::{Xml, XmlElementRef, XmlFragment, XmlOut};
use yrs::types::DeepObservable;
use yrs::updates::decoder::Decode;
use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use super::{block_for, BlockValidationError};
use crate::document::OgreDoc;
use crate::schema::NodeType;

/// A single LiveApp-attr violation surfaced by validation. Carries
/// both the underlying block-validator error and the block-id path
/// (if resolvable) so operators can trace which node produced it.
#[derive(Debug, Clone)]
pub struct LiveAppViolation {
    pub node_type: NodeType,
    pub field: Cow<'static, str>,
    pub reason: String,
    /// The block-id attr of the offending node, if it had one.
    /// Not every LiveApp node carries a blockId — some are
    /// managed purely through their parent's child list.
    pub block_id: Option<String>,
}

impl From<(BlockValidationError, Option<String>)> for LiveAppViolation {
    fn from((e, block_id): (BlockValidationError, Option<String>)) -> Self {
        Self {
            node_type: e.node_type,
            field: e.field,
            reason: e.reason,
            block_id,
        }
    }
}

/// Which walk to run when validating a pre-apply update.
///
/// - `Full` — pre-Phase-3 behavior. Walk the entire post-apply
///   tree via `walk_doc`. Correct but O(doc size), and a
///   pre-existing invalid attribute anywhere blocks every
///   future write.
/// - `Changed` — walk only elements the transaction touched
///   (via `observe_deep`). O(changed) — the fix for
///   gap-001 (post-hardening audit).
/// - `Canary` — run BOTH walks and compare the violation
///   sets. On disagreement, emit
///   `liveapp.gate_walk_canary_mismatch_total` and fall back to
///   `Full`'s answer (safe default). Rollout mode until the
///   canary metric stays zero for a safe-observation window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WalkScope {
    Full,
    Changed,
    Canary,
}

impl WalkScope {
    /// Parse from the `LIVEAPP_GATE_WALK_SCOPE` env var. Unknown
    /// values fall back to `Full` — the safe default during
    /// rollout.
    pub fn from_env_value(v: Option<&str>) -> Self {
        match v.map(str::trim).map(str::to_ascii_lowercase).as_deref() {
            Some("changed") => Self::Changed,
            Some("canary") => Self::Canary,
            _ => Self::Full,
        }
    }
}

/// Speculatively apply `update_bytes` to a fresh clone of `doc`,
/// then walk the resulting tree and check every LiveApp node's
/// attrs against its block's `validate_attrs`.
///
/// Returns `Ok(())` when every LiveApp node in the post-apply
/// tree validates, or `Err(violations)` when one or more fail.
///
/// Non-LiveApp NodeTypes (paragraph, heading, table, etc.) are
/// skipped — `block_for` returns `None` for them.
///
/// # Failure modes
///
/// - **Decode failure**: an update that yrs can't decode returns
///   `Ok(())`. The apply-side will hit the same decode error and
///   surface a canonical decode-failure metric; there is nothing
///   for this validator to add. Decode-then-apply on a scratch
///   avoids the write from ever reaching the real doc if it does
///   decode but violates.
/// - **Apply failure on scratch**: returned as `Ok(())` for the
///   same reason. The real apply will fail identically.
pub fn validate_liveapp_writes(
    doc: &OgreDoc,
    update_bytes: &[u8],
) -> Result<(), Vec<LiveAppViolation>> {
    validate_liveapp_writes_scoped(doc, update_bytes, WalkScope::Full)
}

/// Same as `validate_liveapp_writes` but takes a `WalkScope`
/// selecting whole-doc walk vs the (Phase 3) changed-refs walk.
/// The wrapper above preserves the pre-Phase-3 behavior for any
/// caller that hasn't opted in to the new scope knob.
pub fn validate_liveapp_writes_scoped(
    doc: &OgreDoc,
    update_bytes: &[u8],
    scope: WalkScope,
) -> Result<(), Vec<LiveAppViolation>> {
    // Clone the doc via its state-bytes. Cheaper than a
    // deep-walk-and-rebuild, and preserves every yrs invariant
    // (client IDs, timestamps, GC markers) so the scratch apply
    // reaches the same tree the real apply would.
    let state = doc.to_state_bytes();
    let scratch = Doc::new();
    {
        let mut txn = scratch.transact_mut();
        let Ok(base_update) = Update::decode_v1(&state) else {
            return Ok(());
        };
        if txn.apply_update(base_update).is_err() {
            return Ok(());
        }
    }

    // Attach the observe_deep subscription BEFORE the transaction
    // that applies the incoming update, so the observer catches
    // every touched XmlElement. The subscription is dropped when
    // this function returns; if `scope == Full` the touched-set
    // is never read, so paying for the subscription only makes
    // sense when we'll actually use it.
    let touched: Arc<Mutex<Vec<XmlElementRef>>> = Arc::new(Mutex::new(Vec::new()));
    let _subscription = if matches!(scope, WalkScope::Changed | WalkScope::Canary) {
        let capture = Arc::clone(&touched);
        // Subscribe on the root content fragment — every event in
        // the LiveApp subtree bubbles through the deep observer.
        let root = {
            let mut txn = scratch.transact_mut();
            crate::document::get_or_insert_content_fragment(&mut txn)
        };
        Some(root.observe_deep(move |_txn, events| {
            if let Ok(mut buf) = capture.lock() {
                for event in events.iter() {
                    if let Out::YXmlElement(el) = event.target() {
                        buf.push(el);
                    }
                }
            }
        }))
    } else {
        None
    };

    {
        let mut txn = scratch.transact_mut();
        let Ok(new_update) = Update::decode_v1(update_bytes) else {
            return Ok(());
        };
        if txn.apply_update(new_update).is_err() {
            return Ok(());
        }
        // Drop of `txn` triggers commit, which fires observers,
        // which populate `touched`.
    }

    let violations = match scope {
        WalkScope::Full => walk_doc(&scratch),
        WalkScope::Changed => {
            let refs = touched.lock().map(|g| g.clone()).unwrap_or_default();
            walk_changed(&scratch, &refs)
        }
        WalkScope::Canary => {
            let refs = touched.lock().map(|g| g.clone()).unwrap_or_default();
            let full = walk_doc(&scratch);
            let changed = walk_changed(&scratch, &refs);
            if !violation_sets_equivalent(&full, &changed) {
                for missing in violation_diff(&full, &changed) {
                    counter::inc(MetricKey::new(
                        "liveapp.gate_walk_canary_mismatch_total",
                        &[
                            ("node_type", missing.node_type.tag_name()),
                            ("field", missing.field.as_ref()),
                            ("direction", "full_only"),
                        ],
                    ));
                }
                for extra in violation_diff(&changed, &full) {
                    counter::inc(MetricKey::new(
                        "liveapp.gate_walk_canary_mismatch_total",
                        &[
                            ("node_type", extra.node_type.tag_name()),
                            ("field", extra.field.as_ref()),
                            ("direction", "changed_only"),
                        ],
                    ));
                }
            }
            // Fall back to Full's answer under canary — safest
            // during rollout.
            full
        }
    };
    if violations.is_empty() {
        Ok(())
    } else {
        Err(violations)
    }
}

/// Walk only the XmlElements touched by the current transaction
/// (Phase 3 — the gap-001 fix). The observer fires a deep event
/// for every touched branch during the transaction commit; this
/// function iterates that captured set and runs the LiveApp
/// validator on each element that is a LiveApp NodeType.
///
/// **Descent policy.** yrs's deep observer fires an event per
/// touched branch (attr write → target = that element; child
/// insertion → target = the parent). So a new KanbanCard
/// inserted with attrs surfaces via BOTH its parent column
/// (children-changed event) and the card itself (attribute
/// events). We validate each observed element in isolation and
/// rely on the observer's coverage rather than doing an explicit
/// walk of container children. The canary rollout mode
/// (`WalkScope::Canary`) is the coverage check for this
/// assumption.
pub fn walk_changed(_doc: &Doc, touched: &[XmlElementRef]) -> Vec<LiveAppViolation> {
    let mut violations: Vec<LiveAppViolation> = Vec::new();
    // Use a read transaction for the doc lifetime of the walk;
    // XmlElementRefs are just BranchPtrs into the same store.
    let txn = _doc.transact();
    // Dedup by pointer identity — the deep observer may fire
    // multiple events per branch (e.g. children AND attributes
    // change in the same transaction).
    let mut seen: Vec<*const ()> = Vec::new();
    for el in touched {
        let key = el as *const _ as *const ();
        if seen.contains(&key) {
            continue;
        }
        seen.push(key);
        walk_element(&txn, el, &mut violations);
    }
    violations
}

/// Compare two violation sets ignoring order. Two sets are
/// equivalent iff each side's violations project to the same
/// multi-set of `(node_type, field, reason)` triples.
///
/// Used by canary rollout mode to catch a silent divergence
/// between `walk_doc` and `walk_changed` before flipping the
/// default.
fn violation_sets_equivalent(
    a: &[LiveAppViolation],
    b: &[LiveAppViolation],
) -> bool {
    if a.len() != b.len() {
        return false;
    }
    // NodeType is not Ord, so key by its tag_name string. The
    // 20-way ALL_NODE_TYPES set is sparse enough that string
    // comparison is negligible compared to the walk itself.
    let mut ka: Vec<(&'static str, String, String)> = a
        .iter()
        .map(|v| (v.node_type.tag_name(), v.field.to_string(), v.reason.clone()))
        .collect();
    let mut kb: Vec<(&'static str, String, String)> = b
        .iter()
        .map(|v| (v.node_type.tag_name(), v.field.to_string(), v.reason.clone()))
        .collect();
    ka.sort();
    kb.sort();
    ka == kb
}

/// Return violations present in `a` but missing from `b`. Used
/// by canary rollout to attribute mismatches to a specific side.
fn violation_diff<'a>(
    a: &'a [LiveAppViolation],
    b: &[LiveAppViolation],
) -> Vec<&'a LiveAppViolation> {
    a.iter()
        .filter(|v| {
            !b.iter().any(|other| {
                other.node_type == v.node_type
                    && other.field == v.field
                    && other.reason == v.reason
            })
        })
        .collect()
}

/// A LiveApp sub-node that was removed from a doc between two
/// walks. Produced by `diff_liveapp_deletions` and consumed by
/// the WS handler to emit `SecurityAuditAction::LiveAppNodeDeleted`
/// rows. gap-003 from the post-hardening security audit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LiveAppDeletion {
    pub node_type: NodeType,
    /// Empty when the deleted node had no `blockId` attribute.
    pub block_id: String,
}

/// A compact index of the LiveApp nodes present in a doc at a
/// point in time — `(node_type, block_id)` pairs. Cheap to
/// snapshot before an apply and diff against a post-apply
/// snapshot to detect deletions.
///
/// gap-003: the diff is what the WS handler uses to emit
/// `LiveAppNodeDeleted` audit rows for interactive deletes
/// (Kanban card, KanbanColumn, CalendarEvent).
pub type LiveAppIndex = Vec<(NodeType, String)>;

/// Walk a doc's `content` fragment and collect a
/// `(node_type, block_id)` entry per LiveApp node. Deterministic
/// order (fragment order + child order) so a lexical diff between
/// two snapshots is stable across `changed` and `full` scopes.
///
/// Cost: O(LiveApp-node count) per call. Two calls (pre + post)
/// are required per gated apply if the deletion signal is
/// wanted; both happen under the room write lock.
pub fn collect_liveapp_index(doc: &Doc) -> LiveAppIndex {
    let mut out: LiveAppIndex = Vec::new();
    let txn = doc.transact();
    let Some(fragment) = txn.get_xml_fragment("content") else {
        return out;
    };
    let len = fragment.len(&txn);
    for i in 0..len {
        if let Some(XmlOut::Element(el)) = fragment.get(&txn, i) {
            walk_element_for_index(&txn, &el, &mut out);
        }
    }
    out
}

fn walk_element_for_index<T: ReadTxn>(
    txn: &T,
    el: &yrs::XmlElementRef,
    out: &mut LiveAppIndex,
) {
    if let Some(nt) = NodeType::from_tag(el.tag().as_ref()) {
        if block_for(nt).is_some() {
            let block_id = collect_attrs(txn, el)
                .get("blockId")
                .cloned()
                .unwrap_or_default();
            out.push((nt, block_id));
        }
    }
    let child_len = el.len(txn);
    for i in 0..child_len {
        if let Some(XmlOut::Element(child)) = el.get(txn, i) {
            walk_element_for_index(txn, &child, out);
        }
    }
}

/// Return the (node_type, block_id) pairs present in `pre` but
/// absent from `post`. Matches on the pair — two different
/// blocks with the same tag but different block_ids are treated
/// as distinct.
///
/// When two nodes share the exact same (node_type, block_id)
/// pair (e.g. both have no blockId), we count them once each on
/// both sides and only report deletions when the pre-count
/// exceeds the post-count. That guards against a false positive
/// when a duplicate-blockId cluster shrinks by one.
pub fn diff_liveapp_deletions(
    pre: &LiveAppIndex,
    post: &LiveAppIndex,
) -> Vec<LiveAppDeletion> {
    use std::collections::HashMap;
    let mut post_counts: HashMap<(NodeType, &str), usize> = HashMap::new();
    for (nt, bid) in post {
        *post_counts.entry((*nt, bid.as_str())).or_insert(0) += 1;
    }
    let mut result = Vec::new();
    for (nt, bid) in pre {
        let key = (*nt, bid.as_str());
        let counter = post_counts.entry(key).or_insert(0);
        if *counter == 0 {
            result.push(LiveAppDeletion {
                node_type: *nt,
                block_id: bid.clone(),
            });
        } else {
            *counter -= 1;
        }
    }
    result
}

/// Walk a doc's `content` fragment and collect every LiveApp
/// attribute-validation violation.
///
/// Public because both the WS pre-apply gate and the REST
/// full-state-upload path (`PUT /documents/:id/content`) need
/// to validate an already-materialized doc. The WS path
/// materializes via scratch-clone-then-apply; the REST path
/// materializes via `from_state_bytes`.
pub fn walk_doc(doc: &Doc) -> Vec<LiveAppViolation> {
    let mut violations: Vec<LiveAppViolation> = Vec::new();
    let txn = doc.transact();
    let Some(fragment) = txn.get_xml_fragment("content") else {
        return violations;
    };
    let len = fragment.len(&txn);
    for i in 0..len {
        if let Some(XmlOut::Element(el)) = fragment.get(&txn, i) {
            walk_element(&txn, &el, &mut violations);
        }
    }
    violations
}

fn walk_element<T: ReadTxn>(
    txn: &T,
    el: &yrs::XmlElementRef,
    out: &mut Vec<LiveAppViolation>,
) {
    let tag = el.tag();
    if let Some(nt) = NodeType::from_tag(tag.as_ref()) {
        if let Some(block) = block_for(nt) {
            let attrs = collect_attrs(txn, el);
            let block_id = attrs.get("blockId").cloned();
            match block.validate_attrs(nt, &attrs) {
                Err(e) => {
                    out.push((e, block_id).into());
                }
                Ok(canonical) => {
                    // Strict comparison: `validate_attrs` clamps
                    // oversized values silently and returns Ok
                    // (paste/import wants that friendly behavior).
                    // For the interactive CRDT-write gate, though,
                    // silent-clamp defeats the purpose — an
                    // attacker's oversized attr would still land
                    // in the doc. So we compare each input attr
                    // against the canonical form and flag any
                    // divergence (including unknown-key drops).
                    surface_canonicalization_diff(
                        nt, &attrs, &canonical, block_id, out,
                    );
                }
            }
        }
    }
    // Descend regardless of whether this element is a LiveApp node —
    // a LiveApp block can sit inside a non-LiveApp container
    // (blockquote, list_item), and a non-LiveApp node can contain
    // a nested LiveApp (unlikely today but not forbidden by schema).
    let child_len = el.len(txn);
    for i in 0..child_len {
        if let Some(XmlOut::Element(child)) = el.get(txn, i) {
            walk_element(txn, &child, out);
        }
    }
}

/// Compare raw input attrs against the block's canonical form and
/// push a violation per attr that diverges. Empty input values
/// are skipped — the block-level validators intentionally treat
/// an absent-or-empty attr as "no assertion made" and default
/// the canonical form, which is the desired behavior even in
/// strict mode.
fn surface_canonicalization_diff(
    node_type: NodeType,
    input: &HashMap<String, String>,
    canonical: &HashMap<String, String>,
    block_id: Option<String>,
    out: &mut Vec<LiveAppViolation>,
) {
    for (key, value) in input {
        if value.is_empty() {
            continue;
        }
        // The blockId attr is not schema-managed — every LiveApp
        // node stamps its own runtime id, and the block-level
        // validators pass it through unchanged. Excluding it from
        // the strict check keeps every legitimate LiveApp write
        // from tripping over its own identity attribute.
        if key == "blockId" {
            continue;
        }
        match canonical.get(key) {
            Some(canon) if canon == value => continue,
            Some(canon) => out.push(LiveAppViolation {
                node_type,
                field: Cow::Owned(key.clone()),
                reason: format!(
                    "value was canonicalized: input {value:?}, canonical {canon:?}"
                ),
                block_id: block_id.clone(),
            }),
            None => out.push(LiveAppViolation {
                node_type,
                field: Cow::Owned(key.clone()),
                reason: format!(
                    "unknown attribute (input {value:?}) dropped by validator"
                ),
                block_id: block_id.clone(),
            }),
        }
    }
}

fn collect_attrs<T: ReadTxn>(
    txn: &T,
    el: &yrs::XmlElementRef,
) -> HashMap<String, String> {
    let mut out = HashMap::new();
    // `Attributes` iterator yields `(&str, String)` — yrs
    // already coerces every attribute value to its string
    // representation for us.
    for (k, v) in el.attributes(txn) {
        out.insert(k.to_string(), v);
    }
    out
}

/// Report from `repair_liveapp_attrs`. Counts LiveApp nodes whose
/// current attrs were rewritten to their canonical form.
#[derive(Debug, Clone, Default)]
pub struct RepairReport {
    /// Total LiveApp nodes that had at least one attribute changed.
    pub nodes_touched: usize,
    /// (node_type, block_id, field_that_was_changed) triples the
    /// caller can log or return in the response. Bounded — 32
    /// entries max, additional changes still increment
    /// `nodes_touched` but stop appending.
    pub changes: Vec<(NodeType, Option<String>, String)>,
}

/// Walk `doc` and canonicalize every LiveApp node whose current
/// attrs diverge from what `validate_attrs` would return. Writes
/// happen inside a single yrs transaction — one atomic delta
/// captures the whole repair, so peers see it as one update.
///
/// Repair strategy per node:
/// - If `validate_attrs(current) == Err(...)`, the current
///   attrs are structurally invalid (bad enum, malformed date).
///   Delete every attr on the node and re-insert only the keys
///   the block's canonical answer would have, using DEFAULT
///   values (validate_attrs's own defaults for missing input).
/// - If `validate_attrs(current) == Ok(canonical)` but any
///   canonical value differs from what the node stores, write
///   the canonical value for each divergent key. Unknown /
///   dropped keys are removed.
///
/// Returns the number of nodes whose attrs the walk actually
/// modified.
///
/// Called by the admin repair endpoint
/// (`POST /admin/documents/:id/repair-liveapp-attrs`).
pub fn repair_liveapp_attrs(doc: &Doc) -> RepairReport {
    const MAX_LOGGED_CHANGES: usize = 32;
    let mut report = RepairReport::default();

    // First pass: read-only walk to collect the plan. We can't
    // mutate under an active read txn, so gather everything
    // first, then apply in one write txn.
    let plans: Vec<RepairPlan> = {
        let txn = doc.transact();
        let Some(fragment) = txn.get_xml_fragment("content") else {
            return report;
        };
        let mut plans = Vec::new();
        collect_repair_plans(&txn, &fragment, &mut plans);
        plans
    };

    if plans.is_empty() {
        return report;
    }

    // Second pass: apply all rewrites in one write txn.
    let mut txn = doc.transact_mut();
    for plan in plans {
        let mut any_change = false;
        // Snapshot current attrs so we can log deltas.
        let before = collect_attrs(&txn, &plan.el);
        // Remove dropped-by-canonicalizer keys.
        for k in &plan.removes {
            plan.el.remove_attribute(&mut txn, k);
            any_change = true;
        }
        // Write canonical values.
        for (k, v) in &plan.canonical {
            if before.get(k).map(String::as_str) != Some(v.as_str()) {
                plan.el.insert_attribute(&mut txn, k.as_str(), v.as_str());
                any_change = true;
                if report.changes.len() < MAX_LOGGED_CHANGES {
                    report
                        .changes
                        .push((plan.node_type, plan.block_id.clone(), k.clone()));
                }
            }
        }
        if any_change {
            report.nodes_touched += 1;
        }
    }
    report
}

/// Walk a fragment and collect one repair-plan entry per LiveApp
/// node whose current attrs diverge from canonical. Read-only; the
/// second pass in `repair_liveapp_attrs` applies the writes.
fn collect_repair_plans<T: ReadTxn>(
    txn: &T,
    fragment: &yrs::XmlFragmentRef,
    out: &mut Vec<RepairPlan>,
) {
    let len = fragment.len(txn);
    for i in 0..len {
        if let Some(XmlOut::Element(el)) = fragment.get(txn, i) {
            plan_element(txn, &el, out);
        }
    }
}

// Local struct — decouples plan collection from the write-pass
// struct so `repair_liveapp_attrs` owns the aliasing / lifetime
// details in one place.
struct RepairPlan {
    el: yrs::XmlElementRef,
    node_type: NodeType,
    block_id: Option<String>,
    canonical: HashMap<String, String>,
    removes: Vec<String>,
}

fn plan_element<T: ReadTxn>(
    txn: &T,
    el: &yrs::XmlElementRef,
    out: &mut Vec<RepairPlan>,
) {
    let tag = el.tag();
    if let Some(nt) = NodeType::from_tag(tag.as_ref()) {
        if let Some(block) = block_for(nt) {
            let attrs = collect_attrs(txn, el);
            let block_id = attrs.get("blockId").cloned();
            let canonical = canonicalize_via_iteration(block, nt, &attrs);
            // Which stored keys must be removed: any non-blockId
            // input key that (a) validate_attrs canonically drops
            // (canonical map has no such key) or (b) whose stored
            // value differs from the canonical value AND the
            // canonical value is empty (the block dropped it).
            let mut removes: Vec<String> = Vec::new();
            for (k, v) in &attrs {
                if k == "blockId" {
                    continue;
                }
                match canonical.get(k) {
                    None => removes.push(k.clone()),
                    Some(canon) if canon.is_empty() && !v.is_empty() => {
                        // Canonical form has this attr as empty
                        // (block signals "drop"); stored has a
                        // non-empty value. Remove.
                        removes.push(k.clone());
                    }
                    _ => {}
                }
            }
            // Compute which canonical values must be written.
            // Skip canonical keys whose value is empty AND that
            // aren't stored — matches strict-compare semantics
            // where empty input keys are "no assertion made."
            let needs_write = canonical.iter().any(|(k, v)| {
                let stored = attrs.get(k).map(String::as_str);
                if v.is_empty() {
                    // The block canonicalized to an empty default.
                    // Only relevant if stored had a non-empty value
                    // (already covered by `removes`); ignore here.
                    false
                } else {
                    stored != Some(v.as_str())
                }
            });
            if needs_write || !removes.is_empty() {
                out.push(RepairPlan {
                    el: el.clone(),
                    node_type: nt,
                    block_id,
                    canonical,
                    removes,
                });
            }
        }
    }
    let child_len = el.len(txn);
    for i in 0..child_len {
        if let Some(XmlOut::Element(child)) = el.get(txn, i) {
            plan_element(txn, &child, out);
        }
    }
}

/// Iteratively strip offending fields until `validate_attrs`
/// returns Ok. If the block validator refuses on a "field
/// required" error, inject a placeholder to unstick it.
///
/// Used only by `repair_liveapp_attrs`. On the interactive gate
/// path we reject the whole update instead of trying to salvage.
fn canonicalize_via_iteration(
    block: &'static (dyn super::LiveAppBlock + 'static),
    nt: NodeType,
    initial: &HashMap<String, String>,
) -> HashMap<String, String> {
    let mut attempt = initial.clone();
    for _ in 0..16 {
        match block.validate_attrs(nt, &attempt) {
            Ok(canonical) => return canonical,
            Err(e) => {
                let field = e.field.to_string();
                // "field cannot be empty" style errors: inject a
                // placeholder so the required-field check passes.
                if e.reason.contains("cannot be empty") {
                    attempt.insert(field, "(recovered)".into());
                } else {
                    // Remove the offending field and retry;
                    // validate_attrs will re-default from empty
                    // input where it can.
                    attempt.remove(&field);
                }
            }
        }
    }
    // Give up — return whatever the last attempt produced,
    // ignoring blockId which we keep untouched anyway.
    HashMap::new()
}


#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::OgreDoc;
    use yrs::types::xml::XmlElementPrelim;

    /// Build a doc with a single Kanban board containing one column
    /// and one card. Returns the doc and the state bytes.
    fn kanban_doc_with_card(
        card_title: &str,
        card_color: &str,
    ) -> (OgreDoc, Vec<u8>) {
        let doc = OgreDoc::new();
        {
            let mut txn = doc.inner().transact_mut();
            let frag = txn.get_or_insert_xml_fragment("content");
            // Replace the seeded paragraph with our kanban.
            let n = frag.len(&txn);
            if n > 0 {
                frag.remove_range(&mut txn, 0, n);
            }
            let board = frag.insert(
                &mut txn,
                0,
                XmlElementPrelim::empty(NodeType::Kanban.tag_name()),
            );
            let col = board.insert(
                &mut txn,
                0,
                XmlElementPrelim::empty(NodeType::KanbanColumn.tag_name()),
            );
            col.insert_attribute(&mut txn, "title", "To Do");
            let card = col.insert(
                &mut txn,
                0,
                XmlElementPrelim::empty(NodeType::KanbanCard.tag_name()),
            );
            card.insert_attribute(&mut txn, "title", card_title);
            card.insert_attribute(&mut txn, "color", card_color);
        }
        let bytes = doc.to_state_bytes();
        (doc, bytes)
    }

    #[test]
    fn valid_kanban_doc_passes() {
        let (doc, _) = kanban_doc_with_card("Fix login", "red");
        // Empty update should validate — no changes, no violations.
        let empty_update = {
            let sv = doc.state_vector();
            doc.encode_diff(&sv).unwrap()
        };
        assert!(validate_liveapp_writes(&doc, &empty_update).is_ok());
    }

    #[test]
    fn update_setting_invalid_color_is_rejected() {
        // Baseline doc has a valid card.
        let (doc, base_bytes) = kanban_doc_with_card("Fix", "red");
        // Build a peer doc, load baseline, then mutate the card's
        // color to something the validator rejects.
        let peer = OgreDoc::from_state_bytes(&base_bytes).unwrap();
        {
            let mut txn = peer.inner().transact_mut();
            let frag = txn.get_xml_fragment("content").unwrap();
            let XmlOut::Element(board) = frag.get(&txn, 0).unwrap() else {
                unreachable!()
            };
            let XmlOut::Element(col) = board.get(&txn, 0).unwrap() else {
                unreachable!()
            };
            let XmlOut::Element(card) = col.get(&txn, 0).unwrap() else {
                unreachable!()
            };
            card.insert_attribute(&mut txn, "color", "javascript:");
        }
        // Diff from baseline → the mutation update.
        let mutation = peer
            .encode_diff(&doc.state_vector())
            .unwrap();
        let result = validate_liveapp_writes(&doc, &mutation);
        let violations = result.expect_err("should reject invalid color");
        assert_eq!(violations.len(), 1);
        assert_eq!(violations[0].node_type, NodeType::KanbanCard);
        assert_eq!(violations[0].field, "color");
    }

    #[test]
    fn update_setting_malformed_due_at_is_rejected() {
        let (doc, base_bytes) = kanban_doc_with_card("Fix", "red");
        let peer = OgreDoc::from_state_bytes(&base_bytes).unwrap();
        {
            let mut txn = peer.inner().transact_mut();
            let frag = txn.get_xml_fragment("content").unwrap();
            let XmlOut::Element(board) = frag.get(&txn, 0).unwrap() else {
                unreachable!()
            };
            let XmlOut::Element(col) = board.get(&txn, 0).unwrap() else {
                unreachable!()
            };
            let XmlOut::Element(card) = col.get(&txn, 0).unwrap() else {
                unreachable!()
            };
            card.insert_attribute(&mut txn, "dueAt", "not-a-date");
        }
        let mutation = peer.encode_diff(&doc.state_vector()).unwrap();
        let violations = validate_liveapp_writes(&doc, &mutation)
            .expect_err("should reject bad dueAt");
        assert_eq!(violations[0].field, "dueAt");
    }

    #[test]
    fn non_liveapp_nodes_are_skipped() {
        // A doc with only a paragraph — no LiveApp nodes anywhere,
        // no way for validate to fire.
        let doc = OgreDoc::new();
        {
            let mut txn = doc.inner().transact_mut();
            let frag = txn.get_or_insert_xml_fragment("content");
            let XmlOut::Element(para) = frag.get(&txn, 0).unwrap() else {
                unreachable!()
            };
            let text = para.insert(
                &mut txn,
                0,
                yrs::types::xml::XmlTextPrelim::new(""),
            );
            let _ = text;
        }
        // Empty update on a paragraph-only doc validates.
        let empty_update = {
            let sv = doc.state_vector();
            doc.encode_diff(&sv).unwrap()
        };
        assert!(validate_liveapp_writes(&doc, &empty_update).is_ok());
    }

    #[test]
    fn decode_failure_returns_ok() {
        let (doc, _) = kanban_doc_with_card("Fix", "red");
        // Random bytes that aren't a valid yrs update.
        let junk = [0xFFu8, 0xFE, 0xFD, 0xFC];
        assert!(validate_liveapp_writes(&doc, &junk).is_ok());
    }

    /// #1 from the Phase 2a review — the gate only checked
    /// Ok/Err, so an oversized `title` (which the validator
    /// silently clamped) passed through untouched. This test
    /// pins the fix: divergence between input and canonical
    /// must surface as a violation.
    #[test]
    fn oversized_title_surfaces_as_violation() {
        use crate::blocks::kanban::MAX_CARD_TITLE_LEN;
        let (doc, base_bytes) = kanban_doc_with_card("Fix", "red");
        let peer = OgreDoc::from_state_bytes(&base_bytes).unwrap();
        let too_long = "x".repeat(MAX_CARD_TITLE_LEN + 100);
        {
            let mut txn = peer.inner().transact_mut();
            let frag = txn.get_xml_fragment("content").unwrap();
            let XmlOut::Element(board) = frag.get(&txn, 0).unwrap() else { unreachable!() };
            let XmlOut::Element(col) = board.get(&txn, 0).unwrap() else { unreachable!() };
            let XmlOut::Element(card) = col.get(&txn, 0).unwrap() else { unreachable!() };
            card.insert_attribute(&mut txn, "title", too_long.as_str());
        }
        let mutation = peer.encode_diff(&doc.state_vector()).unwrap();
        let violations = validate_liveapp_writes(&doc, &mutation)
            .expect_err("clamped title must surface");
        assert!(
            violations.iter().any(|v| v.field == "title"),
            "expected a title violation, got {violations:?}"
        );
    }

    /// Empty-string attrs are treated as "no assertion made" by
    /// the block validators (they default in the canonical form).
    /// Strict comparison must NOT flag those — otherwise every
    /// fresh insert of a card with no due date / no labels /
    /// no assignee would trip the gate.
    #[test]
    fn empty_attrs_are_not_violations() {
        let (doc, base_bytes) = kanban_doc_with_card("Fix", "red");
        let peer = OgreDoc::from_state_bytes(&base_bytes).unwrap();
        {
            let mut txn = peer.inner().transact_mut();
            let frag = txn.get_xml_fragment("content").unwrap();
            let XmlOut::Element(board) = frag.get(&txn, 0).unwrap() else { unreachable!() };
            let XmlOut::Element(col) = board.get(&txn, 0).unwrap() else { unreachable!() };
            let XmlOut::Element(card) = col.get(&txn, 0).unwrap() else { unreachable!() };
            // Set optional attrs to empty — legitimate "clear the field".
            card.insert_attribute(&mut txn, "dueAt", "");
            card.insert_attribute(&mut txn, "labels", "");
            card.insert_attribute(&mut txn, "assigneeId", "");
        }
        let mutation = peer.encode_diff(&doc.state_vector()).unwrap();
        assert!(validate_liveapp_writes(&doc, &mutation).is_ok());
    }

    /// Unknown attributes (attrs the block validator doesn't
    /// recognize) surface as violations — the strict compare
    /// treats a dropped key as divergence.
    #[test]
    fn unknown_attribute_surfaces_as_violation() {
        let (doc, base_bytes) = kanban_doc_with_card("Fix", "red");
        let peer = OgreDoc::from_state_bytes(&base_bytes).unwrap();
        {
            let mut txn = peer.inner().transact_mut();
            let frag = txn.get_xml_fragment("content").unwrap();
            let XmlOut::Element(board) = frag.get(&txn, 0).unwrap() else { unreachable!() };
            let XmlOut::Element(col) = board.get(&txn, 0).unwrap() else { unreachable!() };
            let XmlOut::Element(card) = col.get(&txn, 0).unwrap() else { unreachable!() };
            card.insert_attribute(&mut txn, "smuggledPayload", "arbitrary-content");
        }
        let mutation = peer.encode_diff(&doc.state_vector()).unwrap();
        let violations = validate_liveapp_writes(&doc, &mutation)
            .expect_err("unknown attribute must surface");
        assert!(violations.iter().any(|v| v.field == "smuggledPayload"));
    }

    // ── Phase 3 — walk_changed / WalkScope ────────────────────────

    #[test]
    fn walk_scope_env_parse() {
        assert_eq!(WalkScope::from_env_value(Some("full")), WalkScope::Full);
        assert_eq!(WalkScope::from_env_value(Some("FULL")), WalkScope::Full);
        assert_eq!(WalkScope::from_env_value(Some("changed")), WalkScope::Changed);
        assert_eq!(WalkScope::from_env_value(Some("canary")), WalkScope::Canary);
        assert_eq!(WalkScope::from_env_value(Some("garbage")), WalkScope::Full);
        assert_eq!(WalkScope::from_env_value(None), WalkScope::Full);
    }

    /// The load-bearing gap-001 regression: with the whole-doc
    /// walk, an existing invalid attribute anywhere in the doc
    /// blocks every subsequent WS write, no matter what the write
    /// actually touched. `WalkScope::Changed` fixes that by only
    /// validating touched elements — an unrelated paragraph edit
    /// must pass even when the doc has a legacy-invalid card.
    #[test]
    fn changed_scope_ignores_untouched_invalid_card() {
        use yrs::types::xml::XmlTextPrelim;

        // Baseline: a doc with a KanbanCard whose color is already
        // corrupt from some pre-hardening write path.
        let (doc, _) = kanban_doc_with_card("Fix", "red");
        {
            let mut txn = doc.inner().transact_mut();
            let frag = txn.get_xml_fragment("content").unwrap();
            let XmlOut::Element(board) = frag.get(&txn, 0).unwrap() else { unreachable!() };
            let XmlOut::Element(col) = board.get(&txn, 0).unwrap() else { unreachable!() };
            let XmlOut::Element(card) = col.get(&txn, 0).unwrap() else { unreachable!() };
            card.insert_attribute(&mut txn, "color", "chartreuse");
        }
        let base_bytes = doc.to_state_bytes();

        // A peer applies an unrelated edit: append a paragraph
        // after the Kanban board. Nothing in the peer's update
        // touches the corrupt card.
        let peer = OgreDoc::from_state_bytes(&base_bytes).unwrap();
        {
            let mut txn = peer.inner().transact_mut();
            let frag = txn.get_xml_fragment("content").unwrap();
            let n = frag.len(&txn);
            let para = frag.insert(
                &mut txn,
                n,
                XmlElementPrelim::empty("paragraph"),
            );
            let text = para.insert(&mut txn, 0, XmlTextPrelim::new(""));
            let _ = text;
        }
        let mutation = peer.encode_diff(&doc.state_vector()).unwrap();

        // Full scope: legacy invalid attr surfaces → rejected.
        let full = validate_liveapp_writes_scoped(&doc, &mutation, WalkScope::Full);
        assert!(
            full.is_err(),
            "Full-mode walk should still see the pre-existing corrupt card"
        );

        // Changed scope: only the new paragraph is touched → clean.
        let changed = validate_liveapp_writes_scoped(&doc, &mutation, WalkScope::Changed);
        assert!(
            changed.is_ok(),
            "Changed-mode walk must not see the untouched corrupt card, got {changed:?}"
        );
    }

    /// The other side of the coin: `Changed` mode must still
    /// reject when the write itself lands an invalid attribute
    /// (this is exactly what Reject mode is protecting against).
    #[test]
    fn changed_scope_rejects_new_invalid_attr() {
        let (doc, base_bytes) = kanban_doc_with_card("Fix", "red");
        let peer = OgreDoc::from_state_bytes(&base_bytes).unwrap();
        {
            let mut txn = peer.inner().transact_mut();
            let frag = txn.get_xml_fragment("content").unwrap();
            let XmlOut::Element(board) = frag.get(&txn, 0).unwrap() else { unreachable!() };
            let XmlOut::Element(col) = board.get(&txn, 0).unwrap() else { unreachable!() };
            let XmlOut::Element(card) = col.get(&txn, 0).unwrap() else { unreachable!() };
            card.insert_attribute(&mut txn, "color", "javascript:");
        }
        let mutation = peer.encode_diff(&doc.state_vector()).unwrap();
        let violations = validate_liveapp_writes_scoped(&doc, &mutation, WalkScope::Changed)
            .expect_err("Changed mode must reject a write that lands an invalid attr");
        assert!(violations.iter().any(|v| v.node_type == NodeType::KanbanCard));
    }

    /// A newly-inserted invalid card is caught: the deep observer
    /// fires for the parent (children changed) AND for the card
    /// itself (attribute writes), so walk_changed sees the new
    /// card and validates it.
    #[test]
    fn changed_scope_catches_newly_inserted_invalid_card() {
        let (doc, base_bytes) = kanban_doc_with_card("Existing", "red");
        let peer = OgreDoc::from_state_bytes(&base_bytes).unwrap();
        {
            let mut txn = peer.inner().transact_mut();
            let frag = txn.get_xml_fragment("content").unwrap();
            let XmlOut::Element(board) = frag.get(&txn, 0).unwrap() else { unreachable!() };
            let XmlOut::Element(col) = board.get(&txn, 0).unwrap() else { unreachable!() };
            // Insert a new card at position 0 (before the
            // existing valid one).
            let new_card = col.insert(&mut txn, 0, XmlElementPrelim::empty("kanban_card"));
            new_card.insert_attribute(&mut txn, "title", "New");
            new_card.insert_attribute(&mut txn, "color", "not-a-color");
        }
        let mutation = peer.encode_diff(&doc.state_vector()).unwrap();
        let violations = validate_liveapp_writes_scoped(&doc, &mutation, WalkScope::Changed)
            .expect_err("Changed mode must catch newly-inserted invalid card");
        assert!(
            violations.iter().any(|v| v.field == "color"),
            "expected color violation, got {violations:?}"
        );
    }

    /// Canary mode: when full and changed disagree, we fall back
    /// to full's answer. This test builds a scenario where the
    /// TWO walks WOULD disagree (invalid untouched card) and
    /// asserts canary picks full.
    #[test]
    fn canary_scope_falls_back_to_full_on_disagreement() {
        use yrs::types::xml::XmlTextPrelim;

        let (doc, _) = kanban_doc_with_card("Fix", "red");
        {
            let mut txn = doc.inner().transact_mut();
            let frag = txn.get_xml_fragment("content").unwrap();
            let XmlOut::Element(board) = frag.get(&txn, 0).unwrap() else { unreachable!() };
            let XmlOut::Element(col) = board.get(&txn, 0).unwrap() else { unreachable!() };
            let XmlOut::Element(card) = col.get(&txn, 0).unwrap() else { unreachable!() };
            card.insert_attribute(&mut txn, "color", "chartreuse");
        }
        let base_bytes = doc.to_state_bytes();

        let peer = OgreDoc::from_state_bytes(&base_bytes).unwrap();
        {
            let mut txn = peer.inner().transact_mut();
            let frag = txn.get_xml_fragment("content").unwrap();
            let n = frag.len(&txn);
            let para = frag.insert(&mut txn, n, XmlElementPrelim::empty("paragraph"));
            let _ = para.insert(&mut txn, 0, XmlTextPrelim::new(""));
        }
        let mutation = peer.encode_diff(&doc.state_vector()).unwrap();
        let canary = validate_liveapp_writes_scoped(&doc, &mutation, WalkScope::Canary);
        assert!(
            canary.is_err(),
            "Canary must return Full's (rejecting) answer to stay safe during rollout"
        );
    }

    // ── repair_liveapp_attrs ─────────────────────────────

    /// After repair, the doc's LiveApp nodes must all pass
    /// validate_attrs cleanly. Verifies both the "invalid color
    /// gets replaced with default" path and the "canonical form
    /// diverges from stored" path (label canonicalization).
    #[test]
    fn repair_liveapp_attrs_canonicalizes_and_report_counts() {
        let (doc, _) = kanban_doc_with_card("Fix", "red");
        {
            let mut txn = doc.inner().transact_mut();
            let frag = txn.get_xml_fragment("content").unwrap();
            let XmlOut::Element(board) = frag.get(&txn, 0).unwrap() else { unreachable!() };
            let XmlOut::Element(col) = board.get(&txn, 0).unwrap() else { unreachable!() };
            let XmlOut::Element(card) = col.get(&txn, 0).unwrap() else { unreachable!() };
            // Structurally-invalid color.
            card.insert_attribute(&mut txn, "color", "chartreuse");
            // Labels with an empty segment — canonical form
            // drops the empty and keeps just the valid entries.
            card.insert_attribute(&mut txn, "labels", "bug|red;;ux|blue;");
        }

        let report = repair_liveapp_attrs(doc.inner());

        assert!(
            report.nodes_touched >= 1,
            "at least the card must be reported as touched, got {report:?}"
        );

        // Post-repair, walk_doc must not surface any violations.
        let post = walk_doc(doc.inner());
        assert!(
            post.is_empty(),
            "post-repair walk must return no violations, got {post:?}"
        );
    }

    // ── gap-003: LiveApp deletion detection ─────────────

    #[test]
    fn diff_liveapp_deletions_reports_removed_pair() {
        let pre = vec![
            (NodeType::KanbanCard, "card-a".to_string()),
            (NodeType::KanbanCard, "card-b".to_string()),
        ];
        let post = vec![(NodeType::KanbanCard, "card-a".to_string())];
        let deletions = diff_liveapp_deletions(&pre, &post);
        assert_eq!(deletions.len(), 1);
        assert_eq!(deletions[0].node_type, NodeType::KanbanCard);
        assert_eq!(deletions[0].block_id, "card-b");
    }

    #[test]
    fn diff_liveapp_deletions_empty_on_noop() {
        let pre = vec![(NodeType::KanbanCard, "card-a".to_string())];
        let post = pre.clone();
        assert!(diff_liveapp_deletions(&pre, &post).is_empty());
    }

    #[test]
    fn diff_liveapp_deletions_handles_duplicate_block_ids() {
        // Two cards share the same block_id (edge case — legacy
        // data or bugged import). Removing one should surface
        // exactly one deletion.
        let pre = vec![
            (NodeType::KanbanCard, "".to_string()),
            (NodeType::KanbanCard, "".to_string()),
            (NodeType::KanbanCard, "".to_string()),
        ];
        let post = vec![
            (NodeType::KanbanCard, "".to_string()),
            (NodeType::KanbanCard, "".to_string()),
        ];
        assert_eq!(diff_liveapp_deletions(&pre, &post).len(), 1);
    }

    #[test]
    fn diff_liveapp_deletions_distinguishes_node_types() {
        let pre = vec![
            (NodeType::KanbanCard, "shared-id".to_string()),
            (NodeType::CalendarEvent, "shared-id".to_string()),
        ];
        let post = vec![(NodeType::KanbanCard, "shared-id".to_string())];
        let deletions = diff_liveapp_deletions(&pre, &post);
        assert_eq!(deletions.len(), 1);
        assert_eq!(deletions[0].node_type, NodeType::CalendarEvent);
    }

    #[test]
    fn collect_liveapp_index_walks_nested_kanban() {
        let (doc, _) = kanban_doc_with_card("Fix", "red");
        let idx = collect_liveapp_index(doc.inner());
        // Kanban board + KanbanColumn + KanbanCard = 3.
        assert_eq!(idx.len(), 3);
        assert!(idx.iter().any(|(nt, _)| *nt == NodeType::KanbanCard));
        assert!(idx.iter().any(|(nt, _)| *nt == NodeType::KanbanColumn));
        assert!(idx.iter().any(|(nt, _)| *nt == NodeType::Kanban));
    }

    #[test]
    fn repair_liveapp_attrs_is_a_noop_on_valid_doc() {
        let (doc, _) = kanban_doc_with_card("Fix", "red");
        let report = repair_liveapp_attrs(doc.inner());
        assert_eq!(
            report.nodes_touched, 0,
            "clean doc must not report any touched nodes"
        );
    }

    /// Sanity: on a valid doc with a valid write, both walks
    /// return the empty violation set and canary passes.
    #[test]
    fn canary_scope_passes_when_walks_agree() {
        let (doc, base_bytes) = kanban_doc_with_card("Fix", "red");
        let peer = OgreDoc::from_state_bytes(&base_bytes).unwrap();
        {
            let mut txn = peer.inner().transact_mut();
            let frag = txn.get_xml_fragment("content").unwrap();
            let XmlOut::Element(board) = frag.get(&txn, 0).unwrap() else { unreachable!() };
            let XmlOut::Element(col) = board.get(&txn, 0).unwrap() else { unreachable!() };
            col.insert_attribute(&mut txn, "title", "Doing");
        }
        let mutation = peer.encode_diff(&doc.state_vector()).unwrap();
        let full = validate_liveapp_writes_scoped(&doc, &mutation, WalkScope::Full);
        let changed = validate_liveapp_writes_scoped(&doc, &mutation, WalkScope::Changed);
        let canary = validate_liveapp_writes_scoped(&doc, &mutation, WalkScope::Canary);
        assert!(full.is_ok() && changed.is_ok() && canary.is_ok());
    }
}

#[cfg(test)]
mod proptests {
    //! Canary-equivalence proptest.
    //!
    //! For a doc whose baseline is guaranteed VALID (constructed from
    //! whitelist-only building blocks), `walk_doc` and `walk_changed`
    //! must return the same violation set for any single update. The
    //! proptest generates random sequences of card writes — some
    //! valid, some deliberately invalid — and asserts equivalence.
    //!
    //! We intentionally DO NOT proptest with invalid baselines: the
    //! whole point of the changed-refs walk is that it ignores
    //! untouched invalid attrs (the gap-001 fix). Under an invalid
    //! baseline the walks are supposed to differ, so a proptest
    //! there would be testing the wrong thing.

    use super::*;
    use crate::schema::NodeType;
    use proptest::prelude::*;
    use yrs::types::xml::XmlElementPrelim;
    use yrs::{Transact, WriteTxn};

    #[derive(Debug, Clone)]
    enum CardOp {
        /// Set the card's color to a legitimate palette value.
        SetValidColor(&'static str),
        /// Set the card's color to a value that fails the whitelist.
        SetInvalidColor(String),
        /// Set the title to a value within the length cap.
        SetShortTitle(String),
        /// Set the title to a deliberately-oversized value —
        /// strict-compare surfaces this as a violation.
        SetOversizedTitle,
        /// Set a valid dueAt.
        SetValidDueAt,
        /// Set a malformed dueAt.
        SetInvalidDueAt,
        /// Insert a smuggled unknown attribute.
        SmuggleUnknownAttr,
    }

    fn card_op_strategy() -> impl Strategy<Value = CardOp> {
        prop_oneof![
            Just(CardOp::SetValidColor("red")),
            Just(CardOp::SetValidColor("blue")),
            Just(CardOp::SetValidColor("green")),
            "[a-z]{2,10}".prop_map(CardOp::SetInvalidColor),
            "[a-zA-Z0-9 ]{1,30}".prop_map(CardOp::SetShortTitle),
            Just(CardOp::SetOversizedTitle),
            Just(CardOp::SetValidDueAt),
            Just(CardOp::SetInvalidDueAt),
            Just(CardOp::SmuggleUnknownAttr),
        ]
    }

    fn apply_card_ops(doc: &OgreDoc, ops: &[CardOp]) {
        use yrs::types::xml::XmlOut;
        let mut txn = doc.inner().transact_mut();
        let frag = txn.get_xml_fragment("content").unwrap();
        let XmlOut::Element(board) = frag.get(&txn, 0).unwrap() else {
            return;
        };
        let XmlOut::Element(col) = board.get(&txn, 0).unwrap() else {
            return;
        };
        let XmlOut::Element(card) = col.get(&txn, 0).unwrap() else {
            return;
        };
        for op in ops {
            match op {
                CardOp::SetValidColor(c) => card.insert_attribute(&mut txn, "color", *c),
                CardOp::SetInvalidColor(c) => {
                    card.insert_attribute(&mut txn, "color", c.as_str())
                }
                CardOp::SetShortTitle(t) => card.insert_attribute(&mut txn, "title", t.as_str()),
                CardOp::SetOversizedTitle => {
                    let long = "z".repeat(500);
                    card.insert_attribute(&mut txn, "title", long.as_str());
                }
                CardOp::SetValidDueAt => card.insert_attribute(&mut txn, "dueAt", "2026-07-05"),
                CardOp::SetInvalidDueAt => card.insert_attribute(&mut txn, "dueAt", "yesterday"),
                CardOp::SmuggleUnknownAttr => {
                    card.insert_attribute(&mut txn, "smuggle", "value")
                }
            }
        }
    }

    fn valid_baseline() -> (OgreDoc, Vec<u8>) {
        let doc = OgreDoc::new();
        {
            let mut txn = doc.inner().transact_mut();
            let frag = txn.get_or_insert_xml_fragment("content");
            let n = frag.len(&txn);
            if n > 0 {
                frag.remove_range(&mut txn, 0, n);
            }
            let board = frag.insert(
                &mut txn,
                0,
                XmlElementPrelim::empty(NodeType::Kanban.tag_name()),
            );
            let col = board.insert(
                &mut txn,
                0,
                XmlElementPrelim::empty(NodeType::KanbanColumn.tag_name()),
            );
            col.insert_attribute(&mut txn, "title", "To Do");
            let card = col.insert(
                &mut txn,
                0,
                XmlElementPrelim::empty(NodeType::KanbanCard.tag_name()),
            );
            card.insert_attribute(&mut txn, "title", "Base");
            card.insert_attribute(&mut txn, "color", "red");
        }
        let bytes = doc.to_state_bytes();
        (doc, bytes)
    }

    proptest! {
        #![proptest_config(ProptestConfig {
            cases: 128,
            .. ProptestConfig::default()
        })]

        /// For any single-card update on a valid baseline,
        /// `walk_doc` and `walk_changed` return the same violation
        /// set (modulo ordering). This is the equivalence-under-
        /// valid-baseline invariant that lets `Canary` graduate to
        /// the new default.
        #[test]
        fn walk_doc_and_walk_changed_agree_on_valid_baseline(
            ops in proptest::collection::vec(card_op_strategy(), 1..8)
        ) {
            let (doc, base_bytes) = valid_baseline();
            let peer = OgreDoc::from_state_bytes(&base_bytes).unwrap();
            apply_card_ops(&peer, &ops);
            let mutation = peer.encode_diff(&doc.state_vector()).unwrap();

            let full = validate_liveapp_writes_scoped(&doc, &mutation, WalkScope::Full);
            let changed = validate_liveapp_writes_scoped(&doc, &mutation, WalkScope::Changed);

            let a: Vec<LiveAppViolation> = full.err().unwrap_or_default();
            let b: Vec<LiveAppViolation> = changed.err().unwrap_or_default();
            prop_assert!(
                super::violation_sets_equivalent(&a, &b),
                "walks disagreed: full={a:?}, changed={b:?}"
            );
        }
    }
}
