---
name: New wrong-dir / builtin skill
about: Propose a new build tool PRECC should auto-correct (great first contribution)
title: "skill: <tool>-wrong-dir"
labels: ["good first issue", "skill"]
---

**Tool / command** (e.g. `bazel`, `mvn`, `pnpm`):

**Project marker file** that identifies its root (e.g. `WORKSPACE`, `pom.xml`):

**The mistake it should fix**
<!-- e.g. running `bazel build` from a subdir instead of the WORKSPACE root -->

**Trigger regex** (first word, e.g. `^bazel\s`):

**Correction** (usually `prepend_cd` → `cd {{project_root}} && {{original_command}}`):

---
See `CONTRIBUTING.md` → "Great first contribution" for the 2-step recipe. Happy
to mentor — say so and we'll guide the PR.
