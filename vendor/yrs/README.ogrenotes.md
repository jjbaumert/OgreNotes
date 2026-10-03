# Yrs 0.21.3 backport

Source: the unmodified crates.io `yrs` 0.21.3 package, except for the one-line
`Branch::insert_at` fix described below. `LICENSE` is the original MIT license
from upstream tag `v0.21.3`. Registry installation metadata is omitted.

Upstream fix: [ed78a0523ae9e7c6c8c8573ae4beebba0be8ac00](https://github.com/y-crdt/y-crdt/commit/ed78a0523ae9e7c6c8c8573ae4beebba0be8ac00),
first released in Yrs 0.27.3; [upstream issue #636](https://github.com/y-crdt/y-crdt/issues/636).

`src/branch.rs` anchors an XML insertion at index zero to `self.start` as its
right neighbor. Without this origin, an existing peer's block can precede the
new block according to client ID, even when the caller explicitly prepends.
The patch is identical to the upstream production change.

Both the backend workspace and the separate frontend workspace use this source
through `[patch.crates-io]`. Version, public API, and update encoding remain
0.21.3. The full upgrade to 0.27.3 changes unrelated APIs and is outside this
backport. Remove the vendor patch when both workspaces upgrade to a release
containing this fix. Application collaboration tests cover both client-ID
orderings and persisted reloads.
