# MEDULLA-SDLC — Research

Governing plan: [plan.md](plan.md). Each entry resolves one Phase 0 unknown of the delivery rows (R019). The entries describe capabilities and techniques. A measurement on this platform replaces each starting value.

## 1. Which events reach a delivery

**Decision.** A delivery follows the proposals it opened. Every event on a followed proposal becomes an input of the delivery. Check and automated-reviewer events never resume an ended delivery. A person's event resumes an ended delivery after mandate revalidation.

**Technique.** Separate routing from authorization. A subscription of the delivery to its proposal routes events. The immutable mandate holds the authority. A follow-up therefore needs no new admission decision, and it grants no new scope. Deduplicate by the provider event ID, because code hosts redeliver events.

**Rejected.** A new admission decision for each follow-up. It costs a model call per comment and adds no safety, because the mandate already bounds the scope.

## 2. Which facts allow merge arming

**Decision.** The verdict is `arm` only with five facts: the delivery opened the proposal, every required gate passed on the head, the required check passed on the head, an owner approval names the head commit, and the change size stays under a ceiling. A moved head or a change request gives `disarm`. The platform principal never merges.

**Technique.** Bind the approval to the head commit digest, not to the proposal. A later push then voids the approval without a second rule. Ask the code host to arm with an expected head, so a race between push and arming fails closed. Withdraw arming before each push.

**Starting value.** The change-size ceiling starts at 400 changed lines. The value covers a typical row with its test.

## 3. How many automated review rounds to address

**Decision.** A delivery addresses at most three rounds per automated reviewer and proposal. A round is one distinct review run, keyed by its run identifier, not one comment.

**Technique.** Treat automated review as fallible input. Verify each finding against the code before a change. Resolve a fixed finding's thread without a reply. Answer a declined finding once with a reason. Count rounds across assignments and restarts, so a new run does not reset the budget.

**Starting value.** Three rounds. Person reviews have no limit.

## 4. How to recover after a restart

**Decision.** An assignment whose lease expired ends as `host_restarted`. A new assignment starts with a recovery note. The note lists each recorded outside effect and its receipt state.

**Technique.** Record each outside effect before it runs and record its receipt after it runs. An effect without a receipt is uncertain. Reconcile it against the code host before any retry. Move unread inputs to the new assignment exactly once.

## 5. How an owner gains trust before unprompted delivery

**Decision.** In mode `development` the loop records `would_dispatch` for an unprompted slice and starts nothing. The owner reads these records and then approves promotion to `evolving`.

**Technique.** Run the decision in shadow before it acts. The shadow record holds the same evidence as a real dispatch, so the owner can compare it with the work the owner would choose.

## 6. Which generated artifacts are committed, who consumes them, and where each is produced (MEDULLA-SDLC-R020.1)

Inventory of every ledger entry and every tracked file with a generated header or a
rewrite-and-compare test, as of this change. Decision: produce each at its point of use or
delete it; replace a snapshot that only diffs against itself with a test of the property.

| Path | Generator | Consumer | Why it was committed | Decision |
|------|-----------|----------|----------------------|----------|
| `clients/go/medulla_method_codec.wasm` | `scripts/build_method_codec_wasm.py` | Go client `//go:embed` (tests in CI) | embed cannot reach outside the module | built in CI and release before `go test`; gitignored |
| `clients/js/medulla_method_codec.wasm` | same | JS client at load time (tests in CI) | no build step in the package | built in CI and release before `npm test`; gitignored |
| `.config/generated-artifacts.toml`, `scripts/generated_artifacts.py` | hand pins | sanitizer exemption, `--check` | pinned the two wasm copies | deleted with the copies |
| `crates/medulla-core/ontology/decision.ttl`, `decision.shapes.ttl` | decision ontology generator | engine schema sources (`include_str!`) | read at compile time | `medulla-graph-core/build.rs` renders into `OUT_DIR` |
| `tests/fixtures/wire_vectors/*.json` (rewrite switch only; no file tracked) | wire-vector test | the test itself | compare-to-self | computed and asserted in the test |
| `tests/fixtures/method_policy_registry_v1.json` | none (hand snapshot) | one golden test | compare-to-self | deleted; the computed test already asserts the rows |
| `docs/operations/owner-store-formats.md` | `gen_owner_store_formats` example | runbook link | operator reference | example prints it on demand; test asserts the rendering |
| `docs/reference/skill-graph.generated.md` | cross-repo skill-graph builder | docs nav only | slice of a corpus published elsewhere | deleted; no in-repo consumer |
| `docs/concept_reservations.yaml` | external concept tool | nobody in this repository | aggregate of fragments | deleted |

