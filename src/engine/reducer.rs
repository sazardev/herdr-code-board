//! The pure core of the automation engine.
//!
//! Given a card, its rules and one input, decide what should happen. No I/O, no
//! database, no herdr. Everything the board does automatically is decided here,
//! which is what makes the behaviour testable without a running server.

use crate::model::{Action, AgentStatus, Card, Column, Rule};

/// Something that happened, scoped to one card.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Input {
    /// Herdr reported a new agent lifecycle state for the card's pane.
    AgentStatus(AgentStatus),
    /// The card's pane exited, closed, or its workspace went away.
    PaneGone,
    /// A periodic sweep; `now` is unix seconds.
    Tick { now: i64 },
}

/// A decision the executor must carry out. Ordered; apply them in sequence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Effect {
    /// Move the card to this lane.
    Lane(Column),
    /// Fire a rule. The executor consumes the rule's fire budget first.
    Fire { rule_id: String, action: Action },
    /// Close the card's open run row.
    FinishRun {
        outcome: String,
        detail: Option<String>,
    },
    /// Send the card's own prompt to its agent. Emitted when the agent becomes
    /// ready after a startup dialog delayed the handover.
    DeliverPrompt,
    /// Forget the herdr pane/agent this card owned.
    ClearBinding,
    /// Append to the card's audit trail.
    Log {
        kind: String,
        detail: Option<String>,
    },
}

/// Where a card lands when its agent reports `status`.
///
/// `None` means "no opinion": herdr's `unknown` does not prove anything, and a
/// status arriving before we sent a prompt is just the agent booting.
pub fn lane_for_status(card: &Card, status: AgentStatus) -> Option<Column> {
    match status {
        AgentStatus::Working => Some(Column::Running),
        AgentStatus::Blocked => Some(Column::Blocked),
        // Both mean the turn ended. `done` is the same idle state for work the
        // user has not seen yet, so it is not a stronger signal, just an unseen one.
        AgentStatus::Idle | AgentStatus::Done => {
            if card.prompt_sent {
                Some(Column::Waiting)
            } else {
                // The agent came up and is waiting for our prompt.
                None
            }
        }
        AgentStatus::Unknown => None,
    }
}

/// Rules whose trigger matches a card entering `lane` right now.
fn rules_for_lane(rules: &[Rule], lane: Column) -> impl Iterator<Item = &Rule> {
    rules.iter().filter(move |r| {
        r.enabled && r.trigger.delay_seconds().is_none() && r.trigger.watched_column() == Some(lane)
    })
}

/// Timed rules that are due for a card that has been sitting still.
fn timed_rules_due<'a>(
    card: &'a Card,
    rules: &'a [Rule],
    now: i64,
) -> impl Iterator<Item = &'a Rule> {
    rules.iter().filter(move |r| {
        let Some(delay) = r.trigger.delay_seconds() else {
            return false;
        };
        r.enabled
            && r.trigger.watched_column() == Some(card.column)
            && now - card.status_since >= delay
            && (r.max_fires == 0 || r.fired < r.max_fires)
    })
}

/// The earliest unix time a timed rule of this card could fire, if any.
///
/// The engine's timer wheel sleeps until the nearest of these instead of polling.
pub fn next_deadline(card: &Card, rules: &[Rule]) -> Option<i64> {
    rules
        .iter()
        .filter(|r| r.enabled && (r.max_fires == 0 || r.fired < r.max_fires))
        .filter(|r| r.trigger.watched_column() == Some(card.column))
        .filter_map(|r| r.trigger.delay_seconds())
        .map(|d| card.status_since + d)
        .min()
}

/// Everything that should happen to `card` because of `input`.
pub fn step(card: &Card, rules: &[Rule], input: &Input) -> Vec<Effect> {
    match input {
        Input::AgentStatus(status) => on_status(card, rules, *status),
        Input::PaneGone => on_pane_gone(card, rules),
        Input::Tick { now } => on_tick(card, rules, *now),
    }
}

