---
paths:
  - "**/Cargo.toml"
---

# Dependency Check

Before adding any Rust dependency:
1. Check crates.io for the last publish date (the more recent the better)
2. Verify that it is not deprecated (check README, docs or github/gitlab page for archival status)
3. Check for number of contributors (github or gitlab) if available. Single person project have less relevance than projects with many human contributors (excluding automation bots and AI agents)
4. Check for security advisories via `cargo audit`
5. Prefer crates with many downloads (example: >1000 downloads/day)