Not generated, kept: `medulla/generated/*.py` (hand-maintained wire models; the directory name
is historical), `tests/fixtures/decision/assembly_golden.json` (cross-language input the Python
verifier reads; it changes only with a record-version change), insta plan snapshots and numeric
reference fixtures (expected outputs, not derived copies), and `vendor/` upstream sources.

### R022 census of the scripts directory (lane scripts-a, 2026-11-07)

Helper modules imported by name (_git_subprocess_env, rust_*, method_*, third_party_sources, scanner_contract) show callers by file name only and may read none; they move with their importers.

Destinations: pipelines mode (generic check, PIPE- row + hook mode), crate test / xtask (engine-specific check), xtask (wheel/sdist, build/release tooling), B (Rust-lexer stack, lane B), C (exact campaigns, lane C), delete-as-dead (no caller in pre-commit, CI, docs or other scripts). Callers lists files outside `scripts/` and `specs/` first, then scripts that call it.

| Destination | Entries | Bytes |
|---|---|---|
| B | 24 | 470878 |
| C | 8 | 323475 |
| crate test / xtask | 6 | 48038 |
| delete-as-dead | 9 | 48079 |
| pipelines mode | 29 | 433050 |
| xtask (build/release/bench tooling) | 41 | 357545 |
| xtask (wheel/sdist) | 8 | 92106 |
| total | 125 | 1773171 |

