---
description: versionator/version-bump
---

# version-bump

Analyze changes and recommend semantic version bump.

Current version: {{version}}
Changes: {{changes}}

Decision rules:
- Breaking API changes → major
- New backward-compatible features → minor
- Bug fixes → patch
- Pre-1.0 applies different rules

Output: Recommended version + reasoning.

⚠️ Recommendation only. For actual release: invoke `release` prompt with `versionator release push` (NOT bare `versionator release` or manual `bump` + `release` + `git push` — prevents remote/local sync issues).
