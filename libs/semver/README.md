# semver

SemVer version parsing (pre-release and build metadata included),
precedence ordering, and caret/exact requirement matching — the same
rules `nestpkg.nvpm` uses, available to your own code.

`docs/overview.md` is the reference: the two orders (`core_cmp` versus
`version_cmp`), where this package is deliberately ahead of the
manifest grammar, and what "market ready" still needs.

Run the tests:

    noct test

Or directly:

    noct run tests/semver_test.nv