| Entry | Destination | Callers | Bytes |
|---|---|---|---|
| `_git_subprocess_env.py` | pipelines mode | none | 4952 |
| `_kernel_build.py` | xtask (build/release/bench tooling) | scripts/build_numeric_kernel.py, scripts/build_pyengine_kernel.py | 5478 |
| `bench_gate.py` | xtask (build/release/bench tooling) | .github/workflows/ci.yml, Cargo.toml, architecture/component-registry.yml, benches/r613_finance_primitives.rs, | 9430 |
| `bench_helix_compare.py` | xtask (build/release/bench tooling) | architecture/component-registry.yml, docs/benchmarks-helix.md, scripts/helix_compare/__init__.py, tests/test_b | 6181 |
| `bench_scale.py` | xtask (build/release/bench tooling) | CONTRIBUTING.md, docs/benchmarks.md, tests/test_bench_scale_internal.py, tests/test_bench_scale_smoke.py | 11519 |
| `bench_transport.py` | xtask (build/release/bench tooling) | CONTRIBUTING.md, benches/pyo3_inprocess_vs_uds_bench.rs, docs/architecture/scaling_program.md, docs/architectu | 3504 |
| `bootstrap.sh` | pipelines mode | .claude/hooks/session-start.sh, .config/pre-commit.yaml, AGENTS.md, CONTRIBUTING.md, scripts/check_cargo_advis | 2557 |
| `bounded_test_runner.py` | xtask (build/release/bench tooling) | scripts/all_features_validation_gate.sh, scripts/constrained_parallelism_gate.sh, tests/test_bounded_test_runn | 24199 |
| `build_method_codec_wasm.py` | xtask (build/release/bench tooling) | .github/workflows/ci.yml, .github/workflows/release.yml, Cargo.toml, architecture/component-registry.yml, clie | 10966 |
| `build_numeric_kernel.py` | xtask (build/release/bench tooling) | .config/pre-commit.yaml, .github/workflows/ci.yml, docs/benchmarks-helix.md, scripts/check_mounted_kernel.py | 717 |
| `build_pyengine_kernel.py` | xtask (build/release/bench tooling) | .github/workflows/ci.yml | 757 |
| `build_release_server.sh` | xtask (build/release/bench tooling) | .github/actions/folded-wheel/action.yml, .github/actions/server-binary/action.yml, Cargo.toml, medulla/skills/ | 2182 |
| `cargo_test_rescue.py` | xtask (build/release/bench tooling) | .github/workflows/ci.yml, .github/workflows/medulla-release-recovery.yml, architecture/component-registry.yml, | 8715 |
| `certify_exact_fault_restart.py` | C | .config/pre-commit.yaml, docs/operations/exact-fault-restart-certification.md, scripts/certify_exact_performan | 51718 |
| `certify_exact_knowledge_batch.py` | C | docs/operations/exact-release-campaigns.md, protocols/performance/v1/scenarios.json, scripts/check_exact_relea | 14365 |
| `certify_exact_multimodal.py` | C | docs/operations/exact-release-campaigns.md, scripts/check_exact_release_campaigns.py, tests/test_certify_exact | 56690 |
| `certify_exact_performance.py` | C | docs/performance_certification.md, tests/test_exact_performance_certification.py | 124345 |
| `certify_exact_protocol_authorization.py` | C | docs/operations/exact-release-campaigns.md, scripts/bench_scale.py, scripts/certify_exact_fault_restart.py, sc | 22951 |
| `certify_exact_reasoning_repair.py` | C | docs/operations/exact-release-campaigns.md, scripts/check_exact_release_campaigns.py, tests/test_exact_release | 24822 |
| `check_canonical_property_schema.py` | B | .config/pre-commit.yaml, docs/architecture/canonical-property-schema.md | 6837 |
| `check_cargo_advisories.sh` | pipelines mode | .config/cargo-audit-allow.txt, .config/deny.toml, .config/pre-commit.yaml, .github/workflows/ci.yml, CHANGELOG | 6458 |
| `check_cluster_extras_affected_lint.py` | crate test / xtask | .config/pre-commit.yaml, architecture/component-registry.yml, tests/test_check_cluster_extras_affected_lint.py | 12308 |
| `check_complexity_staged.py` | B | .config/pre-commit.yaml, architecture/component-registry.yml, scripts/check_tracked_privacy.py, scripts/report | 36369 |
| `check_crate_graph.py` | pipelines mode | .specify/memory/constitution.md | 5885 |
| `check_crates_io_only.py` | pipelines mode | .config/pre-commit.yaml | 6200 |
| `check_current_only_architecture.py` | B | .config/pre-commit.yaml, crates/medulla-server-served-tests/src/tests.rs, docs/architecture/epistemic-os-harde | 20188 |
| `check_decide_budgets.py` | crate test / xtask | .github/workflows/nightly-decide-budgets.yml, .github/workflows/nightly-finance-budgets.yml, architecture/comp | 2992 |
| `check_dispatch_decomposition.py` | delete-as-dead | none | 14380 |
| `check_dupehound.py` | pipelines mode | .config/pre-commit.yaml, docs/architecture/code-intelligence.md, docs/operations/dupehound-census.md, scripts/ | 18029 |
| `check_dupehound_census.py` | pipelines mode | docs/operations/dupehound-census.md, tests/test_dupehound_census.py | 71843 |
| `check_duplication.py` | pipelines mode | .config/pre-commit.yaml, docs/architecture/code-intelligence.md, tests/test_contract_landed_rows.py, tests/tes | 38328 |
| `check_durable_table_registration.py` | B | .config/pre-commit.yaml, scripts/source_gate_scope.py, tests/test_durable_table_registration_census.py | 15495 |
| `check_epistemic_operations_protocol.py` | B | .config/pre-commit.yaml, .github/workflows/ci.yml, crates/medulla-result-types/src/epistemic_operations_ext.rs | 24544 |
| `check_exact_fault_restart_harness.py` | C | .config/pre-commit.yaml, docs/operations/exact-fault-restart-certification.md, tests/test_exact_fault_restart_ | 16559 |
| `check_exact_release_campaigns.py` | C | .config/pre-commit.yaml, docs/operations/exact-release-campaigns.md | 12025 |
| `check_fanout_host_groups.py` | crate test / xtask | .config/fanout-host-groups.yml, architecture/component-registry.yml | 4935 |
| `check_integration_baseline.py` | crate test / xtask | .config/pre-commit.yaml, tests/integration_failure_baseline.txt | 12717 |
| `check_kiss_census.py` | pipelines mode | .config/pre-commit.yaml, docs/quality-gate-terms.md, tests/test_kiss_fork.py, tests/test_scanner_distribution. | 9812 |
| `check_kiss_staged.sh` | B | .config/kiss.toml, .config/pre-commit.yaml, docs/quality-gate-terms.md, scripts/kiss_diff_scope.py, tests/test | 18820 |
| `check_mint_lease_call_sites.py` | B | .config/pre-commit.yaml, crates/medulla-isolation/src/isolation/policy_lease.rs, tests/test_architecture_gate_ | 13422 |
| `check_no_pyo3.sh` | pipelines mode | CONTRIBUTING.md, Cargo.toml, architecture/component-registry.yml, crates/medulla-numeric/Cargo.toml, crates/me | 6475 |
| `check_orphan_modules.py` | B | .config/kiss.toml, .config/pre-commit.yaml, tests/test_orphan_modules_gate.py | 13747 |
| `check_p2_analytics_reasoning_architecture.py` | B | .config/pre-commit.yaml, docs/architecture/distributed-analytics-reasoning.md, tests/test_e1_guard_wiring.py,  | 10082 |
| `check_p2_modality_architecture.py` | B | .config/pre-commit.yaml, CHANGELOG.md, crates/medulla-modality/README.md, docs/architecture/modality_serving.m | 9368 |
| `check_persisted_mutation_contract.py` | B | .config/pre-commit.yaml, docs/architecture/distributed-analytics-reasoning.md, docs/architecture/mutation_batc | 83777 |
| `check_pinned_reference_resolution.py` | B | .config/pre-commit.yaml, tests/test_ci_change_scope.py | 28703 |
| `check_public_specs.py` | pipelines mode | .config/pre-commit.yaml | 12902 |
| `check_registry_test_ownership.py` | pipelines mode | .config/pre-commit.yaml, architecture/component-registry.yml, tests/test_check_registry_test_ownership.py, tes | 6572 |
| `check_rust_arch_lint.py` | B | .config/pre-commit.yaml, architecture/component-registry.yml, tests/test_arch_lint_policy.py, tests/test_scann | 27971 |
| `check_rustfmt_scope.py` | pipelines mode | .config/pre-commit.yaml, .mergequeue.yaml, tests/test_rustfmt_scope.py | 10202 |
| `check_sdist_completeness.py` | xtask (wheel/sdist) | .github/workflows/release.yml, tests/test_normal_release_publication.py, tests/test_release_full_wheel_contrac | 8164 |
| `check_sole_write_envelope.py` | pipelines mode | .config/pre-commit.yaml, .github/workflows/ci.yml, tests/test_sole_write_envelope.py | 4246 |
| `check_tracked_privacy.py` | pipelines mode | .config/pre-commit.yaml, crates/medulla-protocol-types/src/change_envelope.rs, scripts/security/check_secret_h | 57549 |
| `check_universal_read_rls.py` | B | .config/pre-commit.yaml, .github/workflows/ci.yml, crates/medulla-h-graph/src/graph_ops/terminal.rs, crates/me | 37252 |
| `check_wheel_completeness.py` | xtask (wheel/sdist) | .config/pre-commit.yaml, .github/actions/folded-wheel/action.yml, .github/workflows/ci.yml, .github/workflows/ | 15295 |
| `check_wheel_privacy.py` | xtask (wheel/sdist) | .config/pre-commit.yaml, .github/actions/folded-wheel/action.yml, .github/workflows/ci.yml, scripts/check_trac | 16431 |
| `ci_affected_crates.py` | crate test / xtask | .github/workflows/ci.yml, architecture/component-registry.yml, tests/test_ci_affected_crates.py | 8565 |
| `ci_change_scope.py` | crate test / xtask | .github/workflows/ci.yml, architecture/component-registry.yml, tests/test_ci_change_scope.py | 6521 |
| `ci_parity.sh` | pipelines mode | .config/pre-commit.yaml, AGENTS.md, medulla/skills/medulla-development/SKILL.md, scripts/check_cluster_extras_ | 1401 |
| `compare_wheel_reproducibility.py` | xtask (wheel/sdist) | .github/workflows/medulla-release-recovery.yml, .github/workflows/release.yml, .github/workflows/repro-diagnos | 15703 |
| `configure_arm64_linker.sh` | xtask (build/release/bench tooling) | .github/actions/release-maturin/action.yml, .github/workflows/arm64-linker-probe.yml, tests/test_arm64_linker. | 1588 |
| `configure_macos_linker.sh` | xtask (build/release/bench tooling) | .github/workflows/ci.yml, .github/workflows/macos-release-probe.yml, .github/workflows/release.yml, tests/test | 1341 |
| `configure_rust_path_remap.py` | xtask (build/release/bench tooling) | .github/workflows/arm64-linker-probe.yml, .github/workflows/ci.yml, .github/workflows/macos-release-probe.yml, | 21576 |
| `constrained_parallelism_gate.sh` | xtask (build/release/bench tooling) | .config/pre-commit.yaml, tests/integration_failure_baseline.txt, tests/test_bounded_test_runner_contract.py, t | 20939 |
| `dupehound_ledger.py` | pipelines mode | .config/dupehound-distinct.toml, architecture/component-registry.yml, tests/test_dupehound_ledger.py | 15143 |
| `fetch_onnxruntime.sh` | xtask (build/release/bench tooling) | .github/workflows/ci.yml, .github/workflows/medulla-release-recovery.yml, crates/medulla-tts-piper/tests/piper | 1537 |
| `fetch_server_binary.py` | xtask (build/release/bench tooling) | .github/actions/folded-wheel/action.yml, .github/actions/server-binary/action.yml | 2004 |
| `fetch_sqlite3.sh` | xtask (build/release/bench tooling) | .github/workflows/ci.yml, crates/medulla-h-txn/src/sqlite_file.rs, crates/medulla-sqlite-format/tests | 1651 |
| `fetch_whisper_test_fixture.sh` | xtask (build/release/bench tooling) | .github/workflows/ci.yml, crates/medulla-asr-whisper/src/abort_bridge.rs, crates/medulla-asr-whisper/tests/rea | 1933 |
| `gate_base_ref.sh` | pipelines mode | .config/pre-commit.yaml, tests/test_gate_base_ref.py | 796 |
| `helix_compare` | xtask (build/release/bench tooling) | architecture/component-registry.yml, crates/medulla-plan/benches/filtered_rank.rs, docs/benchmarks-helix.md, s | 56636 |
| `inject_numeric_kernel.py` | xtask (build/release/bench tooling) | .github/actions/folded-wheel/action.yml, .github/workflows/ci.yml, CHANGELOG.md, architecture/component-regist | 6660 |
| `inject_ontology.py` | xtask (build/release/bench tooling) | .github/actions/folded-wheel/action.yml, .github/workflows/ci.yml, CHANGELOG.md, build_backend.py, medulla/ont | 3939 |
| `inject_pyengine.py` | xtask (build/release/bench tooling) | architecture/component-registry.yml, crates/medulla-pyengine/src/lib.rs, docs/architecture/unified-inprocess-e | 7347 |
| `inject_server_binary.py` | xtask (build/release/bench tooling) | .github/actions/folded-wheel/action.yml, medulla/skills/medulla-development/SKILL.md, scripts/build_release_se | 3280 |
| `install_scanners.sh` | pipelines mode | .config/pre-commit.yaml, scripts/bootstrap.sh, tests/test_arch_lint_policy.py, tests/test_kiss_fork.py, tests/ | 3140 |
| `kiss_config_keys.py` | pipelines mode | .config/kiss.toml, architecture/component-registry.yml, scripts/check_kiss_staged.sh, tests/test_kiss_hook_con | 3302 |
| `kiss_diff_scope.py` | B | architecture/component-registry.yml, docs/quality-gate-terms.md, scripts/check_kiss_staged.sh, tests/test_kiss | 11030 |
| `kiss_fork.py` | pipelines mode | architecture/component-registry.yml, docs/quality-gate-terms.md, scripts/check_kiss_staged.sh, scripts/install | 3177 |
| `list_scanner_sources.py` | pipelines mode | .config/pre-commit.yaml, scripts/check_kiss_census.py | 3144 |
| `medulla_coverage_report.sh` | xtask (build/release/bench tooling) | scripts/check_tracked_privacy.py, scripts/medulla_coverage_zero_report.py | 6859 |
| `medulla_coverage_zero_report.py` | xtask (build/release/bench tooling) | scripts/medulla_coverage_report.sh | 24997 |
| `method_families.py` | B | architecture/component-registry.yml | 2502 |
| `method_policy_inventory.py` | B | tests/test_method_policy_registry.py | 16206 |
| `normalize_wheel_build_paths.py` | xtask (wheel/sdist) | .config/pre-commit.yaml, .github/actions/folded-wheel/action.yml, .github/workflows/ci.yml, build_backend.py,  | 13231 |
| `normalize_wheel_sbom.py` | xtask (wheel/sdist) | .config/pre-commit.yaml, .github/actions/folded-wheel/action.yml, .github/workflows/ci.yml, build_backend.py,  | 16311 |
| `prebuild_macos_bins.sh` | xtask (build/release/bench tooling) | .github/workflows/macos-release-probe.yml, scripts/probe_macos_release.py | 921 |
| `probe_arm64_linker.sh` | xtask (build/release/bench tooling) | .github/workflows/arm64-linker-probe.yml, tests/test_arm64_linker.py | 5949 |
| `probe_macos_release.py` | xtask (build/release/bench tooling) | .github/workflows/macos-release-probe.yml | 9688 |
| `promote_engine.sh` | xtask (build/release/bench tooling) | Cargo.toml, docs/deploy/binary_promotion.md, docs/deploy/transition-runbook.md, medulla/skills/medulla-deploy/ | 5172 |
| `push_gate_evidence.py` | pipelines mode | .config/pre-commit.yaml, architecture/component-registry.yml, docs/architecture/index.md, docs/architecture/pu | 41493 |
| `python_suite_artifact.sh` | xtask (build/release/bench tooling) | .github/workflows/ci.yml, tests/test_release_python_suite_contract.py | 1398 |
| `raft_cpu_load.py` | xtask (build/release/bench tooling) | scripts/validate-raft-cluster.sh, tests/test_validate_raft_cluster.py | 8403 |
| `recovery` | xtask (build/release/bench tooling) | .github/workflows/medulla-arm64-recovery.yml, .github/workflows/medulla-release-recovery-dispatch.yml, .github | 3706 |
| `release_build_metadata.py` | xtask (build/release/bench tooling) | .github/workflows/medulla-release-x86.yml, tests/test_normal_release_publication.py | 2330 |
| `release_publication_recovery.json` | xtask (build/release/bench tooling) | .github/workflows/release-publication-recovery.yml, tests/test_release_publication_recovery.py | 2035 |
| `release_publication_recovery.py` | xtask (build/release/bench tooling) | .github/workflows/release-publication-recovery.yml, architecture/component-registry.yml, tests/test_release_fu | 10025 |
| `release_wheel_smoke.sh` | xtask (build/release/bench tooling) | .github/workflows/ci.yml, .github/workflows/medulla-release-x86.yml, .github/workflows/release.yml, tests/test | 2171 |
| `report_complexity_terms.py` | B | .config/pre-commit.yaml, docs/quality-gate-terms.md | 6033 |
| `rust_callgraph.py` | B | scripts/check_p2_modality_architecture.py, scripts/check_universal_read_rls.py | 4117 |
| `rust_context.py` | B | none | 12606 |
| `rust_exhaustive_match.py` | B | architecture/component-registry.yml, crates/medulla-sql-lang/src/sql/classify.rs, docs/quality-gate-terms.md,  | 15925 |
| `rust_lexer.py` | B | architecture/component-registry.yml, scripts/check_kiss_staged.sh, tests/test_ci_change_scope.py, tests/test_k | 21245 |
| `rust_module_tree.py` | B | scripts/check_kiss_staged.sh, tests/test_architecture_gate_source_universe.py, tests/test_ci_change_scope.py,  | 30574 |
| `scanner_contract.py` | pipelines mode | scripts/check_dupehound_census.py, scripts/check_kiss_staged.sh, tests/test_kiss_staged.py | 20606 |
| `security` | pipelines mode | .config/dupehound-distinct.toml, .config/pre-commit.yaml, .config/repo-layout.toml, .config/security/security- | 37496 |
| `security_sanitizer.py` | pipelines mode | .config/pre-commit.yaml, tests/test_security_sanitizer_contract.py | 14102 |
| `sibling_gate.py` | delete-as-dead | none | 3081 |
| `soak_scale.py` | xtask (build/release/bench tooling) | docs/benchmarks-soak.md, tests/test_batch_binary_client.py | 39144 |
| `sole_write_envelope.allow` | pipelines mode | scripts/check_sole_write_envelope.py, tests/test_sole_write_envelope.py | 649 |
| `source_gate_scope.py` | B | none | 4065 |
| `strip_wheel_executables.py` | xtask (wheel/sdist) | .github/actions/folded-wheel/action.yml, .github/workflows/ci.yml, architecture/component-registry.yml, pyproj | 2404 |
| `third_party_sources.py` | pipelines mode | .config/third-party-sources.toml | 13620 |
| `transition_deploy.sh` | xtask (build/release/bench tooling) | docs/deploy/transition-runbook.md | 13410 |
| `validate-raft-cluster.sh` | xtask (build/release/bench tooling) | crates/medulla-raft-xshard/src/xshard_modality_harness.rs, docs/architecture/m2_raft_status.md, docs/architect | 5644 |
| `validate_cccc_census.py` | pipelines mode | .config/pre-commit.yaml, docs/quality-gate-terms.md, tests/test_scanner_distribution.py | 12971 |
| `verify_release_tag.py` | xtask (build/release/bench tooling) | .github/actions/publish-distribution/action.yml, docs/development/pypi-tombstones.md, tests/test_normal_releas | 1614 |
| `wheel_privacy_gate.sh` | xtask (wheel/sdist) | .config/pre-commit.yaml, Cargo.toml | 4567 |