fn on_status(card: &Card, rules: &[Rule], status: AgentStatus) -> Vec<Effect> {
    let mut out = Vec::new();
    if card.column.is_terminal() {
        // A late event about a card we already closed changes nothing.
        return out;
    }

    // The agent became ready before we managed to hand over the prompt — which is
    // what happens when a startup dialog blocked the dispatch. Deliver it now,
    // otherwise the card sits in its pane forever with nothing asked of it.
    if !card.prompt_sent
        && card.column.is_live()
        && matches!(status, AgentStatus::Idle | AgentStatus::Done)
    {
        out.push(Effect::DeliverPrompt);
        return out;
    }

    let Some(lane) = lane_for_status(card, status) else {
        return out;
    };
    if lane == card.column {
        return out;
    }
    enter_lane(card, rules, lane, &mut out);
    out
}

/// Move to `lane`, fire that lane's rules, and cascade an auto-completing card
/// from `waiting` straight into `done`.
fn enter_lane(card: &Card, rules: &[Rule], lane: Column, out: &mut Vec<Effect>) {
    out.push(Effect::Lane(lane));
    for rule in rules_for_lane(rules, lane) {
        out.push(Effect::Fire {
            rule_id: rule.id.clone(),
            action: rule.action.clone(),
        });
    }

    if lane == Column::Waiting && card.auto_complete {
        out.push(Effect::FinishRun {
            outcome: "done".into(),
            detail: None,
        });
        out.push(Effect::Lane(Column::Done));
        for rule in rules_for_lane(rules, Column::Done) {
            out.push(Effect::Fire {
                rule_id: rule.id.clone(),
                action: rule.action.clone(),
            });
        }
        out.push(Effect::ClearBinding);
    }
}

fn on_pane_gone(card: &Card, rules: &[Rule]) -> Vec<Effect> {
    let mut out = Vec::new();
    if !card.column.is_live() {
        return out;
    }
    out.push(Effect::ClearBinding);

    if card.attempts <= card.max_retries {
        // Retries are counted by attempts already made, so a card with
        // max_retries = 1 gets one dispatch plus one retry.
        out.push(Effect::FinishRun {
            outcome: "lost".into(),
            detail: Some("pane disappeared; retrying".into()),
        });
        out.push(Effect::Log {
            kind: "retry".into(),
            detail: Some(format!(
                "attempt {} of {}",
                card.attempts,
                card.max_retries + 1
            )),
        });
        out.push(Effect::Lane(Column::Ready));
        return out;
    }

    out.push(Effect::FinishRun {
        outcome: "failed".into(),
        detail: Some("pane disappeared".into()),
    });
    enter_lane(card, rules, Column::Failed, &mut out);
    out
}

