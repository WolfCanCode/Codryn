//! **Validates: Requirements 1.1**
//!
//! Property 1: Install Prompt Ordering Invariant
//!
//! For any set of valid user responses to install prompts, the interactive install
//! flow SHALL invoke prompts in exactly this order: (1) scope selection, (2) IDE
//! selection, (3) steering choice, (4) steering intensity — regardless of the
//! specific values chosen.
//!
//! Note: When no IDEs are detected, the IDE prompt (MultiSelect) is skipped and
//! replaced with an Info message. The test verifies relative ordering of whatever
//! Select/MultiSelect prompts are made.

use codryn_cli::install::install_interactive;
use codryn_cli::prompter::{MockPrompter, MockResponse, PromptCall};
use proptest::prelude::*;

// ── Strategies ────────────────────────────────────────────────────────────────

/// Generate a valid scope selection index (0=workspace-only, 1=global, 2=both)
fn arb_scope_index() -> impl Strategy<Value = usize> {
    0..3usize
}

/// Generate a valid IDE multi-select response.
/// Since we don't control which IDEs are detected on the test machine, we generate
/// a subset of indices from 0..max_ides. If no IDEs are detected, this response
/// won't be consumed (the prompt is skipped), but we still need it in the queue
/// in case IDEs ARE present.
fn arb_ide_selection() -> impl Strategy<Value = Vec<usize>> {
    // Generate a selection of 0..5 indices in 0..9 range (covering more than enough IDEs)
    proptest::collection::vec(0..9usize, 0..5)
}

/// Generate a valid steering choice index (0=workspace-only, 1=yes, 2=no)
fn arb_steering_index() -> impl Strategy<Value = usize> {
    0..3usize
}

/// Generate a valid intensity index (0=lite, 1=full, 2=none)
fn arb_intensity_index() -> impl Strategy<Value = usize> {
    0..3usize
}

// ── Ordering verification helpers ─────────────────────────────────────────────

/// Identifies the type of a prompt call by examining its prompt text for
/// identifying keywords. Returns None for Info/ShowDiff calls.
fn classify_prompt(call: &PromptCall) -> Option<&'static str> {
    match call {
        PromptCall::Select { prompt, .. } => {
            let p = prompt.to_lowercase();
            if p.contains("scope") {
                Some("scope")
            } else if p.contains("steering") && p.contains("intensity") {
                Some("intensity")
            } else if p.contains("steering") {
                Some("steering")
            } else if p.contains("ide") {
                Some("ide")
            } else {
                // Unknown select prompt — don't interfere with ordering check
                None
            }
        }
        PromptCall::MultiSelect { prompt, .. } => {
            let p = prompt.to_lowercase();
            if p.contains("ide") {
                Some("ide")
            } else {
                None
            }
        }
        // Info and ShowDiff are not interactive prompts — skip them
        PromptCall::Info { .. } | PromptCall::ShowDiff { .. } | PromptCall::Confirm { .. } => None,
    }
}

/// The expected ordering of prompts. Each prompt that appears must respect
/// this relative order.
const EXPECTED_ORDER: &[&str] = &["scope", "ide", "steering", "intensity"];

/// Verify that the classified prompts appear in the expected relative order.
/// Some prompts may be missing (e.g., IDE when none detected), but those
/// that appear must maintain relative ordering.
fn verify_ordering(classified: &[&str]) -> bool {
    let mut last_pos = 0;
    for item in classified {
        let pos = EXPECTED_ORDER
            .iter()
            .position(|&x| x == *item)
            .expect("classified item must be in EXPECTED_ORDER");
        if pos < last_pos {
            return false;
        }
        last_pos = pos;
    }
    true
}

// ── Property Test ─────────────────────────────────────────────────────────────

proptest! {
    #![proptest_config(ProptestConfig::with_cases(50))]

    /// Property 1: Install Prompt Ordering Invariant
    ///
    /// For any valid combination of user responses, the interactive install flow
    /// invokes prompts in exactly the order: scope → IDE → steering → intensity.
    /// When no IDEs are detected, the IDE prompt is skipped but all other prompts
    /// maintain their relative order.
    #[test]
    fn install_prompts_always_in_order(
        scope_idx in arb_scope_index(),
        ide_selection in arb_ide_selection(),
        steering_idx in arb_steering_index(),
        intensity_idx in arb_intensity_index(),
    ) {
        // Build responses: scope (Select), IDE (MultiSelect), steering (Select), intensity (Select)
        // If no IDEs are detected on this machine, the MultiSelect won't be consumed,
        // but we include it in case IDEs are present.
        let responses = vec![
            MockResponse::Select(scope_idx),
            MockResponse::MultiSelect(ide_selection),
            MockResponse::Select(steering_idx),
            MockResponse::Select(intensity_idx),
        ];

        let prompter = MockPrompter::new(responses);

        // Execute with dry_run=true to avoid filesystem side effects
        let result = install_interactive(&prompter, false, true, None);

        // The flow should succeed (all responses are valid indices that map to
        // valid enum variants via match-with-fallback-to-default).
        prop_assert!(result.is_ok(), "install_interactive failed: {:?}", result.err());

        // Extract the call history and classify each prompt
        let history = prompter.call_history();
        let classified: Vec<&str> = history
            .iter()
            .filter_map(|call| classify_prompt(call))
            .collect();

        // Must have at least scope, steering, and intensity (IDE may be skipped)
        prop_assert!(
            classified.len() >= 3,
            "Expected at least 3 classified prompts (scope, steering, intensity), got {:?}",
            classified
        );

        // Verify the ordering invariant
        prop_assert!(
            verify_ordering(&classified),
            "Prompt ordering violated! Got: {:?}, expected relative order: {:?}",
            classified,
            EXPECTED_ORDER
        );

        // If IDE prompt is present, it must be exactly 4 prompts in the expected order
        if classified.len() == 4 {
            prop_assert_eq!(classified[0], "scope");
            prop_assert_eq!(classified[1], "ide");
            prop_assert_eq!(classified[2], "steering");
            prop_assert_eq!(classified[3], "intensity");
        } else if classified.len() == 3 {
            // No IDE prompt (no IDEs detected) — verify scope, steering, intensity order
            prop_assert_eq!(classified[0], "scope");
            prop_assert_eq!(classified[1], "steering");
            prop_assert_eq!(classified[2], "intensity");
        }
    }
}
