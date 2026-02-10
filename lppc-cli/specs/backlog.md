# Backlog

This file tracks planned improvements, migrations, and technical debt for future consideration.

---

## Planned Migrations

### Migrate from `saphyr` to `saphyr-serde`

**Status**: Waiting for release
**Priority**: Medium
**Tracking**: Monitor [saphyr-rs/saphyr](https://github.com/saphyr-rs/saphyr) for `saphyr-serde` release

**Current State**:
We use the `saphyr` crate directly for YAML parsing, which requires manual deserialization of YAML documents into our Rust types.

**Target State**:
Switch to `saphyr-serde` once it's released, enabling automatic serde-based deserialization.

**Why this makes sense**:
1. **Same maintainer team**: `saphyr-serde` is developed by the saphyr-rs organization (50+ contributors), ensuring consistency and long-term support
2. **Reduced boilerplate**: Serde derive macros eliminate manual YAML-to-struct mapping code
3. **Type safety**: Compile-time guarantees for deserialization
4. **Ecosystem alignment**: Most Rust projects use serde for serialization; `saphyr-serde` integrates naturally
5. **Active development**: The saphyr project is actively maintained with YAML 1.2 compliance

**Why we don't use alternatives**:
- `serde_yaml` (dtolnay): Deprecated and archived
- `serde_yml`: Also archived as of September 2025
- `serde-saphyr` (bourumir-wyngs): Single maintainer, independent project - higher risk for long-term maintenance

**Migration effort**: Low
Once `saphyr-serde` is released, the migration involves:
1. Replace `saphyr` dependency with `saphyr-serde`
2. Remove manual deserialization code in `src/mapping/loader.rs`
3. Add `#[derive(Deserialize)]` to `ActionMapping` and `OptionalActions` types
4. Update parsing calls from manual API to `saphyr_serde::from_str()`

---

## Future Enhancements

*Track future enhancement ideas here as they arise.*

---

## Technical Debt

*Track technical debt items here as they arise.*
