# MEDULLA-CONTRACT — Public engine contract and release

Component: Public contract and release

## Outcome

A contributor changes an engine method once and gets the same typed, signed contract for Rust, Python, Go and JavaScript. An application discovers callable methods, scopes and stable errors from the engine at run time, without importing private server internals. A release ships one `medulla` distribution per platform, with the server executable inside, and proves its artifacts from one source revision.

## Scope

Owner: `medulla` (native Rust, Python client, Go and JavaScript clients, release workflows). This spec governs the published method and error projection, the client models and codec, the wheel and image composition, the hosted gates and the crate layering rules. Adjacent capabilities define their own semantics in their own specs.

Non-goals: no second method registry, error classifier, Python model hierarchy or signing implementation. External applications check authorization in their own service layer. `is_wire_callable` describes a capability, not a grant.

Why a new spec: no other spec owns the published contract, the client codec, the release composition and the build-layer rules. The owner map was searched with `git grep -n "wire_callable\|method_catalog\|release-server" -- specs` and found no other owner.

## User Scenarios & Testing

### User Story 1 - Call any served method through strict typed models (Priority: P1)

The user is a connector or application developer. The outcome is a typed call that fails before sending when its payload is wrong.

**Acceptance scenarios**

1. **Given** an installed `medulla` client and a served engine, **When** a payload carries one unknown nested field, **Then** the model raises `ValidationError` before any request is sent.
2. **Given** a valid payload, **When** the client sends it, **Then** the typed result matches the Rust round-trip vector for that method.
3. **Given** a read call on a dropped connection, **When** the client retries once, **Then** the call succeeds, and a write call never retries.

Independent test: `tests/test_decide_auth_boundary.py::test_decide_refuses_a_request_tenant_that_does_not_match_the_verified_session` runs this story through a served engine.

### User Story 2 - Sign request bodies the same way in every language (Priority: P1)

The user is a Go or JavaScript connector developer. The outcome is a signed body that equals the Rust body byte for byte.

**Acceptance scenarios**

1. **Given** the committed body vectors, **When** the Go and JS clients replay them, **Then** each signed body equals the Rust output.
2. **Given** a source or toolchain change, **When** the codec check rebuilds the WASM twice, **Then** both builds are identical and within the size budget.

Independent test: `tests/test_build_method_codec_wasm.py::test_ci_builds_the_codec_before_client_tests_and_uploads_it` covers the export path; the vector tests cover the replay.

### User Story 3 - Install one distribution that carries the server (Priority: P1)

The user is an operator. The outcome is `pip install medulla` giving the engine modules and the server executable for the platform.

**Acceptance scenarios**

1. **Given** a platform wheel, **When** the completeness gate runs with `--require-server-binary`, **Then** it finds exactly one non-empty, executable server member and no console script.
2. **Given** the installed wheel, **When** the smoke test runs `medulla-server --version`, **Then** the printed version equals the wheel version.
3. **Given** a tag push, **When** `guard` finds no successful main CI run on the commit, **Then** the release stops.

Independent test: `tests/test_server_package.py::test_one_distribution_no_server_extra_and_no_retired_name`.

### User Story 4 - Keep the public gates honest (Priority: P2)

The user is a maintainer. The outcome is that hosted gates run on exact trees with no bypass, baseline or suppression.

**Acceptance scenarios**

1. **Given** a pull request, **When** CI runs, **Then** one required job checks the changed files and the affected crates.
2. **Given** a planted private path or credential literal, **When** the privacy scan runs, **Then** it fails; a clean tree reports zero findings.
3. **Given** a stale dispatch target, **When** the decomposition gate runs, **Then** it fails closed.

Independent test: `tests/test_documentation_privacy.py::test_privacy_self_check_catches_a_planted_violation`.

### User Story 5 - Keep the crate layers and the architecture map true (Priority: P2)

The user is a contributor. The outcome is that a change rebuilds only its own crates and the map matches the source.

**Acceptance scenarios**

1. **Given** the workspace manifests, **When** the layering check runs, **Then** no crate depends upward.
2. **Given** the architecture map in `plan.md`, **When** the map test runs, **Then** every workspace package has an owner group.

Independent test: `tests/test_server_split_rollup.py::test_backbone_crates_exist_and_depend_only_downward`.

## Edge Cases

- A method id with no descriptor row or a descriptor with no method fails the registry test in both directions.
- A code newer than the installed client maps to the unknown-code variant, not to a crash.
- A failed canonical build never exports cached bytes.
- A wheel that carries a console script or two server members is refused.
- A pull request that touches only specs runs no cargo step.

## Assumptions

- The Rust serde types remain the only source of the wire shapes.
- The PyPI per-file limit of the `medulla` project is 300 MiB, and the combined wheel stays below it.
- Consumers read the served catalog; they do not read wheel files.
- A release fix that changes the per-platform server build updates `MEDULLA-CONTRACT-R053.9` in the same change.

## Requirements

### Functional requirements

- FR-001 Rust declarations are the only authority for method identity, policy, stability and `error_set`; the engine serves them (`MEDULLA-CONTRACT-R054`, `MEDULLA-CONTRACT-R057`).
- FR-002 Python models are strict, one per wire shape, pinned by Rust vectors (`MEDULLA-CONTRACT-R001.3`, `MEDULLA-CONTRACT-R001.4`, `MEDULLA-CONTRACT-R005`).
- FR-003 Go and JavaScript sign through the shared codec (`MEDULLA-CONTRACT-R004`, `MEDULLA-CONTRACT-R006.2`).
- FR-004 One `medulla` distribution per platform carries the server (`MEDULLA-CONTRACT-R053.9`, `MEDULLA-CONTRACT-R053.3`).
- FR-005 A tag publishes only a commit that main CI passed (`MEDULLA-CONTRACT-R053.7`).
- FR-006 Hosted gates run on exact trees without baseline or suppression (`MEDULLA-CONTRACT-R022`, `MEDULLA-CONTRACT-R039`).
- FR-007 The installed client has a served Agent Library and Decide journey (`MEDULLA-CONTRACT-R002.2`, `MEDULLA-CONTRACT-R002.3`).
- FR-008 Crate layers are downward-only and the architecture map matches the source (`MEDULLA-CONTRACT-R056.2`, `MEDULLA-CONTRACT-R052.1`).

### Success criteria

- SC-001 The Go and JS clients produce byte-identical signed bodies for every committed vector (`MEDULLA-CONTRACT-R004`).
- SC-002 Two canonical codec builds are identical and at most `MAX_BYTES` (`MEDULLA-CONTRACT-R006`).
- SC-003 Every platform wheel holds exactly one executable server member (`MEDULLA-CONTRACT-R053.9`).
- SC-004 The privacy and secret-history scans report zero findings on the tracked tree (`MEDULLA-CONTRACT-R040`, `MEDULLA-CONTRACT-R043`).
- SC-005 The workspace builds with no dependency cycle and no crate-level lint allow (`MEDULLA-CONTRACT-R056.17`).

## Architecture and ownership

The build graph is: Rust declarations, then the served catalog, then the codec WASM, then the Go and JS embedding with vector tests, then the platform wheels with the server folded in, then the clean-install consumer tests, then the release gate. Each arrow has a deterministic check. The architecture map in [plan.md](plan.md#current-architecture-boundaries) is owned by `MEDULLA-CONTRACT-R052`. It changes no feature, package recipe or release version.

Requirement IDs are in [requirements.md](requirements.md). Delivery state per ID is in `status.json`.
