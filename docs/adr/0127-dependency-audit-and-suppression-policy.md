# ADR 0127 — Dependency Audit and Advisory Suppression Policy

**Date:** 2026-04-14  
**Status:** Accepted

---

## Context

`cargo audit` (via `make check`) scans `Cargo.lock` for RUSTSEC advisories.
As of 2026-04-14, four advisories were active. One was resolved by a dependency
upgrade; three cannot be resolved from within this project and are suppressed
with documented rationale in `.cargo/audit.toml`.

---

## What Was Fixed

### `instant 0.1.13` — RUSTSEC-2024-0384 (unmaintained)

**Resolution:** upgraded `notify 7` → `notify 8` in `Cargo.toml`.

`notify 8` pulls in `notify-types 2.x`, which replaced `instant` with
`web-time`. The `RecommendedWatcher` / `RecursiveMode` / `EventKind` API
surface used by forgiven is identical across both versions — no source changes
were required.

---

## What Is Suppressed and Why

Suppressed advisories live in `.cargo/audit.toml` under `[advisories] ignore`.
Each entry must have a code comment explaining the block and naming the upstream
condition that would allow removal.

### `rand 0.8.5` — RUSTSEC-2026-0097 (unsound)

**Advisory:** `rand::rng()` is unsound when a custom global logger is active
during initialisation of the thread-local RNG.

**Dependency chain:**

```
ratatui 0.30
└── ratatui-termwiz 0.1.0   ← optional feature, NOT enabled
    └── termwiz 0.23.3
        └── phf 0.11.3
            └── phf_generator 0.11.3
                └── rand 0.8.5          ← advisory here
```

**Why it is safe to suppress:**

- `ratatui-termwiz` is an optional ratatui backend gated behind the `termwiz`
  feature flag. Forgiven uses the `crossterm` backend exclusively; the
  `termwiz` feature is never enabled.
- `cargo tree` (which shows the compiled dependency graph, not the full lock
  resolution) lists no `termwiz`, `phf`, or `rand 0.8` nodes — confirming
  the vulnerable code is **not compiled into the binary**.
- `rand 0.8.5` appears in `Cargo.lock` only because Cargo's lock file format
  v3 records optional dependencies even when they are not enabled.

**Upstream block:** `termwiz 0.23.3` (latest as of 2026-04-14) constrains
`phf` to `^0.11`. `phf_generator 0.13+` replaces `rand` entirely with
`fastrand`, which is not affected by this advisory. The suppression can be
removed once termwiz upgrades to `phf 0.12+`.

**Trigger for re-evaluation:** `termwiz 0.24+` in crates.io, or any release
of `ratatui-termwiz` that pulls in `termwiz` with `phf 0.12+`.

---

### `bincode 1.3.3` — RUSTSEC-2025-0141 (unmaintained)

**Dependency chain:**

```
syntect 5.3.0
└── bincode 1.3.3
```

**Why it is safe to suppress:** `bincode` is used by syntect to serialise
pre-compiled `.packdump` theme/syntax assets at build time and to load them
at runtime. The format is internal to syntect; no user data is deserialised
through bincode. The unmaintained status carries no known CVE.

**Upstream block:** syntect 5.3.0 is the latest release and has not migrated
to bincode 2. The API between bincode 1 and 2 is a breaking change requiring
significant syntect refactoring.

**Trigger for re-evaluation:** syntect 6.x release, or syntect switching to a
different serialisation mechanism.

---

### `yaml-rust 0.4.5` — RUSTSEC-2024-0320 (unmaintained)

**Dependency chain:**

```
syntect 5.3.0  (feature: yaml-load)
└── yaml-rust 0.4.5
```

**Why it is safe to suppress:** `yaml-rust` is used by syntect to parse
`.sublime-syntax` definition files at startup. These are read-only,
trusted, bundled assets — not user-controlled input. The unmaintained
status carries no known CVE.

**Upstream block:** syntect's `yaml-load` feature requires `yaml-rust`. A
maintained fork (`yaml-rust2`) exists but syntect has not yet migrated to it.

**Trigger for re-evaluation:** syntect 6.x release, or syntect switching to
`yaml-rust2` / another YAML parser.

---

## Suppression File

**Location:** `.cargo/audit.toml`

```toml
[advisories]
ignore = [
    "RUSTSEC-2026-0097",   # rand 0.8.5 — unused optional dep, see ADR 0127
    "RUSTSEC-2025-0141",   # bincode 1.3.3 — syntect, see ADR 0127
    "RUSTSEC-2024-0320",   # yaml-rust 0.4.5 — syntect, see ADR 0127
]
```

---

## Policy: Adding New Suppressions

1. **Never suppress a vulnerability that affects compiled code.** Verify with
   `cargo tree` before suppressing.
2. Every suppression entry in `.cargo/audit.toml` must have a comment naming
   this ADR and the upstream blocking condition.
3. Update this ADR (or open a new one) when a suppression is added or removed.
4. Review suppressed advisories on each major ratatui or syntect upgrade.

---

## Future Upgrade Checklist

When any of the following events occur, revisit the suppressions:

| Event | Advisory to re-check | Action |
|-------|---------------------|--------|
| `termwiz` 0.24+ released | RUSTSEC-2026-0097 | Check if phf upgraded to 0.12+; remove suppression if so |
| `ratatui` 0.31+ released | RUSTSEC-2026-0097 | Check `ratatui-termwiz` transitive deps |
| `syntect` 6.x released | RUSTSEC-2025-0141, RUSTSEC-2024-0320 | Remove both suppressions if syntect drops bincode 1 and yaml-rust |
| Any new `rand` 0.8.x advisory | RUSTSEC-2026-0097 | Re-verify that `ratatui-termwiz` is still not compiled |

---

## Consequences

**Positive**
- `cargo audit` (and therefore `make check`) passes cleanly.
- Each suppression is documented with an explicit upstream blocking condition
  and a removal trigger, preventing silent accumulation of ignored advisories.
- The `notify 8` upgrade resolves one advisory entirely with no code changes.

**Negative / trade-offs**
- Three advisories remain unresolved at source. The suppression file must be
  actively maintained as upstream dependencies evolve.
- If `ratatui`'s feature resolution ever changes to enable `termwiz` by
  default, RUSTSEC-2026-0097 would become a real issue. This risk is mitigated
  by the `cargo tree` check policy above.