fn on_tick(card: &Card, rules: &[Rule], now: i64) -> Vec<Effect> {
    let mut out = Vec::new();
    if card.column.is_terminal() {
        return out;
    }
    for rule in timed_rules_due(card, rules, now) {
        out.push(Effect::Fire {
            rule_id: rule.id.clone(),
            action: rule.action.clone(),
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Binding, Placement, Trigger};

    fn card(column: Column) -> Card {
        Card {
            id: "01CARD".into(),
            key: None,
            title: "review".into(),
            prompt: "look at the diff".into(),
            repo_id: None,
            session: None,
            tags: vec![],
            agent_kind: "claude".into(),
            model: None,
            extra_args: vec![],
            placement: Placement::default(),
            column,
            binding: Binding {
                pane_id: Some("w1:p2".into()),
                ..Default::default()
            },
            priority: 0,
            auto_complete: false,
            auto_answer: false,
            max_retries: 0,
            attempts: 1,
            created_at: 0,
            updated_at: 0,
            status_since: 1_000,
            dispatched_at: Some(0),
            last_error: None,
            prompt_sent: true,
        }
    }

    fn rule(id: &str, trigger: Trigger, action: Action) -> Rule {
        Rule {
            id: id.into(),
            card_id: Some("01CARD".into()),
            repo_id: None,
            trigger,
            action,
            max_fires: 0,
            fired: 0,
            enabled: true,
        }
    }

    fn enqueue(cards: &[&str]) -> Action {
        Action::Enqueue {
            cards: cards.iter().map(|s| s.to_string()).collect(),
        }
    }

    #[test]
    fn working_moves_a_card_to_running() {
        let effects = step(
            &card(Column::Ready),
            &[],
            &Input::AgentStatus(AgentStatus::Working),
        );
        assert_eq!(effects, vec![Effect::Lane(Column::Running)]);
    }

    #[test]
    fn a_turn_ending_parks_the_card_in_waiting() {
        let effects = step(
            &card(Column::Running),
            &[],
            &Input::AgentStatus(AgentStatus::Idle),
        );
        assert_eq!(effects, vec![Effect::Lane(Column::Waiting)]);
    }

    /// A card blocked on a startup dialog owns a pane but never received its
    /// prompt. When the dialog clears, the prompt has to go out, or the card sits
    /// there with an idle agent and nothing asked of it.
    #[test]
    fn an_agent_that_becomes_ready_before_the_handover_gets_the_prompt_then() {
        for column in [Column::Running, Column::Blocked] {
            let mut c = card(column);
            c.prompt_sent = false;
            assert_eq!(
                step(&c, &[], &Input::AgentStatus(AgentStatus::Idle)),
                vec![Effect::DeliverPrompt],
                "from {column}"
            );
        }
    }

    #[test]
    fn a_card_that_owns_no_pane_is_not_prompted() {
        let mut c = card(Column::Backlog);
        c.prompt_sent = false;
        assert!(step(&c, &[], &Input::AgentStatus(AgentStatus::Idle)).is_empty());
    }

    #[test]
    fn unknown_status_is_never_treated_as_progress() {
        for column in [Column::Running, Column::Waiting, Column::Blocked] {
            assert!(step(
                &card(column),
                &[],
                &Input::AgentStatus(AgentStatus::Unknown)
            )
            .is_empty());
        }
    }

    #[test]
    fn a_repeated_status_produces_no_effects() {
        assert!(step(
            &card(Column::Running),
            &[],
            &Input::AgentStatus(AgentStatus::Working)
        )
        .is_empty());
    }

    #[test]
    fn a_late_event_about_a_closed_card_is_ignored() {
        for column in [Column::Done, Column::Failed, Column::Cancelled] {
            assert!(step(
                &card(column),
                &[],
                &Input::AgentStatus(AgentStatus::Working)
            )
            .is_empty());
        }
    }

    /// The headline flow: "when this one is done, start that one."
    #[test]
    fn auto_complete_cascades_waiting_into_done_and_fires_the_done_rule() {
        let mut c = card(Column::Running);
        c.auto_complete = true;
        let rules = vec![rule("R1", Trigger::Done, enqueue(&["02NEXT"]))];

        let effects = step(&c, &rules, &Input::AgentStatus(AgentStatus::Done));

        assert_eq!(
            effects,
            vec![
                Effect::Lane(Column::Waiting),
                Effect::FinishRun {
                    outcome: "done".into(),
                    detail: None
                },
                Effect::Lane(Column::Done),
                Effect::Fire {
                    rule_id: "R1".into(),
                    action: enqueue(&["02NEXT"])
                },
                Effect::ClearBinding,
            ]
        );
    }

    #[test]
    fn without_auto_complete_the_card_stops_at_waiting() {
        let rules = vec![rule("R1", Trigger::Done, enqueue(&["02NEXT"]))];
        let effects = step(
            &card(Column::Running),
            &rules,
            &Input::AgentStatus(AgentStatus::Done),
        );
        assert_eq!(effects, vec![Effect::Lane(Column::Waiting)]);
    }

    #[test]
    fn entering_waiting_fires_waiting_rules_immediately() {
        let rules = vec![rule("R1", Trigger::Review, enqueue(&["x"]))];
        // A Review rule must not fire on entering Waiting.
        let effects = step(
            &card(Column::Running),
            &rules,
            &Input::AgentStatus(AgentStatus::Idle),
        );
        assert_eq!(effects, vec![Effect::Lane(Column::Waiting)]);
    }

    #[test]
    fn blocked_fires_its_rule_on_arrival() {
        let rules = vec![rule("R1", Trigger::Blocked, Action::Answer { choice: 1 })];
        let effects = step(
            &card(Column::Running),
            &rules,
            &Input::AgentStatus(AgentStatus::Blocked),
        );
        assert_eq!(
            effects,
            vec![
                Effect::Lane(Column::Blocked),
                Effect::Fire {
                    rule_id: "R1".into(),
                    action: Action::Answer { choice: 1 }
                }
            ]
        );
    }

    #[test]
    fn a_waiting_for_rule_fires_only_once_the_delay_has_passed() {
        let mut c = card(Column::Waiting);
        c.status_since = 1_000;
        let rules = vec![rule(
            "R1",
            Trigger::WaitingFor { seconds: 900 },
            Action::Notify {
                title: "stuck".into(),
                body: None,
            },
        )];

        assert!(step(&c, &rules, &Input::Tick { now: 1_899 }).is_empty());
        assert_eq!(
            step(&c, &rules, &Input::Tick { now: 1_900 }),
            vec![Effect::Fire {
                rule_id: "R1".into(),
                action: Action::Notify {
                    title: "stuck".into(),
                    body: None
                }
            }]
        );
    }

    #[test]
    fn a_timed_rule_watches_only_its_own_lane() {
        let rules = vec![rule(
            "R1",
            Trigger::WaitingFor { seconds: 10 },
            Action::Cancel,
        )];
        // Same elapsed time, but the card is running, not waiting.
        assert!(step(&card(Column::Running), &rules, &Input::Tick { now: 9_999 }).is_empty());
        assert!(!step(&card(Column::Waiting), &rules, &Input::Tick { now: 9_999 }).is_empty());
    }

    #[test]
    fn an_exhausted_timed_rule_stops_being_due() {
        let mut r = rule("R1", Trigger::BlockedFor { seconds: 10 }, Action::Cancel);
        r.max_fires = 1;
        r.fired = 1;
        assert!(step(&card(Column::Blocked), &[r], &Input::Tick { now: 9_999 }).is_empty());
    }

    #[test]
    fn next_deadline_is_the_earliest_pending_timer() {
        let c = card(Column::Waiting); // status_since = 1000
        let rules = vec![
            rule("R1", Trigger::WaitingFor { seconds: 900 }, Action::Cancel),
            rule("R2", Trigger::WaitingFor { seconds: 300 }, Action::Cancel),
            rule("R3", Trigger::BlockedFor { seconds: 5 }, Action::Cancel),
        ];
        assert_eq!(next_deadline(&c, &rules), Some(1_300));
        assert_eq!(next_deadline(&card(Column::Running), &rules), None);
    }

    #[test]
    fn a_lost_pane_retries_while_attempts_remain() {
        let mut c = card(Column::Running);
        c.max_retries = 2;
        c.attempts = 1;
        let effects = step(&c, &[], &Input::PaneGone);
        assert!(effects.contains(&Effect::Lane(Column::Ready)));
        assert!(effects.contains(&Effect::ClearBinding));
        assert!(!effects
            .iter()
            .any(|e| matches!(e, Effect::Lane(Column::Failed))));
    }

    #[test]
    fn a_lost_pane_fails_the_card_once_retries_are_exhausted() {
        let mut c = card(Column::Running);
        c.max_retries = 1;
        c.attempts = 2;
        let rules = vec![rule("R1", Trigger::Failed, enqueue(&["02CLEANUP"]))];
        let effects = step(&c, &rules, &Input::PaneGone);
        assert!(effects.contains(&Effect::Lane(Column::Failed)));
        assert!(effects.contains(&Effect::Fire {
            rule_id: "R1".into(),
            action: enqueue(&["02CLEANUP"])
        }));
    }

    #[test]
    fn a_pane_event_for_a_card_that_owns_no_pane_is_ignored() {
        assert!(step(&card(Column::Backlog), &[], &Input::PaneGone).is_empty());
        assert!(step(&card(Column::Done), &[], &Input::PaneGone).is_empty());
    }

    #[test]
    fn disabled_rules_never_fire() {
        let mut r = rule("R1", Trigger::Blocked, Action::Answer { choice: 1 });
        r.enabled = false;
        let effects = step(
            &card(Column::Running),
            &[r],
            &Input::AgentStatus(AgentStatus::Blocked),
        );
        assert_eq!(effects, vec![Effect::Lane(Column::Blocked)]);
    }
}
