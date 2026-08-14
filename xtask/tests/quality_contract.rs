use xtask::{QualityStep, quality_steps};

#[test]
fn quality_gate_covers_format_lint_tests_and_bench_builds() {
    assert_eq!(
        quality_steps(),
        vec![
            QualityStep::new("fmt", &["--all", "--", "--check"]),
            QualityStep::new(
                "clippy",
                &[
                    "--workspace",
                    "--all-targets",
                    "--all-features",
                    "--",
                    "-D",
                    "warnings",
                ],
            ),
            QualityStep::new("test", &["--workspace", "--all-targets", "--all-features"]),
            QualityStep::new("check", &["--workspace", "--benches", "--all-features"]),
        ]
    );
}
